import assert from 'node:assert/strict'
import test from 'node:test'
import { execFileSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { copyFileSync, existsSync, linkSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { basename, join } from 'node:path'
import { TARGETS } from '../lib/targets.mjs'
import { recoverNpmBinaries } from '../../scripts/release/recover-npm-binaries.mjs'
import { verifyNpmBinaries } from '../../scripts/release/verify-npm-binaries.mjs'
import { prepareAll } from '../../scripts/release/prepare-npm-packages.mjs'
import { platformManifest } from '../../scripts/release/platform-package.mjs'

const repo = 'owner/project'
const sha = 'a'.repeat(40)
const keys = Object.keys(TARGETS)

/** Real archives and binaries; only the GitHub transport and authentication service are simulated. */
function fixture(t, { version = '0.1.0', layout, badTarget = keys.at(-1) } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'npm-recovery-'))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const archives = join(root, 'published')
  const output = join(root, 'native')
  mkdirSync(archives)
  const contents = new Map()
  for (const [key, target] of Object.entries(TARGETS)) {
    const stem = `vitest-dedup-${version}-${key}`
    const staging = join(root, stem)
    mkdirSync(staging)
    const bytes = Buffer.from(`published binary for ${key}\0\xff`)
    contents.set(key, bytes)
    writeFileSync(join(staging, 'README.md'), 'not an executable')
    const member = `${stem}/${target.binaryName}`
    if (key === badTarget && layout === 'symlink') {
      symlinkSync('README.md', join(staging, target.binaryName))
    } else if (key === badTarget && layout === 'hardlink') {
      linkSync(join(staging, 'README.md'), join(staging, target.binaryName))
    } else if (key === badTarget && layout === 'directory') {
      mkdirSync(join(staging, target.binaryName))
    } else if (!(key === badTarget && layout === 'missing')) {
      writeFileSync(join(staging, target.binaryName), key === badTarget && layout === 'empty' ? Buffer.alloc(0) : bytes)
    }
    const archive = join(archives, `${stem}.tar.gz`)
    const members = key === badTarget && layout === 'duplicate' ? [member, member] : [
      `${stem}/README.md`, ...(key === badTarget && layout === 'missing' ? [] : [member]),
    ]
    execFileSync('tar', ['-czf', archive, '-C', root, ...members], { timeout: 10_000, stdio: 'pipe' })
    const digest = createHash('sha256').update(readFileSync(archive)).digest('hex')
    writeFileSync(`${archive}.sha256`, `${digest}  ${basename(archive)}\n`)
  }
  const calls = []
  const options = { version, repo, sha, output }
  const run = (command, args, opts) => {
    calls.push([command, args])
    if (command === 'gh' && args[0] === 'release' && args[1] === 'view') {
      assert.deepEqual(args, ['release', 'view', `v${version}`, '--repo', repo, '--json', 'tagName,isDraft'])
      return Buffer.from(JSON.stringify({ tagName: `v${version}`, isDraft: false }))
    }
    if (command === 'gh' && args[0] === 'release' && args[1] === 'download') {
      assert.equal(args[2], `v${version}`)
      assert.equal(args[args.indexOf('--repo') + 1], repo)
      const directory = args[args.indexOf('--dir') + 1]
      for (let i = 0; i < args.length; i++) {
        if (args[i] === '--pattern') copyFileSync(join(archives, args[i + 1]), join(directory, args[i + 1]))
      }
      return Buffer.alloc(0)
    }
    if (command === 'gh' && args[0] === 'attestation') {
      assert.deepEqual(args.slice(3), [
        '--repo', repo, '--signer-workflow', `${repo}/.github/workflows/release.yml`,
        '--source-ref', `refs/tags/v${version}`, '--source-digest', sha,
      ])
      return Buffer.alloc(0)
    }
    assert.equal(command, 'tar', 'recovery must not build or execute a binary')
    const archive = args[1]
    assert.ok(calls.some(([c, a]) => c === 'gh' && a[0] === 'attestation' && a[2] === archive),
      'authenticate this archive before passing it to tar')
    return execFileSync(command, args, opts)
  }
  return { root, archives, contents, calls, options, run, output }
}

