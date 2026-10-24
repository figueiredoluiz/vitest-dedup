// Unit tests for the collection oracle's comparison and its missing-module stubs.
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { missingModuleStubs, wholeModuleImports } from './collection-oracle-stubs.mjs'
import { analyzerView, compareDirectory, configSource, hookSite, matchKnown, minus, ownConfig, vitestInstallError, vitestView } from './collection-oracle.mjs'

/** A file the probe collected cleanly, with `over` replacing any field. */
const collected = (over = {}) => ({
  tests: [],
  hooks: [],
  suiteHookCount: over.hooks?.length ?? 0,
  registrationCount: over.hooks?.length ?? 0,
  unlocatedRegistrations: 0,
  stubReads: [],
  collectErrors: [],
  unprobed: false,
  ...over,
})

/** The analyzer's side, from plain objects keyed by declaring file. */
const analyzed = ({ tests = {}, hooks = {}, excluded = [] } = {}) => ({
  tests: new Map(Object.entries(tests)),
  hooks: new Map(Object.entries(hooks)),
  excluded: new Set(excluded),
})

const kinds = (result) => result.disagreements.map((d) => `${d.kind} ${d.path} ${d.item}`)

test('a multiset difference keeps every unmatched copy', () => {
  assert.deepEqual(minus(['a', 'a', 'b'], ['a']), ['a', 'b'])
  assert.deepEqual(minus(['a'], ['a', 'a']), [])
})

test('a hook site is kind, path and line, without the column', () => {
  assert.equal(hookSite({ kind: 'beforeEach', path: 'a.test.ts', line: 4, column: 13 }), 'beforeEach@a.test.ts:4')
})

test('agreeing sides report nothing and count what was compared', () => {
  const vitest = new Map([['a.test.ts', collected({ tests: ['s > t', 's > t'], hooks: ['beforeEach@a.test.ts:3'] })]])
  const result = compareDirectory(vitest, analyzed({ tests: { 'a.test.ts': ['s > t', 's > t'] }, hooks: { 'a.test.ts': ['beforeEach@a.test.ts:3'] } }))
  assert.deepEqual(result.disagreements, [])
  assert.deepEqual(result.compared, ['a.test.ts'])
  assert.equal(result.testsCompared, 2)
  assert.equal(result.hooksCompared, 1)
})

test('titles on one side only are missing or extra tests, one per copy', () => {
  const vitest = new Map([['a.test.ts', collected({ tests: ['only vitest', 'twice', 'twice'] })]])
  const result = compareDirectory(vitest, analyzed({ tests: { 'a.test.ts': ['twice', 'only analyzer'] } }))
  assert.deepEqual(kinds(result), ['missing-test a.test.ts only vitest', 'missing-test a.test.ts twice', 'extra-test a.test.ts only analyzer'])
})

test('a test an imported runner declares is keyed by the file that registered it', () => {
  const view = analyzerView([
    { type: 'test', path: 'runner.ts', registrationFile: 'slugs.test.ts', title: 'page lowercases' },
    { type: 'test', path: 'runner.ts', registrationFile: 'skipped.test.ts', title: 'from a skipped file' },
  ])
  const vitest = new Map([
    ['slugs.test.ts', collected({ tests: ['page lowercases'] })],
    ['skipped.test.ts', collected({ stubReads: ['./x#y'], tests: ['from a skipped file'] })],
  ])
  assert.deepEqual(compareDirectory(vitest, view).disagreements, [])
  const elsewhere = analyzerView([{ type: 'test', path: 'other.test.ts', title: 'never collected' }])
  assert.deepEqual(kinds(compareDirectory(vitest, elsewhere)), [
    'missing-test slugs.test.ts page lowercases',
    'extra-test other.test.ts never collected',
  ])
})

test('a hook recorded where Vitest registers none is an extra hook; one never read is missing', () => {
  const vitest = new Map([['a.test.ts', collected({ hooks: ['afterEach@a.test.ts:9'] })]])
  const result = compareDirectory(vitest, analyzed({ hooks: { 'a.test.ts': ['beforeEach@a.test.ts:4'] } }))
  assert.deepEqual(kinds(result), ['missing-hook a.test.ts afterEach@a.test.ts:9', 'extra-hook a.test.ts beforeEach@a.test.ts:4'])
})

