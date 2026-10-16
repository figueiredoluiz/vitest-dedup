// Publishing is the one irreversible step in the release. These tests pin the two properties that
// cannot be recovered from if they are wrong: the order, and what counts as evidence that a
// version is already live.
import assert from 'node:assert/strict'
import test from 'node:test'
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { TARGETS } from '../lib/targets.mjs'
import {
  npmInvocation,
  npmPublishArguments,
  publishNpmPackages,
  publishOrder,
  registryVersionState,
  runNpmCommand,
} from '../../scripts/release/publish-npm-packages.mjs'

test('the launcher package is published last, after all eight platform packages', () => {
  const order = publishOrder()
  assert.equal(order.length, Object.keys(TARGETS).length + 1)
  assert.equal(order.at(-1), '.')
  assert.deepEqual(
    order.slice(0, -1).sort(),
    Object.values(TARGETS).map((t) => `npm/platforms/${t.packageDirectory}`).sort(),
  )
})

test('publish arguments request public access and provenance', () => {
  assert.deepEqual(npmPublishArguments('npm/platforms/darwin-arm64', '1.0.0'), [
    'publish',
    'npm/platforms/darwin-arm64',
    '--access',
    'public',
    '--provenance',
    '--tag',
    'latest',
  ])
  assert.deepEqual(npmPublishArguments('.', '1.0.0-rc.1'), ['publish', '.', '--access', 'public', '--provenance', '--tag', 'next'])
})

test('only a clean 404 counts as "not published yet"', () => {
  assert.equal(registryVersionState({ status: 0, stdout: '"0.1.0"', stderr: '' }, '0.1.0'), 'published')
  assert.equal(
    registryVersionState({ status: 1, stdout: '', stderr: 'npm error code E404' }, '0.1.0'),
    'missing',
  )
  // An auth failure or a rate limit must not be read as "missing" and republished over.
  assert.throws(
    () => registryVersionState({ status: 1, stdout: '', stderr: 'npm error code E401' }, '0.1.0'),
    /refusing to publish without a reliable registry check/,
  )
  assert.throws(
    () => registryVersionState({ status: 0, stdout: '"0.2.0"', stderr: '' }, '0.1.0'),
    /npm reported version "0.2.0" while checking 0.1.0/,
  )
  assert.throws(
    () => registryVersionState({ status: 0, stdout: 'not json', stderr: '' }, '0.1.0'),
    /invalid JSON/,
  )
  assert.throws(() => registryVersionState({ error: new Error('offline') }, '0.1.0'), /offline/)
})

test('npm is invoked through Node on Windows, where npm is a .cmd shim', () => {
  assert.deepEqual(
    npmInvocation({
      platform: 'win32',
      nodeExecutable: 'C:\\node\\node.exe',
      npmExecutablePath: 'C:\\node\\node_modules\\npm\\bin\\npm-cli.js',
    }),
    {
      executable: 'C:\\node\\node.exe',
      prefixArguments: ['C:\\node\\node_modules\\npm\\bin\\npm-cli.js'],
    },
  )
  assert.deepEqual(npmInvocation({ platform: 'linux' }), { executable: 'npm', prefixArguments: [] })
})

test('the default npm runner actually works on this machine', () => {
  const result = runNpmCommand(['--version'], { capture: true, root: process.cwd() })
  assert.equal(result.status, 0, result.stderr)
  assert.match(result.stdout, /^\d+\.\d+\.\d+/)
})

