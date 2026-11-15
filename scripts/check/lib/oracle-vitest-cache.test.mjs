import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { cacheEnabled, cacheKey, cacheRoot, cachedCollector, pruneCache } from './oracle-vitest-cache.mjs'

/** Run `body` with a fresh checkout-shaped directory and cache directory, always removed. */
function fixture(body) {
  const base = mkdtempSync(join(tmpdir(), 'oracle-cache-test-'))
  const root = join(base, 'root')
  const cache = join(base, 'cache')
  try {
    for (const [path, text] of Object.entries({
      'package.json': '{"name":"x"}',
      'package-lock.json': '{}',
      'fixtures/a/a.test.ts': "test('a', () => {})",
      'fixtures/a/node_modules/pkg/index.js': 'export const x = 1',
      'scripts/check/lib/collection-probe-runner.mjs': 'export default 1',
      'node_modules/vitest/package.json': '{"version":"5.1.0"}',
      'node_modules/vitest4/package.json': '{"version":"4.0.0"}',
      'src/lib.rs': 'pub fn f() {}',
    })) {
      mkdirSync(join(root, path, '..'), { recursive: true })
      writeFileSync(join(root, path), text)
    }
    return body({ root, cache })
  } finally { rmSync(base, { recursive: true, force: true }) }
}

/** A collector over `root` that counts real collections. */
function counting(root, cache, env = {}) {
  let calls = 0
  const real = (_root, dir) => { calls++; return new Map([[`${dir}/a.test.ts`, { tests: ['a'], hooks: [], collectErrors: [] }]]) }
  const cached = cachedCollector(root, real, { env, base: cache })
  return { cached, calls: () => calls }
}

test('a second collection under the same key is read back, equal to the first', () => fixture(({ root, cache }) => {
  const first = counting(root, cache)
  const collected = first.cached.collect(root, 'fixtures/a')
  const second = counting(root, cache)
  const reused = second.cached.collect(root, 'fixtures/a')
  assert.deepEqual(reused, collected)
  assert.ok(reused instanceof Map)
  assert.equal(second.calls(), 0)
  assert.deepEqual(second.cached.stats, { hits: 1, misses: 0, enabled: true, storageError: null })
  // Another directory under the same key is collected.
  second.cached.collect(root, 'fixtures/b')
  assert.equal(second.calls(), 1)
}))

test('anything Vitest could read changes the key; the analyzer source does not', () => fixture(({ root }) => {
  const before = cacheKey(root)
  assert.equal(cacheKey(root), before, 'stable')
  const changes = [
    ['fixtures/a/a.test.ts', "test('b', () => {})"],
    ['fixtures/a/node_modules/pkg/index.js', 'export const x = 2'],
    ['fixtures/new/new.test.ts', ''],
    ['scripts/check/lib/collection-probe-runner.mjs', 'export default 2'],
    ['package-lock.json', '{"v":2}'],
    ['tsconfig.json', '{}'],
    ['node_modules/vitest/package.json', '{"version":"5.2.0"}'],
    ['node_modules/vitest4/package.json', '{"version":"4.1.0"}'],
  ]
  let previous = before
  for (const [path, text] of changes) {
    mkdirSync(join(root, path, '..'), { recursive: true })
    writeFileSync(join(root, path), text)
    const next = cacheKey(root)
    assert.notEqual(next, previous, path)
    previous = next
  }
  writeFileSync(join(root, 'src', 'lib.rs'), 'pub fn g() {}')
  assert.equal(cacheKey(root), previous, 'Rust source is not something Vitest reads')
  assert.notEqual(cacheKey(root, 'v99.0.0'), previous, 'Node version')
}))

test('the environment Vitest receives is keyed; what the gate strips or sets per call is not', () => fixture(({ root, cache }) => {
  const base = { PATH: '/bin' }
  const key = env => cacheKey(root, 'v24', env)
  assert.notEqual(key({ ...base, SKIP_REST: '1' }), key(base), 'a variable a fixture may read')
  assert.notEqual(key({ ...base, SKIP_REST: '1' }), key({ ...base, SKIP_REST: '2' }))
  assert.equal(key({ ...base, NODE_V8_COVERAGE: '/tmp/cov' }), key(base), 'the oracle strips coverage from the child')
  assert.equal(key({ ...base, VITEST_DEDUP_ORACLE_OUT: '/tmp/out' }), key(base), 'set per call')
  // Through the collector: the same files under another environment are collected again.
  counting(root, cache, base).cached.collect(root, 'fixtures/a')
  const changed = counting(root, cache, { ...base, SKIP_REST: '1' })
  changed.cached.collect(root, 'fixtures/a')
  assert.equal(changed.calls(), 1)
  const same = counting(root, cache, base)
  same.cached.collect(root, 'fixtures/a')
  assert.equal(same.calls(), 0)
}))

