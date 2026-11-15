import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, utimesSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import { execFileSync } from 'node:child_process'
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
      '.gitignore': 'notes.local/\nfixtures/a/node_modules/\n',
    })) {
      mkdirSync(join(root, path, '..'), { recursive: true })
      writeFileSync(join(root, path), text)
    }
    execFileSync('git', ['init', '--quiet'], { cwd: root })
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
  assert.deepEqual(second.cached.stats, { hits: 1, misses: 0, enabled: true, offReason: null })
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
    // Anything outside the old fixture and gate trees counts too.
    ['docs/guide.md', '# guide'],
    ['tests/fixtures/config/vitest.config.ts', 'export default {}'],
    // npm's record of the installed tree changes on any install, `--no-save` included.
    ['node_modules/.package-lock.json', '{"packages":{"node_modules/@test-kit/runtime":{}}}'],
  ]
  let previous = before
  for (const [path, text] of changes) {
    mkdirSync(join(root, path, '..'), { recursive: true })
    writeFileSync(join(root, path), text)
    const next = cacheKey(root)
    assert.notEqual(next, previous, path)
    previous = next
  }
  for (const [path, text] of [['notes.local/big.json', '{}'], ['src/lib.rs', 'pub fn g() {}'], ['tests/cli.rs', 'fn t() {}'], ['target/release/vitest-dedup', 'binary'], ['.git/description', 'edited'], ['node_modules/pkg/index.js', 'x']]) {
    mkdirSync(join(root, path, '..'), { recursive: true })
    writeFileSync(join(root, path), text)
    assert.equal(cacheKey(root), previous, `${path} is not collection input`)
  }
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
  assert.match(blocked.cached.stats.offReason, /ENOTDIR|EEXIST/)
  rmSync(cache)
  // The key directory cannot be created: the first write fails, the result stands, later ones skip.
  mkdirSync(cache)
  writeFileSync(join(cache, 'k'), 'not a directory')
  let calls = 0
  const writing = cachedCollector(root, (_root, dir) => { calls++; return new Map([[dir, {}]]) }, { env: {}, base: cache, key: 'k' })
  assert.deepEqual([...writing.collect(root, 'fixtures/a')], [['fixtures/a', {}]])
  assert.ok(writing.stats.offReason)
  assert.deepEqual([...writing.collect(root, 'fixtures/b')], [['fixtures/b', {}]])
  assert.equal(calls, 2)
  assert.equal(writing.stats.misses, 2)
  assert.deepEqual(readdirSync(cache), ['k'], 'no temporary file left behind')
}))

test('any symlink in the checkout turns the cache off, and collections still run', { skip: process.platform === 'win32' && 'symlinks need privileges on Windows' }, () => fixture(({ root, cache }) => {
  const outside = join(root, '..', 'hop.ts')
  const link = join(root, 'fixtures', 'a', 'helper.ts')
  for (const [target, setup] of [
    ['a.test.ts', () => {}], // inside the fixture tree
    [join(root, '..', 'shared.ts'), () => writeFileSync(join(root, '..', 'shared.ts'), '')], // outside the checkout
    // A chain through an outside link back into the fixture tree: retargeting the hop changes nothing the key could see.
    [outside, () => symlinkSync(join(root, 'fixtures', 'a', 'a.test.ts'), outside)],
    [join(root, 'missing.ts'), () => {}], // dangling
  ]) {
    rmSync(link, { force: true })
    rmSync(outside, { force: true })
    setup()
    symlinkSync(target, link)
    assert.throws(() => cacheKey(root), /fixtures\/a\/helper\.ts is a symlink/, target)
    const off = counting(root, cache)
    assert.equal(off.cached.collect(root, 'fixtures/a').size, 1)
    assert.equal(off.cached.stats.enabled, false)
    assert.match(off.cached.stats.offReason, /is a symlink/)
  }
  // A git-listed link elsewhere in the checkout counts too.
  rmSync(link)
  symlinkSync('package.json', join(root, 'docs-link.json'))
  assert.throws(() => cacheKey(root), /docs-link\.json is a symlink/)
}))