for (const version of ['0.1.0', '0.1.0-rc.1']) {
  test(`recovery reuses all published binary bytes without a build (${version})`, async (t) => {
    const f = fixture(t, { version })
    assert.equal(existsSync(f.output), false)
    assert.deepEqual(recoverNpmBinaries({ ...f.options, run: f.run }), keys)
    assert.equal(f.calls.filter(([c, a]) => c === 'gh' && a[0] === 'attestation').length, 8)
    assert.deepEqual(readdirSync(f.output).sort(), Object.values(TARGETS).map(t => `binary-${t.rustTarget}`).sort())
    for (const [key, target] of Object.entries(TARGETS)) {
      assert.deepEqual(readFileSync(join(f.output, `binary-${target.rustTarget}`, target.binaryName)), f.contents.get(key))
      const dir = join(f.root, 'npm', 'platforms', target.packageDirectory)
      mkdirSync(dir, { recursive: true })
      writeFileSync(join(dir, 'package.json'), JSON.stringify(platformManifest(target, version)))
    }
    assert.equal(verifyNpmBinaries({ binaries: f.output, archives: f.archives, version }).length, 8)
    writeFileSync(join(f.root, 'LICENSE'), 'fixture license')
    assert.equal((await prepareAll({ root: f.root, artifacts: f.output, version })).length, 8)
    for (const [key, target] of Object.entries(TARGETS)) {
      assert.deepEqual(readFileSync(join(f.root, 'npm/platforms', target.packageDirectory, 'bin', target.binaryName)), f.contents.get(key))
    }
  })
}

for (const layout of ['missing', 'duplicate', 'symlink', 'hardlink', 'directory', 'empty']) {
  test(`a ${layout} binary in the last archive leaves no partial output`, (t) => {
    // Windows runners cannot create symlinks without elevated privileges.
    if (layout === 'symlink' && process.platform === 'win32') return t.skip('requires symlink privilege')
    const f = fixture(t, { layout })
    assert.throws(() => recoverNpmBinaries({ ...f.options, run: f.run }))
    assert.equal(f.calls.filter(([c, a]) => c === 'gh' && a[0] === 'attestation').length, 8,
      'the first seven targets succeeded before the last malformed member was rejected')
    assert.equal(existsSync(f.output), false)
    assert.equal(readdirSync(f.root).some(name => name.startsWith('.npm-recovery-')), false)
  })
}

for (const fault of ['missing archive', 'missing checksum', 'checksum mismatch', 'wrong checksum name', 'attestation rejected', 'draft', 'wrong tag', 'release missing']) {
  test(`${fault} fails closed without creating output`, (t) => {
    const f = fixture(t)
    const archive = `vitest-dedup-${f.options.version}-${keys.at(-1)}.tar.gz`
    if (fault === 'missing archive') rmSync(join(f.archives, archive))
    if (fault === 'missing checksum') rmSync(join(f.archives, `${archive}.sha256`))
    if (fault === 'checksum mismatch') writeFileSync(join(f.archives, archive), 'tampered')
    if (fault === 'wrong checksum name') {
      const sidecar = join(f.archives, `${archive}.sha256`)
      writeFileSync(sidecar, readFileSync(sidecar, 'utf8').replace(archive, 'wrong.tar.gz'))
    }
    const run = (command, args, options) => {
      if (command === 'gh' && args[0] === 'attestation' && fault === 'attestation rejected' && args[2].endsWith(archive)) throw new Error('attestation rejected')
      if (command === 'gh' && args[1] === 'view') {
        if (fault === 'release missing') throw new Error('release missing')
        if (fault === 'draft') return Buffer.from(JSON.stringify({ tagName: `v${f.options.version}`, isDraft: true }))
        if (fault === 'wrong tag') return Buffer.from(JSON.stringify({ tagName: 'v9.9.9', isDraft: false }))
      }
      return f.run(command, args, options)
    }
    assert.throws(() => recoverNpmBinaries({ ...f.options, run }))
    assert.equal(existsSync(f.output), false)
    assert.equal(readdirSync(f.root).some(name => name.startsWith('.npm-recovery-')), false)
    if (fault === 'checksum mismatch' || fault === 'wrong checksum name') {
      assert.equal(f.calls.some(([c, a]) => c === 'gh' && a[0] === 'attestation' && a[2].endsWith(archive)), false)
    }
  })
}