/** Return a temporary release root, controllable registry runner and recorded publishes; register tree cleanup. */
async function publishFixture(t, { registry, version = '1.0.0', tag = 'latest' }) {
  const root = await mkdtemp(join(tmpdir(), 'vitest-dedup-publish-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  await writeFile(
    join(root, 'package.json'),
    JSON.stringify({
      name: 'vitest-dedup',
      version,
      optionalDependencies: Object.fromEntries(
        Object.values(TARGETS).map((target) => [target.packageName, version]),
      ),
    }),
  )
  for (const target of Object.values(TARGETS)) {
    const directory = join(root, 'npm', 'platforms', target.packageDirectory)
    await mkdir(directory, { recursive: true })
    await writeFile(
      join(directory, 'package.json'),
      JSON.stringify({ name: target.packageName, version }),
    )
  }

  const published = []
  const runNpm = (args) => {
    if (args[0] === 'view') {
      const [name] = args[1].split('@')
      return registry(name, published)
    }
    assert.equal(args.filter((arg) => arg === '--tag').length, 1)
    assert.equal(args[args.indexOf('--tag') + 1], tag)
    published.push(args[1])
    return { status: 0 }
  }
  return { root, published, runNpm }
}

for (const [version, tag] of [
  ['1.0.0', 'latest'], ['0.2.0', 'latest'],
  ['1.0.0-rc.1', 'next'], ['1.0.0-beta.2', 'next'], ['1.0.0-0', 'next'],
]) {
  test(`all nine packages at ${version} publish with explicit ${tag}`, async (t) => {
    const { root, published, runNpm } = await publishFixture(t, {
      version, tag,
      registry: () => ({ status: 1, stdout: '', stderr: 'npm error code E404' }),
    })
    await publishNpmPackages({ root, runNpm })
    assert.deepEqual(published, publishOrder())
  })
}

for (const [version, tag] of [['1.0.0', 'latest'], ['1.0.0-rc.1', 'next']]) {
  test(`a rerun of ${version} skips what is already live and finishes the rest`, async (t) => {
    const alreadyLive = new Set(['vitest-dedup-darwin-arm64', 'vitest-dedup-darwin-x64'])
    const { root, published, runNpm } = await publishFixture(t, {
      version, tag,
      registry: (name) =>
        alreadyLive.has(name)
          ? { status: 0, stdout: JSON.stringify(version), stderr: '' }
          : { status: 1, stdout: '', stderr: 'npm error code E404' },
    })

    await publishNpmPackages({ root, runNpm })

    assert.equal(published.length, 7, 'two platform packages were already live')
    assert.equal(published.at(-1), '.', 'the launcher is still published last')
  })
}

test('the launcher is refused while any platform package is unconfirmed', async (t) => {
  // The registry reports every platform package missing and every publish "succeeds", but one
  // name never becomes confirmed -- the shape a silently skipped matrix leg produces.
  const { root, runNpm } = await publishFixture(t, {
    registry: () => ({ status: 1, stdout: '', stderr: 'npm error code E404' }),
  })
  const sabotaged = (args, options) => {
    if (args[0] === 'publish' && args[1].endsWith('win32-x64')) {
      return { status: 0, skipped: true }
    }
    return runNpm(args, options)
  }
  // Drop the win32-x64 package from the confirmed set by making its manifest name mismatch.
  await writeFile(
    join(root, 'npm', 'platforms', 'win32-x64', 'package.json'),
    JSON.stringify({ name: 'vitest-dedup-windows-x64-msvc-typo', version: '1.0.0' }),
  )
  await assert.rejects(
    publishNpmPackages({ root, runNpm: sabotaged }),
    /refusing to publish vitest-dedup@1\.0\.0.*vitest-dedup-windows-x64-msvc/s,
  )
})

test('a failing publish stops the run rather than continuing to the launcher', async (t) => {
  const { root, runNpm } = await publishFixture(t, {
    registry: () => ({ status: 1, stdout: '', stderr: 'npm error code E404' }),
  })
  const failing = (args, options) =>
    args[0] === 'publish' && args[1].endsWith('linux-x64-gnu')
      ? { status: 1 }
      : runNpm(args, options)
  await assert.rejects(
    publishNpmPackages({ root, runNpm: failing }),
    /npm publish failed for vitest-dedup-linux-x64-gnu@1\.0\.0/,
  )
})
