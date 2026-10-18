// Unit tests for the npm launcher.
//
// Every one of these exercises a path a user hits on a bad day: an unsupported platform, an
// install that dropped optional dependencies, a binary without the execute bit, a Ctrl-C. The
// launcher's contract is that none of those produce a stack trace and none of them are reported
// as exit code 1, which is reserved for "the quality gate failed".
import assert from 'node:assert/strict'
import test from 'node:test'
import { chmod, mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { TARGETS, detectLibc, resolveBinary, run, targetFor } from '../lib/launcher.mjs'

// Resolution cases share the platform triple, while keeping failure expectations in each test.
const linuxPackageOptions = (manifest) => ({
  platform: 'linux',
  arch: 'x64',
  libc: 'gnu',
  override: '',
  resolvePackage: () => manifest,
})

test('every published target resolves from its own platform triple', () => {
  let resolved = 0
  for (const [key, target] of Object.entries(TARGETS)) {
    const found = targetFor(target.platform, target.arch, target.libc)
    assert.equal(found.key, key)
    assert.equal(found.packageName, target.packageName)
    resolved += 1
  }
  // A floor, not decoration: an empty manifest would make the loop above vacuously pass.
  assert.ok(resolved >= 8, `only resolved ${resolved} targets`)
})

test('linux resolution discriminates glibc from musl', () => {
  assert.equal(targetFor('linux', 'x64', 'gnu').packageName, 'vitest-dedup-linux-x64-gnu')
  assert.equal(targetFor('linux', 'x64', 'musl').packageName, 'vitest-dedup-linux-x64-musl')
  assert.equal(targetFor('linux', 'arm64', 'gnu').packageName, 'vitest-dedup-linux-arm64-gnu')
  assert.equal(targetFor('linux', 'arm64', 'musl').packageName, 'vitest-dedup-linux-arm64-musl')
})

test('windows targets carry the .exe binary name', () => {
  assert.equal(targetFor('win32', 'x64').binaryName, 'vitest-dedup.exe')
  assert.equal(targetFor('win32', 'x64').packageName, 'vitest-dedup-windows-x64-msvc')
  assert.equal(targetFor('win32', 'arm64').packageName, 'vitest-dedup-windows-arm64-msvc')
})

test('an unsupported platform is named, not stack-traced', () => {
  assert.throws(
    () => targetFor('freebsd', 'x64'),
    /unsupported platform freebsd-x64.*darwin-arm64.*win32-x64.*cargo install vitest-dedup/s,
  )
  // A 32-bit linux host is the realistic case: supported platform, unsupported architecture.
  assert.throws(() => targetFor('linux', 'ia32', 'gnu'), /unsupported platform linux-ia32-gnu/)
})

test('libc detection falls back to the statically linked build', () => {
  assert.equal(detectLibc({ getReport: () => ({ header: { glibcVersionRuntime: '2.39' } }) }), 'gnu')
  assert.equal(detectLibc({ getReport: () => ({ header: {} }) }), 'musl')
  assert.equal(detectLibc({ getReport: () => { throw new Error('blocked') } }), 'musl')
  // `null`, not `undefined`: the parameter defaults to `process.report`, so passing `undefined`
  // silently reads the *host's* real libc -- which is 'musl' on a macOS developer machine and
  // 'gnu' on a glibc CI runner. The case under test is "no report available at all".
  assert.equal(detectLibc(null), 'musl')
})

test('a missing platform package says how to fix the install', () => {
  assert.throws(
    () =>
      resolveBinary({
        platform: 'linux',
        arch: 'x64',
        libc: 'gnu',
        override: '',
        resolvePackage() {
          throw new Error('Cannot find module')
        },
      }),
    /vitest-dedup-linux-x64-gnu is not installed.*optional dependencies.*VITEST_DEDUP_BINARY/s,
  )
})

test('a platform package without a runnable binary is reported as such', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'vitest-dedup-empty-'))
  const manifest = join(directory, 'package.json')
  await writeFile(manifest, '{}\n')
  assert.throws(
    () =>
      resolveBinary(linuxPackageOptions(manifest)),
    /no vitest-dedup binary at /,
  )
})