test('unusable storage turns the cache off and keeps every collection', () => fixture(({ root, cache }) => {
  // The cache root is a file: nothing can be listed or created under it.
  writeFileSync(cache, 'not a directory')
  const blocked = counting(root, cache)
  assert.equal(blocked.cached.collect(root, 'fixtures/a').size, 1)
  assert.equal(blocked.cached.stats.enabled, false)
  assert.match(blocked.cached.stats.storageError, /ENOTDIR|EEXIST/)
  rmSync(cache)
  // The key directory cannot be created: the first write fails, the result stands, later ones skip.
  mkdirSync(cache)
  writeFileSync(join(cache, 'k'), 'not a directory')
  let calls = 0
  const writing = cachedCollector(root, (_root, dir) => { calls++; return new Map([[dir, {}]]) }, { env: {}, base: cache, key: 'k' })
  assert.deepEqual([...writing.collect(root, 'fixtures/a')], [['fixtures/a', {}]])
  assert.ok(writing.stats.storageError)
  assert.deepEqual([...writing.collect(root, 'fixtures/b')], [['fixtures/b', {}]])
  assert.equal(calls, 2)
  assert.equal(writing.stats.misses, 2)
  assert.deepEqual(readdirSync(cache), ['k'], 'no temporary file left behind')
}))

test('a symlink is keyed by its target text', { skip: process.platform === 'win32' && 'symlinks need privileges on Windows' }, () => fixture(({ root }) => {
  symlinkSync('a.test.ts', join(root, 'fixtures', 'a', 'link.test.ts'))
  const before = cacheKey(root)
  rmSync(join(root, 'fixtures', 'a', 'link.test.ts'))
  symlinkSync('other.test.ts', join(root, 'fixtures', 'a', 'link.test.ts'))
  assert.notEqual(cacheKey(root), before)
}))

test('CI and the opt-out neither read nor write', () => fixture(({ root, cache }) => {
  counting(root, cache).cached.collect(root, 'fixtures/a')
  for (const env of [{ CI: 'true' }, { VITEST_DEDUP_ORACLE_CACHE: '0' }]) {
    assert.equal(cacheEnabled(env), false)
    const off = counting(root, join(cache, 'off'), env)
    off.cached.collect(root, 'fixtures/a')
    assert.equal(off.calls(), 1, JSON.stringify(env))
    assert.deepEqual(off.cached.stats, { hits: 0, misses: 0, enabled: false, storageError: null })
  }
  assert.equal(existsSync(join(cache, 'off')), false, 'nothing written while off')
  assert.equal(cacheEnabled({}), true)
}))

test('a failed collection stores nothing, and a corrupt entry is collected again', () => fixture(({ root, cache }) => {
  const failing = cachedCollector(root, () => { throw new Error('vitest list collected nothing') }, { env: {}, base: cache })
  assert.throws(() => failing.collect(root, 'fixtures/a'), /collected nothing/)
  const after = counting(root, cache)
  after.cached.collect(root, 'fixtures/a')
  assert.equal(after.calls(), 1, 'the failure was not cached')
  const [key] = readdirSync(cache)
  for (const name of readdirSync(join(cache, key))) writeFileSync(join(cache, key, name), '{"dir":')
  const again = counting(root, cache)
  again.cached.collect(root, 'fixtures/a')
  assert.equal(again.calls(), 1, 'a corrupt entry is a miss')
  // An entry that parses but is not this directory's collection is a miss too.
  for (const stored of ['{"dir":"fixtures/a","files":null}', '{"dir":"fixtures/other","files":[]}']) {
    for (const name of readdirSync(join(cache, key))) writeFileSync(join(cache, key, name), stored)
    const wrong = counting(root, cache)
    wrong.cached.collect(root, 'fixtures/a')
    assert.equal(wrong.calls(), 1, stored)
  }
  assert.equal(readdirSync(join(cache, key)).some(name => name.endsWith('.tmp')), false)
}))

test('old key directories are pruned; recent ones and unrelated names are kept', () => fixture(({ cache }) => {
  const old = 'a'.repeat(64)
  const recent = 'b'.repeat(64)
  for (const name of [old, recent, 'notes']) mkdirSync(join(cache, name), { recursive: true })
  const now = Date.now()
  // `notes` is as old, but no key directory: only names the cache made are removed.
  for (const name of [old, 'notes']) utimesSync(join(cache, name), new Date(now - 8 * 86400000), new Date(now - 8 * 86400000))
  pruneCache(cache, now)
  assert.deepEqual(readdirSync(cache).sort(), [recent, 'notes'])
  pruneCache(join(cache, 'absent'))
}))

test('a run that only reads a key keeps it from being pruned', () => fixture(({ root, cache }) => {
  counting(root, cache).cached.collect(root, 'fixtures/a')
  const [key] = readdirSync(cache)
  // Last used six days ago: still kept, and reading it now restarts its week.
  const old = new Date(Date.now() - 6 * 86400000)
  utimesSync(join(cache, key), old, old)
  const reader = counting(root, cache)
  pruneCache(cache, Date.now() + 6 * 86400000)
  assert.deepEqual(readdirSync(cache), [key])
  reader.cached.collect(root, 'fixtures/a')
  assert.equal(reader.calls(), 0)
}))

test('the cache lives under XDG_CACHE_HOME when it is set', () => {
  assert.equal(cacheRoot({ XDG_CACHE_HOME: '/x' }), join('/x', 'vitest-dedup', 'oracle-vitest'))
  assert.match(cacheRoot({}), /\.cache[/\\]vitest-dedup[/\\]oracle-vitest$/)
})