test('a package linked into node_modules turns the cache off unless it is a workspace of the install', { skip: process.platform === 'win32' && 'symlinks need privileges on Windows' }, () => fixture(({ root, cache }) => {
  writeFileSync(join(root, 'package.json'), '{"name":"x","workspaces":["npm/platforms/*"]}')
  mkdirSync(join(root, 'npm', 'platforms', 'darwin-arm64'), { recursive: true })
  symlinkSync('../npm/platforms/darwin-arm64', join(root, 'node_modules', 'vitest-dedup-darwin-arm64'))
  mkdirSync(join(root, 'node_modules', '.bin'))
  symlinkSync('../vitest/vitest.mjs', join(root, 'node_modules', '.bin', 'vitest'))
  assert.match(cacheKey(root), /^[0-9a-f]{64}$/, 'workspace and .bin links are allowed')
  // `npm link` of a package outside the install: edits to it would not change npm's record.
  mkdirSync(join(root, '..', 'runtime'))
  mkdirSync(join(root, 'node_modules', '@test-kit'))
  symlinkSync(join(root, '..', 'runtime'), join(root, 'node_modules', '@test-kit', 'runtime'))
  assert.throws(() => cacheKey(root), /node_modules\/@test-kit\/runtime is a linked package/)
  const off = counting(root, cache)
  assert.equal(off.cached.collect(root, 'fixtures/a').size, 1)
  assert.match(off.cached.stats.offReason, /linked package/)
  rmSync(join(root, 'node_modules', '@test-kit', 'runtime'))
  // Linked by hand into a dependency's own dependency tree.
  mkdirSync(join(root, 'node_modules', 'vitest', 'node_modules'))
  symlinkSync(join(root, '..', 'runtime'), join(root, 'node_modules', 'vitest', 'node_modules', 'tinyexec'))
  assert.throws(() => cacheKey(root), /node_modules\/vitest\/node_modules\/tinyexec is a linked package/)
  rmSync(join(root, 'node_modules', 'vitest', 'node_modules', 'tinyexec'))
  symlinkSync(join(root, 'missing'), join(root, 'node_modules', 'dangling'))
  assert.throws(() => cacheKey(root), /node_modules\/dangling does not resolve/)
}))

test('a hook snapshot links node_modules to its checkout, whose workspaces are the ones allowed', { skip: process.platform === 'win32' && 'symlinks need privileges on Windows' }, () => fixture(({ root }) => {
  // The checkout that installed the packages, as the hook's snapshot sees it through a link.
  const install = join(root, '..', 'install')
  mkdirSync(join(install, 'npm', 'platforms', 'darwin-arm64'), { recursive: true })
  writeFileSync(join(install, 'package.json'), '{"workspaces":["npm/platforms/*"]}')
  mkdirSync(join(install, 'node_modules'))
  symlinkSync('../npm/platforms/darwin-arm64', join(install, 'node_modules', 'vitest-dedup-darwin-arm64'))
  rmSync(join(root, 'node_modules'), { recursive: true })
  symlinkSync(join(install, 'node_modules'), join(root, 'node_modules'))
  // The snapshot's own manifest names no workspaces; the install's does.
  assert.match(cacheKey(root), /^[0-9a-f]{64}$/)
}))

test('a checkout git cannot list turns the cache off', () => fixture(({ root, cache }) => {
  rmSync(join(root, '.git'), { recursive: true, force: true })
  assert.throws(() => cacheKey(root), /git cannot list/)
  const off = counting(root, cache)
  assert.equal(off.cached.collect(root, 'fixtures/a').size, 1)
  assert.match(off.cached.stats.offReason, /git cannot list/)
}))

test('a tracked file deleted from the checkout changes the key', () => fixture(({ root }) => {
  execFileSync('git', ['add', 'package.json'], { cwd: root })
  const before = cacheKey(root)
  rmSync(join(root, 'package.json'))
  assert.notEqual(cacheKey(root), before)
}))

test('CI and the opt-out neither read nor write', () => fixture(({ root, cache }) => {
  counting(root, cache).cached.collect(root, 'fixtures/a')
  for (const env of [{ CI: 'true' }, { VITEST_DEDUP_ORACLE_CACHE: '0' }]) {
    assert.equal(cacheEnabled(env), false)
    const off = counting(root, join(cache, 'off'), env)
    off.cached.collect(root, 'fixtures/a')
    assert.equal(off.calls(), 1, JSON.stringify(env))
    assert.deepEqual(off.cached.stats, { hits: 0, misses: 0, enabled: false, offReason: null })
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