test('invalid identity and existing output are rejected before running commands', (t) => {
  const f = fixture(t)
  for (const invalid of [{ version: '../1' }, { repo: '--bad' }, { sha: '' }]) {
    assert.throws(() => recoverNpmBinaries({ ...f.options, ...invalid, run: f.run }), /invalid/)
  }
  mkdirSync(f.output)
  writeFileSync(join(f.output, 'sentinel'), 'keep')
  assert.throws(() => recoverNpmBinaries({ ...f.options, run: f.run }), /already exists/)
  assert.equal(readFileSync(join(f.output, 'sentinel'), 'utf8'), 'keep')
  assert.equal(f.calls.length, 0)
})


test('the recovery CLI wires environment identity and real subprocess output into all native inputs', (t) => {
  const f = fixture(t)
  // Preload a GitHub transport stub inside the child, preserving real tar and CLI execution.
  // A Node module works on Windows too, without a shebang executable or PATH shim.
  const shim = join(f.root, 'github-transport.mjs')
  writeFileSync(shim, `
import assert from 'node:assert/strict'
import childProcess from 'node:child_process'
import { syncBuiltinESMExports } from 'node:module'
import { copyFileSync } from 'node:fs'
import { join } from 'node:path'
const execute = childProcess.execFileSync
childProcess.execFileSync = (command, args, options) => {
  if (command !== 'gh') return execute(command, args, options)
  if (args[0] === 'release' && args[1] === 'view') {
    return Buffer.from(JSON.stringify({ tagName: 'v0.1.0', isDraft: false }))
  }
  if (args[0] === 'release' && args[1] === 'download') {
    const dest = args[args.indexOf('--dir') + 1]
    for (let i = 0; i < args.length; i++) if (args[i] === '--pattern') {
      copyFileSync(join(process.env.FIXTURE_ARCHIVES, args[i + 1]), join(dest, args[i + 1]))
    }
    return Buffer.alloc(0)
  }
  assert.equal(args[0], 'attestation')
  assert.equal(args[args.indexOf('--source-digest') + 1], process.env.RELEASE_SHA)
  return Buffer.alloc(0)
}
syncBuiltinESMExports()
`)
  const env = { ...process.env, VERSION: '0.1.0',
    GH_REPO: repo, RELEASE_SHA: sha, FIXTURE_ARCHIVES: f.archives }
  const script = new URL('../../scripts/release/recover-npm-binaries.mjs', import.meta.url)
  const result = execFileSync(process.execPath,
    ['--import', pathToFileURL(shim).href, fileURLToPath(script), f.output],
    { env, timeout: 30_000, encoding: 'utf8' })
  assert.match(result, /recovered 8 authenticated release binaries/)
  for (const [key, target] of Object.entries(TARGETS)) {
    assert.deepEqual(readFileSync(join(f.output, `binary-${target.rustTarget}`, target.binaryName)), f.contents.get(key))
  }
  assert.throws(() => execFileSync(process.execPath, [fileURLToPath(script)], { env, timeout: 5000, stdio: 'pipe' }),
    error => error.status === 1 && /usage:/.test(error.stderr.toString()))
})