test('a hook registered once per loop iteration is one site, and a helper hook is its own line', () => {
  const vitest = new Map([['a.test.ts', collected({ hooks: ['afterEach@a.test.ts:5', 'afterEach@a.test.ts:5', 'beforeEach@seed.ts:6'] })]])
  const result = compareDirectory(vitest, analyzed({ hooks: { 'a.test.ts': ['afterEach@a.test.ts:5'], 'seed.ts': ['beforeEach@seed.ts:6'] } }))
  assert.deepEqual(result.disagreements, [])
  assert.equal(result.hooksCompared, 2)
})

test('skipped files are bucketed and their tests and hooks are compared on neither side', () => {
  const vitest = new Map([
    ['broken.test.ts', collected({ collectErrors: ['describe is not defined\nstack'] })],
    ['stubbed.test.ts', collected({ stubReads: ['./cart#enabled'], tests: ['t'] })],
    ['excluded.test.ts', collected({ tests: ['t'] })],
    ['../outside/x.test.ts', collected({ tests: ['t'] })],
  ])
  const result = compareDirectory(
    vitest,
    analyzed({
      tests: { 'excluded.test.ts': ['other'], 'stubbed.test.ts': ['other'] },
      hooks: { 'broken.test.ts': ['beforeEach@broken.test.ts:2'] },
      excluded: ['excluded.test.ts'],
    }),
  )
  assert.deepEqual(result.disagreements, [])
  assert.deepEqual(result.compared, [])
  assert.deepEqual(result.skipped.get('uncollectable'), [{ path: 'broken.test.ts', detail: 'describe is not defined' }])
  assert.deepEqual(result.skipped.get('stubbed'), [{ path: 'stubbed.test.ts', detail: './cart#enabled' }])
  assert.deepEqual(result.skipped.get('excluded').map((e) => e.path), ['excluded.test.ts'])
  assert.deepEqual(result.skipped.get('outside-root').map((e) => e.path), ['../outside/x.test.ts'])
})

test('a helper hook that only a skipped file registers is compared on neither side', () => {
  const vitest = new Map([
    ['stubbed.test.ts', collected({ stubReads: ['./cart#enabled'], hooks: ['beforeEach@helper.ts:3'] })],
    ['plain.test.ts', collected({ hooks: ['afterEach@shared.ts:7'] })],
  ])
  const result = compareDirectory(
    vitest,
    analyzed({ hooks: { 'helper.ts': ['beforeEach@helper.ts:3'], 'shared.ts': ['afterEach@shared.ts:7', 'afterAll@shared.ts:9'] } }),
  )
  assert.deepEqual(kinds(result), ['extra-hook shared.ts afterAll@shared.ts:9'])
  assert.equal(result.hooksCompared, 1)
})

test('benchmarks and type tests are bucketed, not reported as extra tests', () => {
  const vitest = new Map([['types.test-d.ts', collected({ unprobed: true })]])
  const result = compareDirectory(vitest, analyzed({ tests: { 'perf.bench.ts': ['parses'], 'other.test-d.ts': ['types'] } }))
  assert.deepEqual(result.disagreements, [])
  assert.deepEqual(result.skipped.get('bench'), [{ path: 'perf.bench.ts', detail: '1 benchmarks' }])
  assert.deepEqual(result.skipped.get('type-test').map((e) => e.path), ['types.test-d.ts', 'other.test-d.ts'])
})

test('a probe that missed a registration, or never saw a listed file, is a probe error', () => {
  const vitest = new Map([
    ['short.test.ts', collected({ hooks: ['beforeEach@short.test.ts:2'], suiteHookCount: 2 })],
    ['unlocated.test.ts', collected({ registrationCount: 1, suiteHookCount: 1, unlocatedRegistrations: 1 })],
    ['unprobed.test.ts', collected({ unprobed: true })],
  ])
  const result = compareDirectory(vitest, analyzed())
  assert.deepEqual(result.skipped.get('probe-error').map((e) => e.path), ['short.test.ts', 'unlocated.test.ts', 'unprobed.test.ts'])
  assert.deepEqual(result.compared, [])
})

/** The stub plugin's hooks bound to a fake Rollup context whose resolver knows `known`. */
function plugin(known) {
  const [resolver, inspector] = missingModuleStubs('/repo/fixtures')
  const context = { resolve: async (id) => (known.includes(id) ? { id } : null) }
  // The inspector's `this.parse` is Vite's; here the "code" is the AST itself, or a string to fail on.
  const parsing = { parse: (code) => (typeof code === 'string' ? (() => { throw new Error('unparsable') })() : code) }
  return { transform: inspector.transform.bind(parsing), resolveId: resolver.resolveId.bind(context), load: resolver.load.bind(context) }
}