test('a non-executable binary is reported as such', async (t) => {
  if (process.platform === 'win32') return t.skip('no execute bit on Windows')
  const directory = await mkdtemp(join(tmpdir(), 'vitest-dedup-mode-'))
  await mkdir(join(directory, 'bin'), { recursive: true })
  const manifest = join(directory, 'package.json')
  const binary = join(directory, 'bin', 'vitest-dedup')
  await writeFile(manifest, '{}\n')
  await writeFile(binary, '#!/bin/sh\nexit 0\n')
  await chmod(binary, 0o644)
  assert.throws(
    () =>
      resolveBinary(linuxPackageOptions(manifest)),
    /is not executable/,
  )
})

test('resolution walks the real platform package layout', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'vitest-dedup-platform-'))
  await mkdir(join(directory, 'bin'), { recursive: true })
  const manifest = join(directory, 'package.json')
  const binary = join(directory, 'bin', 'vitest-dedup')
  await writeFile(manifest, '{}\n')
  await writeFile(binary, '#!/bin/sh\nexit 0\n')
  await chmod(binary, 0o755)
  assert.equal(
    resolveBinary({
      ...linuxPackageOptions(manifest),
      resolvePackage(specifier) {
        assert.equal(specifier, 'vitest-dedup-linux-x64-gnu/package.json')
        return manifest
      },
    }),
    binary,
  )
})

test('VITEST_DEDUP_BINARY overrides platform resolution entirely', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'vitest-dedup-override-'))
  const binary = join(directory, 'locally-built')
  await writeFile(binary, '#!/bin/sh\nexit 0\n')
  await chmod(binary, 0o755)
  assert.equal(
    resolveBinary({
      platform: 'linux',
      override: binary,
      resolvePackage() {
        throw new Error('resolution must not be attempted when an override is set')
      },
    }),
    binary,
  )
})

test('argv is forwarded verbatim', () => {
  let seen
  run(['check', 'src', '--no-metrics', '--rule', 'duplicate-test=warning', '--', '--not-a-flag'], {
    binary: '/controlled/vitest-dedup',
    spawn(binary, args, options) {
      seen = { binary, args, options }
      return { status: 0, signal: null }
    },
  })
  assert.deepEqual(seen, {
    binary: '/controlled/vitest-dedup',
    args: ['check', 'src', '--no-metrics', '--rule', 'duplicate-test=warning', '--', '--not-a-flag'],
    options: { stdio: 'inherit' },
  })
})

test('all three documented exit codes are forwarded unchanged', () => {
  for (const status of [0, 1, 2]) {
    assert.equal(
      run([], { binary: '/controlled/vitest-dedup', spawn: () => ({ status, signal: null }) }),
      status,
      `exit code ${status}`,
    )
  }
})

test('launcher-side failures report 2, never 1', () => {
  let diagnostic = ''
  const stderr = { write: (value) => { diagnostic += value } }

  assert.equal(
    run([], {
      binary: '/controlled/vitest-dedup',
      spawn: () => ({ status: null, signal: null, error: new Error('permission denied') }),
      stderr,
    }),
    2,
  )
  assert.match(diagnostic, /^vitest-dedup: failed to start .*permission denied/m)

  // An unknown outcome must not be reported as a clean run.
  assert.equal(
    run([], {
      binary: '/controlled/vitest-dedup',
      spawn: () => ({ status: null, signal: null }),
      stderr,
    }),
    2,
  )
})

test('a signalled analyzer reports the shell convention, not a quality-gate failure', () => {
  let diagnostic = ''
  assert.equal(
    run([], {
      binary: '/controlled/vitest-dedup',
      spawn: () => ({ status: null, signal: 'SIGINT' }),
      stderr: { write: (value) => { diagnostic += value } },
    }),
    130,
  )
  assert.match(diagnostic, /terminated by SIGINT/)
})

// Supply-chain scanners flag a URL or IP address literal in a package's code as something it may
// fetch at runtime. The launcher fetches nothing, so none of its shipped code carries one.
test('the shipped launcher code contains no URL or IP address literal', async () => {
  const root = join(dirname(fileURLToPath(import.meta.url)), '..')
  const files = []
  for (const directory of ['bin', 'lib']) {
    for (const name of await readdir(join(root, directory))) files.push(join(root, directory, name))
  }
  assert.ok(files.length >= 3, `expected the launcher files, found ${files.length}`)
  for (const file of files) {
    const source = await readFile(file, 'utf8')
    assert.doesNotMatch(source, /\bhttps?:\/\/|\b\d{1,3}(?:\.\d{1,3}){3}\b/, `${file} carries a URL or IP address literal`)
  }
})
