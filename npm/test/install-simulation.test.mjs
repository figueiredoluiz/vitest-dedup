// End-to-end proof that the published shape actually works, without publishing anything.
//
// Everything else in this directory tests a part in isolation. This packs the real tarballs npm
// would upload, installs them into a throwaway project the way a user would, and runs
// `node_modules/.bin/vitest-dedup` against source trees chosen to produce each documented exit
// code. It is the only test that exercises the `bin` entry, the shebang, npm's bin shim, the
// allowlist in `files`, platform-package resolution through real `node_modules`, and argv and exit
// code forwarding through a real process -- all at once, which is also the only way the gaps
// between them get covered.
//
// Locally it skips when there is no `target/release/vitest-dedup`; in CI, where the build step
// precedes it, `VITEST_DEDUP_REQUIRE_INSTALL_SIM=1` turns a missing binary into a failure so this
// can never silently become a no-op.
import assert from 'node:assert/strict'
import test from 'node:test'
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { chmod, mkdtemp, mkdir, readFile, rmdir, stat, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { targetFor } from '../lib/targets.mjs'
import { writePlatformPackage } from '../../scripts/release/prepare-npm-packages.mjs'

const root = resolve(fileURLToPath(new URL('../../', import.meta.url)))
const binaryName = process.platform === 'win32' ? 'vitest-dedup.exe' : 'vitest-dedup'
const builtBinary = join(root, 'target', 'release', binaryName)
const required = process.env.VITEST_DEDUP_REQUIRE_INSTALL_SIM === '1'

const npm = (args, cwd) =>
  spawnSync(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, {
    cwd,
    encoding: 'utf8',
    shell: process.platform === 'win32',
  })

const DUPLICATED = `import { test, expect } from 'vitest'

test('alpha', () => {
  const total = 1 + 1
  expect(total).toBe(2)
})

test('beta', () => {
  const total = 1 + 1
  expect(total).toBe(2)
})
`

const CLEAN = `import { test, expect } from 'vitest'

test('adds two values', () => {
  expect(1 + 1).toBe(2)
})
`

test('the packed distribution installs and runs from node_modules', async (t) => {
  if (!existsSync(builtBinary)) {
    assert.ok(!required, `VITEST_DEDUP_REQUIRE_INSTALL_SIM=1 but ${builtBinary} is missing`)
    return t.skip('run `cargo build --release` first')
  }

  const target = targetFor(
    process.platform,
    process.arch,
    process.platform === 'linux' ? 'gnu' : undefined,
  )
  assert.ok(target, `no release target for ${process.platform}-${process.arch}`)

  const scratch = await mkdtemp(join(tmpdir(), 'vitest-dedup-install-'))
  const platformRoot = join(root, 'npm', 'platforms', target.packageDirectory)
  const binDirectory = join(platformRoot, 'bin')
  const generated = [join(platformRoot, 'README.md'), join(platformRoot, 'LICENSE'),
    join(binDirectory, target.binaryName)]
  const binExisted = existsSync(binDirectory)
  const saved = await Promise.all(generated.map(async (path) => existsSync(path)
    ? { path, bytes: await readFile(path), mode: (await stat(path)).mode }
    : { path }))
  t.after(async () => {
    await rm(scratch, { recursive: true, force: true })
    for (const item of saved) {
      if (item.bytes) {
        await writeFile(item.path, item.bytes)
        await chmod(item.path, item.mode)
      } else {
        await rm(item.path, { force: true })
      }
    }
    if (!binExisted && existsSync(binDirectory)) await rmdir(binDirectory)
  })

  // 1. Build the platform package from the manifest and the locally compiled binary, then pack
  //    both tarballs exactly as the release would.
  const version = JSON.parse(await readFile(join(root, 'package.json'), 'utf8')).version
  await writePlatformPackage({ root, target, version, binary: builtBinary })

  const packed = {}
  for (const [label, directory] of [['platform', platformRoot], ['launcher', root]]) {
    const result = npm(['pack', '--pack-destination', scratch, '--json'], directory)
    assert.equal(result.status, 0, `npm pack (${label}) failed: ${result.stderr}`)
    packed[label] = join(scratch, JSON.parse(result.stdout)[0].filename)
    assert.ok(existsSync(packed[label]), `${label} tarball was not produced`)
  }

  // 2. Install both into a throwaway project. `--omit=optional` keeps npm from reaching for the
  //    seven platform packages this host cannot use (and which are not published yet); the host's
  //    own package is installed explicitly, which is the state npm produces for a real user.
  const project = join(scratch, 'project')
  await mkdir(project, { recursive: true })
  await writeFile(
    join(project, 'package.json'),
    `${JSON.stringify(
      {
        name: 'install-simulation',
        version: '0.0.0',
        private: true,
        dependencies: {
          'vitest-dedup': `file:${packed.launcher}`,
          [target.packageName]: `file:${packed.platform}`,
        },
      },
      null,
      2,
    )}\n`,
  )
  const install = npm(
    ['install', '--omit=optional', '--no-audit', '--no-fund', '--ignore-scripts'],
    project,
  )
  assert.equal(install.status, 0, `npm install failed: ${install.stderr}`)

  const shim = join(project, 'node_modules', '.bin', 'vitest-dedup')
  assert.ok(existsSync(shim) || existsSync(`${shim}.cmd`), 'npm did not create the bin shim')

  const cli = (args, cwd = project) =>
    spawnSync(process.execPath, [join(project, 'node_modules', 'vitest-dedup', 'npm', 'bin', 'vitest-dedup.js'), ...args], {
      cwd,
      encoding: 'utf8',
    })

  // 3. Exit code 0: an analyzed tree with no findings. An empty directory is intentionally not
  //    the control here: zero analyzed tests is an operational error, because it cannot prove the
  //    project is clean.
  const clean = join(scratch, 'clean')
  await mkdir(clean, { recursive: true })
  await writeFile(join(clean, 'clean.test.ts'), CLEAN)
  assert.equal(cli(['.'], clean).status, 0, 'a clean tree must exit 0')

  // 4. Exit code 1: the quality gate failed. Anything other than 1 here -- including a launcher
  //    error surfacing as 1 -- would make CI unable to tell duplicated tests from a broken tool.
  const duplicated = join(scratch, 'duplicated')
  await mkdir(join(duplicated, 'src'), { recursive: true })
  await writeFile(join(duplicated, 'src', 'a.test.ts'), DUPLICATED)
  const gate = cli(['.'], duplicated)
  assert.equal(gate.status, 1, `expected the quality gate to fail, got ${gate.status}`)
  assert.match(gate.stdout, /duplicate/i)

  // 5. Exit code 2: an operational error.
  assert.equal(
    cli(['.', '--config', join(scratch, 'nope', 'missing.json')], clean).status,
    2,
    'a bad configuration path must exit 2',
  )

  // 6. Argv forwarding, proved by an effect only the native binary can produce: a flag that
  //    changes where a report is written.
  const prefix = join(scratch, 'reports', 'forwarded')
  const reported = cli(['.', '--reporters', 'json', '--output', prefix], duplicated)
  assert.equal(reported.status, 1)
  assert.ok(existsSync(`${prefix}.json`), '--output did not reach the native binary')
  const report = JSON.parse(await readFile(`${prefix}.json`, 'utf8'))
  assert.ok(report.findings.length > 0, 'the forwarded run produced no findings')

  // 7. The launcher's own failures are still 2, with the platform package removed from under it.
  await rm(join(project, 'node_modules', target.packageName), { recursive: true, force: true })
  const orphaned = cli(['.'], clean)
  assert.equal(orphaned.status, 2, 'a missing platform package must be an operational error')
  assert.match(orphaned.stderr, /vitest-dedup: the prebuilt binary package .* is not installed/)
})