test('only an unresolvable import from inside the root becomes a stub', async () => {
  const { resolveId } = plugin(['./real'])
  assert.equal(await resolveId('./real', '/repo/fixtures/a.test.ts', {}), null)
  assert.equal(await resolveId('./cart', '/elsewhere/a.test.ts', {}), null)
  assert.equal(await resolveId('./cart', undefined, {}), null)
  const id = await resolveId('./cart', '/repo/fixtures/a.test.ts', {})
  assert.equal(await resolveId(id, '/repo/fixtures/a.test.ts', {}), null)
  assert.ok(id.startsWith('\0'))
})

test('a stub reports each named read and nothing the runtime reads', async () => {
  const { transform, resolveId, load } = plugin([])
  transform(program(), '/repo/fixtures/a.test.ts')
  const source = load(await resolveId('./cart', '/repo/fixtures/a.test.ts', {}))
  assert.equal(load('/repo/fixtures/real.ts'), null)
  const exports = Object.create(null)
  new Function('__vite_ssr_exports__', source)(exports)
  const reads = []
  globalThis.__vitestDedupStubRead = (specifier, key) => reads.push(`${specifier}#${key}`)
  try {
    assert.equal(exports.enabled, undefined)
    assert.equal(exports.then, undefined)
    assert.equal(exports[Symbol.toStringTag], undefined)
  } finally {
    delete globalThis.__vitestDedupStubRead
  }
  assert.deepEqual(reads, ['./cart#enabled'])
})

test('the probe records become one view per file, with listed files the probe missed marked', () => {
  const files = vitestView(
    [{
      file: '/d/a.test.ts',
      tests: [{ title: 's > t', mode: 'run' }],
      registrations: [{ kind: 'beforeEach', file: '/d/seed.ts', line: 6 }, { kind: 'afterEach', file: null, line: null }],
      suiteHooks: { beforeEach: 1, afterEach: 1 },
      stubReads: [],
      collectErrors: [],
    }],
    ['/d/a.test.ts', '/d/b.test.ts', '/d/b.test.ts'],
    '/d',
  )
  assert.deepEqual([...files.keys()], ['a.test.ts', 'b.test.ts'])
  assert.deepEqual(files.get('a.test.ts').hooks, ['beforeEach@seed.ts:6'])
  assert.equal(files.get('a.test.ts').suiteHookCount, 2)
  assert.equal(files.get('a.test.ts').unlocatedRegistrations, 1)
  assert.equal(files.get('a.test.ts').unprobed, false)
  assert.equal(files.get('b.test.ts').unprobed, true)
})

test('the analyzer records become tests and hook sites per declaring file', () => {
  const view = analyzerView([
    { type: 'hook', path: 'dir\\seed.ts', kind: 'beforeEach', line: 6, column: 3 },
    { type: 'test', path: 'a.test.ts', title: 's > t' },
    { type: 'excluded-file', path: 'b.test.ts' },
    { type: 'finding' },
    { type: 'summary' },
  ])
  assert.deepEqual(Object.fromEntries(view.hooks), { 'dir/seed.ts': ['beforeEach@dir/seed.ts:6'] })
  assert.deepEqual(Object.fromEntries(view.tests), { 'a.test.ts': ['s > t'] })
  assert.deepEqual([...view.excluded], ['b.test.ts'])
})

test('a directory without a config gets the default include; one with a config keeps it and gains the probe', () => {
  const dir = mkdtempSync(join(tmpdir(), 'oracle-config-'))
  try {
    const plain = configSource(dir)
    assert.equal(ownConfig(dir), null)
    assert.match(plain, /missingModuleStubs/)
    assert.match(plain, /collection-probe-runner\.mjs/)
    assert.match(plain, /"include":\["\*\*\/\*\.\{test,spec\}/)
    writeFileSync(join(dir, 'vitest.config.ts'), 'export default {}\n')
    assert.equal(ownConfig(dir), join(dir, 'vitest.config.ts'))
    const wrapped = configSource(dir)
    assert.match(wrapped, /mergeConfig/)
    assert.match(wrapped, /vitest\.config\.ts/)
    assert.match(wrapped, /base\.test\?\.projects\?\.map/)
    assert.doesNotMatch(wrapped, /"include"/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('the gate requires the installed Vitest to be the pinned one', () => {
  const root = mkdtempSync(join(tmpdir(), 'oracle-pin-'))
  try {
    writeFileSync(join(root, 'package.json'), JSON.stringify({ devDependencies: { vitest: '5.0.5' } }))
    assert.match(vitestInstallError(root), /not found/)
    mkdirSync(join(root, 'node_modules', 'vitest'), { recursive: true })
    writeFileSync(join(root, 'node_modules', 'vitest', 'package.json'), JSON.stringify({ version: '5.0.4' }))
    assert.match(vitestInstallError(root), /installed Vitest is 5\.0\.4, package\.json pins 5\.0\.5/)
    writeFileSync(join(root, 'node_modules', 'vitest', 'package.json'), JSON.stringify({ version: '5.0.5' }))
    assert.equal(vitestInstallError(root), null)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})

test('a known disagreement accounts for one occurrence, not every copy', () => {
  const entry = { dir: 'd', path: 'a.test.ts', kind: 'extra-test', item: 't', reason: 'r' }
  const one = { dir: 'd', path: 'a.test.ts', kind: 'extra-test', item: 't' }
  assert.deepEqual(matchKnown([one], [entry]), { unlisted: [], stale: [] })
  assert.deepEqual(matchKnown([one, one, one], [entry]), { unlisted: [one, one], stale: [] })
  assert.deepEqual(matchKnown([], [entry, entry]), { unlisted: [], stale: [entry, entry] })
  assert.deepEqual(matchKnown([{ ...one, item: 'u' }], [entry]), { unlisted: [{ ...one, item: 'u' }], stale: [entry] })
})

test('a file two projects collect is marked overlapping rather than overwritten', () => {
  const record = (title) => ({ file: '/d/a.test.ts', project: title, tests: [{ title }], registrations: [], suiteHooks: {}, stubReads: [], collectErrors: [] })
  const files = vitestView([record('one'), record('two')], [], '/d')
  assert.equal(files.get('a.test.ts').overlapping, true)
  const result = compareDirectory(files, analyzed({ tests: { 'a.test.ts': ['one'] } }))
  assert.deepEqual(result.disagreements, [])
  assert.deepEqual(result.skipped.get('overlapping-projects').map((e) => e.path), ['a.test.ts'])
})

test('the config keeps no cache between runs, with or without a config of its own', () => {
  const dir = mkdtempSync(join(tmpdir(), 'oracle-cache-'))
  try {
    for (const source of [configSource(dir, '/tmp/run/cache'), (writeFileSync(join(dir, 'vite.config.mjs'), ''), configSource(dir, '/tmp/run/cache'))]) {
      assert.match(source, /fsModuleCache: false/)
      assert.match(source, /cacheDir: "\/tmp\/run\/cache"/)
    }
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

/** An ESTree program holding `body`. */
const program = (...body) => ({ type: 'Program', body })
const source = (value) => ({ type: 'Literal', value })
const importOf = (from, ...specifiers) => ({ type: 'ImportDeclaration', source: source(from), specifiers })
const named = (name) => ({ type: 'ImportSpecifier', imported: { type: 'Identifier', name }, local: { type: 'Identifier', name } })
const call = (expression) => ({ type: 'ExpressionStatement', expression: { type: 'CallExpression', arguments: [expression] } })

test('whole-module imports are read from the AST, wherever they sit', () => {
  const ast = program(
    importOf('./a', { type: 'ImportNamespaceSpecifier', local: { type: 'Identifier', name: 'a' } }),
    importOf('./b', named('then')),
    importOf('./named', named('x'), { type: 'ImportDefaultSpecifier' }),
    { type: 'ExportAllDeclaration', source: source('./d') },
    call({ type: 'ImportExpression', source: source('./e') }),
  )
  assert.deepEqual(wholeModuleImports(ast), new Set(['./a', './d', './e']))
  assert.equal(wholeModuleImports(program(call({ type: 'ImportExpression', source: { type: 'Identifier', name: 'n' } }))), true)
  assert.equal(wholeModuleImports(null), true)
  assert.deepEqual(wholeModuleImports(program(importOf('./named', named('x')))), new Set())
})

test('a stub taken whole reports a read when it loads; a named one only on reads', async () => {
  const { transform, resolveId, load } = plugin([])
  const importer = '/repo/fixtures/ns.test.ts'
  assert.equal(transform(program(importOf('./flags', { type: 'ImportNamespaceSpecifier' }), importOf('./named', named('x'))), importer), null)
  const whole = await resolveId('./flags', importer, {})
  const namedStub = await resolveId('./named', importer, {})
  assert.notEqual(whole, namedStub)
  const reads = []
  globalThis.__vitestDedupStubRead = (specifier, key) => reads.push(`${specifier}#${key}`)
  try {
    new Function('__vite_ssr_exports__', load(whole))(Object.create(null))
    new Function('__vite_ssr_exports__', load(namedStub))(Object.create(null))
  } finally {
    delete globalThis.__vitestDedupStubRead
  }
  assert.deepEqual(reads, ['./flags#*'])
})

test('an importer that does not parse, or was never inspected, takes every stub whole', async () => {
  const { transform, resolveId } = plugin([])
  transform('not an ast', '/repo/fixtures/broken.test.ts')
  assert.match(await resolveId('./x', '/repo/fixtures/broken.test.ts', {}), /whole:/)
  assert.match(await resolveId('./x', '/repo/fixtures/never-inspected.test.ts', {}), /whole:/)
  assert.equal(transform(program(), '/elsewhere/a.test.ts'), null)
  assert.match(await resolveId('./x', '/elsewhere/a.test.ts', {}) ?? 'not stubbed', /not stubbed/)
})

test('the config file is the one Vitest would pick: its own before Vite\'s', () => {
  const dir = mkdtempSync(join(tmpdir(), 'oracle-precedence-'))
  try {
    writeFileSync(join(dir, 'vite.config.ts'), '')
    assert.equal(ownConfig(dir), join(dir, 'vite.config.ts'))
    writeFileSync(join(dir, 'vitest.config.mjs'), '')
    assert.equal(ownConfig(dir), join(dir, 'vitest.config.mjs'))
    writeFileSync(join(dir, 'vitest.config.ts'), '')
    assert.equal(ownConfig(dir), join(dir, 'vitest.config.ts'))
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})

test('a then read reports when code under the root makes it, not when the runtime awaits', () => {
  const root = dirname(fileURLToPath(import.meta.url))
  const [resolver] = missingModuleStubs(root)
  // Named after the stub id, as Vite names the module, so the stub's own frames are recognized.
  const exports = Object.create(null)
  new Function('__vite_ssr_exports__', `${resolver.load('\0vitest-dedup-stub:named:./m')}\n//# sourceURL=vitest-dedup-stub:named:./m`)(exports)
  const reads = []
  globalThis.__vitestDedupStubRead = (specifier, key) => reads.push(`${specifier}#${key}`)
  try {
    assert.equal(exports.then, undefined)
    const elsewhere = missingModuleStubs('/somewhere/else')[0]
    const other = Object.create(null)
    new Function('__vite_ssr_exports__', `${elsewhere.load('\0vitest-dedup-stub:named:./o')}\n//# sourceURL=vitest-dedup-stub:named:./o`)(other)
    assert.equal(other.then, undefined)
  } finally {
    delete globalThis.__vitestDedupStubRead
  }
  assert.deepEqual(reads, ['./m#then'])
})

test('a Windows root matches the forward-slash ids Vite gives importers', async () => {
  const [resolver, inspector] = missingModuleStubs('C:\\repo\\fixtures')
  const context = { resolve: async () => null }
  const importer = 'C:/repo/fixtures/ns.test.ts'
  inspector.transform.call({ parse: (ast) => ast }, program(importOf('./flags', { type: 'ImportNamespaceSpecifier' })), importer)
  assert.match(await resolver.resolveId.call(context, './flags', importer, {}), /whole:\.\/flags$/)
  assert.match(await resolver.resolveId.call(context, './named', importer, {}), /named:\.\/named$/)
  assert.equal(await resolver.resolveId.call(context, './x', 'C:/elsewhere/a.test.ts', {}), null)
  assert.match(resolver.load("\0vitest-dedup-stub:named:./m"), /includes\("C:\/repo\/fixtures\/"\)/)
})

test('a sibling directory sharing the root as a prefix is outside it', async () => {
  const [resolver, inspector] = missingModuleStubs('/repo/fixtures')
  const context = { resolve: async () => null }
  inspector.transform.call({ parse: (ast) => ast }, program(importOf('./x', { type: 'ImportNamespaceSpecifier' })), '/repo/fixtures-extra/a.test.ts')
  assert.equal(await resolver.resolveId.call(context, './x', '/repo/fixtures-extra/a.test.ts', {}), null)
  assert.match(await resolver.resolveId.call(context, './x', '/repo/fixtures/a.test.ts', {}), /whole:/)
  assert.match(resolver.load('\0vitest-dedup-stub:named:./m'), /includes\("\/repo\/fixtures\/"\)/)
})
