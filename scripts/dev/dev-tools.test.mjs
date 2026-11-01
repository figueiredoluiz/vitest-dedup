// Unit tests for the probe's judgement and the revert proof's guarantees.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { analyze, collect, disagreements, parseArguments, project, render, summarize } from './probe.mjs'
import { caught, compileStep, mutate, problems, prove } from './revert-proof.mjs'
import { vitestView } from '../check/lib/collection-oracle.mjs'

const engine = (over = {}) => ({ exit: 0, failure: null, findings: [], rules: [], tests: 2, unresolved: 0, incomplete: false, reasons: [], ...over })

test('a summary reads rules, counts and reasons from the records', () => {
  const records = [
    { type: 'finding', rule: 'duplicate-test', locations: [{ path: 'b.ts', line: 3 }, { path: 'a.ts', line: 1 }] },
    { type: 'finding', rule: 'duplicate-test' },
    { type: 'excluded-file', reason: 'registered-elsewhere' },
    { type: 'summary', testsAnalyzed: 3, unresolvedTests: 1, incomplete: true },
  ]
  assert.deepEqual(summarize(records, 1), {
    exit: 1, failure: null, findings: ['duplicate-test ', 'duplicate-test a.ts:1,b.ts:3'], rules: ['duplicate-test'], tests: 3, unresolved: 1, incomplete: true, reasons: ['registered-elsewhere'],
  })
  assert.equal(summarize([{ type: 'summary' }], 0).failure, null)
  assert.equal(summarize([{ type: 'summary' }], 0).tests, 0)
})

test('an exit with no summary record is a failure', () => {
  assert.equal(summarize([], 0).failure, 'no summary record')
  assert.equal(summarize([], 1, 'said why').failure, 'said why')
})

test('an operational failure is kept, and never reads as agreement', () => {
  assert.equal(summarize([], 2, 'first\nsecond\nthird\n').failure, 'second third')
  assert.equal(summarize([], null, 'killed').failure, 'killed')
  const failed = engine({ exit: 2, failure: 'no binary', tests: 0 })
  const row = { vitest: [], next: engine({ tests: 0 }), current: failed }
  assert.deepEqual(disagreements(row), ['default-failed'])
  assert.deepEqual(disagreements({ ...row, next: failed, current: engine({ tests: 0 }) }), ['next-failed'])
  assert.match(render({ ...row, name: 'a', sites: [] }), /^DIFF a \(default-failed\)[\s\S]*failed: no binary/)
})

test('each way the readings part is named, and agreement is silent', () => {
  const row = { vitest: ['a', 'b'], next: engine(), current: engine() }
  assert.deepEqual(disagreements(row), [])
  assert.deepEqual(disagreements({ ...row, vitest: null }), ['vitest-unread'])
  assert.deepEqual(disagreements({ ...row, next: engine({ tests: 1 }) }), ['next-count-differs'])
  assert.deepEqual(disagreements({ ...row, next: engine({ tests: 3 }) }), ['next-count-differs'])
  assert.deepEqual(disagreements({ ...row, current: engine({ tests: 3 }) }), ['default-count-differs'])
  const on = (place) => engine({ findings: [`duplicate-test ${place}`], rules: ['duplicate-test'] })
  assert.deepEqual(disagreements({ ...row, next: on('a.ts:1'), current: on('a.ts:9') }), ['findings-differ'])
  assert.deepEqual(disagreements({ ...row, next: on('a.ts:1'), current: on('a.ts:1') }), [])
  assert.deepEqual(disagreements({ ...row, next: engine({ incomplete: true }) }), ['completeness-differs'])
  assert.deepEqual(disagreements({ ...row, next: engine({ unresolved: 1 }) }), ['diagnostics-differ'])
  assert.deepEqual(disagreements({ ...row, next: engine({ reasons: ['named-callback'] }) }), ['diagnostics-differ'])
})

test('a rule the next engine does not run never reads as disagreement', () => {
  const ported = new Set(['duplicate-test'])
  const shared = 'duplicate-test a.ts:1,a.ts:2'
  const unported = 'pending-rule a.ts:1,a.ts:3'
  const row = {
    name: 'a',
    vitest: ['a', 'b'],
    sites: [],
    next: engine({ findings: [shared] }),
    current: engine({ findings: [shared, unported] }),
  }
  assert.deepEqual(disagreements(row, ported), [])
  assert.match(render(row, ported), /^SAME a\n[\s\S]*default only, rule not ported: pending-rule a\.ts:1,a\.ts:3$/)
  // A ported rule still disagrees beside it, and the row says so.
  const parted = { ...row, next: engine({ findings: ['duplicate-test a.ts:1,a.ts:9'] }) }
  assert.deepEqual(disagreements(parted, ported), ['findings-differ'])
  assert.match(render(parted, ported), /^DIFF a \(findings-differ\)/)
  assert.doesNotMatch(render({ ...row, current: engine({ findings: [shared] }) }, ported), /rule not ported/)
  // A rule next should not run is a disagreement, or a stray claim would read as agreement.
  const stray = { ...row, next: engine({ findings: [shared, unported] }), current: engine({ findings: [shared] }) }
  assert.deepEqual(disagreements(stray, ported), ['findings-differ'])
})

test('a truncated or missing oracle view never reads as agreement', () => {
  const record = { file: '/owned/a.test.ts', tests: [{ title: 'one test' }], registrations: [], suiteHooks: {}, stubReads: [], collectErrors: [] }
  const views = [
    vitestView([record, record], [], '/owned'),
    vitestView([], ['/owned/a.test.ts'], '/owned'),
  ]
  assert.equal(views[0].get('a.test.ts').overlapping, true)
  assert.equal(views[1].get('a.test.ts').unprobed, true)
  for (const view of views) {
    const result = collect('/owned', () => view)
    assert.equal(result.vitest, null)
    assert.match(render({ name: 'partial', ...result, sites: [], next: engine({ tests: 1 }), current: engine({ tests: 1 }) }), /^DIFF partial \(vitest-unread\)/)
  }
  const single = collect('/owned', () => vitestView([record], [], '/owned'))
  assert.deepEqual(single, { vitest: ['one test'] })
  assert.match(render({ name: 'single', ...single, sites: [], next: engine({ tests: 1 }), current: engine({ tests: 1 }) }), /^SAME single/)
  assert.deepEqual(collect('/owned', () => new Map()), { vitest: [] })
  for (const field of ['collectErrors', 'stubReads']) {
    assert.equal(collect('/owned', () => vitestView([{ ...record, [field]: ['untrusted'] }], [], '/owned')).vitest, null)
  }
  assert.match(collect('/owned', () => { throw new Error('collection failed') }).error, /collection failed/)
})

test('real overlapping Vitest projects stay unread while a single project stays comparable', { timeout: 30_000 }, () => {
  const work = mkdtempSync(join(tmpdir(), 'probe-projects-'))
  const root = join(dirname(fileURLToPath(import.meta.url)), '../..')
  try {
    symlinkSync(join(root, 'node_modules'), join(work, 'node_modules'), process.platform === 'win32' ? 'junction' : 'dir')
    writeFileSync(join(work, 'a.test.ts'), "import { test, expect } from 'vitest';\ntest('one test', () => { expect(1).toBe(1); });\n")
    const config = join(work, 'vitest.config.mjs')
    writeFileSync(config, "export default { test: { projects: [{test:{name:'first',include:['**/*.test.ts']}},{test:{name:'second',include:['**/*.test.ts']}}] } };\n")
    assert.equal(collect(work).vitest, null)
    writeFileSync(config, "export default { test: { projects: [{test:{name:'first',include:['**/*.test.ts']}}] } };\n")
    assert.deepEqual(collect(work), { vitest: ['one test'] })
  } finally { rmSync(work, { recursive: true, force: true }) }
})

test('a run that cannot start or prints no JSONL fails its own row, and does not throw', () => {
  const work = mkdtempSync(join(tmpdir(), 'probe-test-'))
  try {
    const missing = analyze(join(work, 'absent'), work, null)
    assert.equal(missing.exit, null)
    assert.match(missing.failure, /ENOENT/)
    const noise = join(work, 'noise')
    writeFileSync(noise, '#!/bin/sh\necho not json\n', { mode: 0o755 })
    assert.notEqual(analyze(noise, work, 'next').failure, null)
    const clean = join(work, 'clean')
    writeFileSync(clean, '#!/bin/sh\necho \'{"type":"summary","testsAnalyzed":1}\'\n', { mode: 0o755 })
    assert.equal(analyze(clean, work, 'next').failure, null)
  } finally {
    rmSync(work, { recursive: true, force: true })
  }
})

test('a row shows all three readings and leads with its verdict', () => {
  const same = render({ name: 'a', vitest: ['t'], sites: ['test@2'], next: engine({ tests: 1 }), current: engine({ tests: 1 }) })
  assert.match(same, /^SAME a\n/)
  assert.match(same, /vitest: \["t"\]/)
  assert.match(same, /sites: test@2/)
  const diff = render({
    name: 'b', vitest: null, error: 'broke', sites: [],
    next: engine({ incomplete: true, reasons: ['named-callback'] }), current: engine(),
  })
  assert.match(diff, /^DIFF b \(vitest-unread, completeness-differs, diagnostics-differ\)/)
  assert.match(diff, /unread: broke/)
  assert.match(diff, /sites: none/)
  assert.match(diff, /next: exit 0, 2 analyzed, 0 unresolved, incomplete; rules \[\]; excluded \[named-callback\]/)
})

test('a case copies a file or a directory, and arguments are read strictly', () => {
  const work = mkdtempSync(join(tmpdir(), 'probe-test-'))
  try {
    writeFileSync(join(work, 'one.test.ts'), 'x')
    mkdirSync(join(work, 'two'))
    writeFileSync(join(work, 'two', 'a.test.ts'), 'y')
    const out = join(work, 'out')
    mkdirSync(out)
    assert.deepEqual(readdirSync(project(join(work, 'one.test.ts'), out)), ['one.test.ts'])
    assert.deepEqual(readdirSync(project(join(work, 'two'), out)), ['a.test.ts'])
    mkdirSync(join(work, 'three'))
    writeFileSync(join(work, 'three', 'one.test.ts'), 'z')
    const first = project(join(work, 'one.test.ts'), out)
    assert.notEqual(project(join(work, 'three', 'one.test.ts'), out), first)
  } finally {
    rmSync(work, { recursive: true, force: true })
  }
  assert.deepEqual(parseArguments(['a.test.ts']).sources, ['a.test.ts'])
  assert.match(parseArguments(['--binary', '/x/bin', 'a']).binary, /\/x\/bin$/)
  assert.throws(() => parseArguments([]), /usage/)
  assert.throws(() => parseArguments(['--binary']), /needs a path/)
})

/** A spec over one file, `a.rs`, holding `good`, checked by a `unit` check. */
const spec = (mutant = {}) => ({
  build: ['build'],
  checks: [{ name: 'unit', command: ['unit'] }],
  mutants: [{ name: 'm', file: 'a.rs', find: 'good', replace: 'bad', ...mutant }],
})

/** Adapters over an in-memory file; `unit` passes while it holds `good`. */
function memory(start = 'fn good') {
  const files = { 'a.rs': start }
  const writes = []
  return {
    files,
    writes,
    io: {
      read: (file) => files[file],
      write: (file, text) => { writes.push(text); files[file] = text },
      run: async ([command]) => command === 'build' || files['a.rs'].includes('good'),
      print: () => {},
    },
  }
}

test('a mutant the checks fail on is caught, and the file comes back', async () => {
  const { files, writes, io } = memory()
  assert.deepEqual(await prove(spec(), io), [{ name: 'm', verdict: 'caught', failed: ['unit'] }])
  assert.equal(files['a.rs'], 'fn good')
  assert.deepEqual(writes, ['fn bad', 'fn good'])
})

test('a mutant that does not compile proves nothing, even when its checks would fail', async () => {
  const { files, io } = memory()
  const run = async () => files['a.rs'].includes('good')
  assert.deepEqual(await prove(spec(), { ...io, run }), [{ name: 'm', verdict: 'not-built', failed: [] }])
  assert.equal(files['a.rs'], 'fn good')
})

test('a cargo test check compiles under its own flags before it runs', async () => {
  const check = ['cargo', 'test', '--lib', '--features', 'foo', 'filter']
  assert.deepEqual(compileStep(check), ['cargo', 'test', '--no-run', '--lib', '--features', 'foo', 'filter'])
  assert.equal(compileStep(['node', '--test']), null)
  assert.equal(compileStep(['cargo', 'build']), null)
  const { files, io } = memory()
  const ran = []
  const rust = { ...spec(), checks: [{ name: 'unit', command: check }] }
  // The library builds under the mutant, and only this check's own compile fails.
  const run = async (command) => {
    ran.push(command.join(' '))
    if (command.includes('--no-run')) return files['a.rs'].includes('good')
    return true
  }
  assert.deepEqual(await prove(rust, { ...io, run }), [{ name: 'm', verdict: 'not-built', failed: [] }])
  assert.equal(ran.filter((command) => command === check.join(' ')).length, 1, 'the mutant never ran the check')
})

test('Cargo toolchains, executable paths and global options keep compilation separate', async () => {
  for (const prefix of [
    ['cargo', '+stable'], ['/usr/bin/cargo', '+nightly'], ['C:\\tools\\cargo.exe', '+stable'],
    ['cargo', '--color', 'always', '--config', 'test'], ['cargo', '-vv', '--locked', '--offline', '--frozen', '-q'],
    ['cargo', '+nightly', '-Z', 'unstable-options', '-C', '/tmp'],
    ['cargo', '--color=never', '--config=net.offline=true', '-Zunstable-options', '-C/tmp'],
  ]) {
    const check = [...prefix, 'test', '--lib', '--', '--nocapture']
    assert.deepEqual(compileStep(check), [...prefix, 'test', '--no-run', '--lib', '--', '--nocapture'])
    const { files, io } = memory()
    const ran = []
    const run = async (args) => {
      ran.push(args)
      return !args.includes('--no-run') || files['a.rs'].includes('good')
    }
    const result = await prove({ ...spec(), checks: [{ name: 'unit', command: check }] }, { ...io, run })
    assert.equal(result[0].verdict, 'not-built')
    assert.equal(ran.filter((args) => args === check).length, 1, 'only the baseline runs the test')
  }
  assert.deepEqual(compileStep(['cargo', 't', '--lib']), ['cargo', 't', '--no-run', '--lib'])
  assert.equal(compileStep(['cargo', '+stable', 'build']), null)
  assert.throws(() => compileStep(['cargo', '--unknown', 'test']), /cannot determine Cargo command/)
})

test('interruption stops a signal-ignoring grandchild before restoring the source', { skip: process.platform !== 'darwin', timeout: 15_000 }, async () => {
  const work = mkdtempSync(join(tmpdir(), 'proof-descendants-'))
  const target = join(work, 'source.txt')
  const ready = join(work, 'ready')
  const writer = join(work, 'writer.mjs')
  const check = join(work, 'check.mjs')
  writeFileSync(target, 'good')
  writeFileSync(writer, `import {writeFileSync} from 'node:fs'; process.on('SIGTERM',()=>{}); writeFileSync(process.argv[3], String(process.pid)); setInterval(()=>writeFileSync(process.argv[2], 'bad'), 50);`)
  writeFileSync(check, `import {readFileSync} from 'node:fs'; import {spawn} from 'node:child_process'; if(readFileSync(process.argv[2],'utf8')==='bad'){spawn(process.execPath,[process.argv[3],process.argv[2],process.argv[4]],{stdio:'ignore'});setInterval(()=>{},1000)}`)
  const path = join(work, 'spec.json')
  writeFileSync(path, JSON.stringify({ build: [process.execPath, '--check', check], checks: [{ name: 'child', command: [process.execPath, check, target, writer, ready] }], mutants: [{ name: 'm', file: target, find: 'good', replace: 'bad' }] }))
  const child = spawn(process.execPath, [join(dirname(fileURLToPath(import.meta.url)), 'revert-proof.mjs'), path], { stdio: 'ignore' })
  let descendant
  let failure = null
  const exited = new Promise((resolve) => child.once('close', (code) => resolve(code)))
  try {
    const deadline = Date.now() + 5_000
    while (Date.now() < deadline && !readdirSync(work).includes('ready')) await new Promise((resolve) => setTimeout(resolve, 20))
    descendant = Number(readFileSync(ready, 'utf8'))
    child.kill('SIGTERM')
    assert.equal(await exited, 143)
    assert.equal(readFileSync(target, 'utf8'), 'good')
    await new Promise((resolve) => setTimeout(resolve, 250))
    assert.equal(readFileSync(target, 'utf8'), 'good', 'no descendant overwrites the restored source')
  } catch (error) {
    failure = error
  } finally {
    child.kill('SIGKILL')
    if (descendant) { try { process.kill(descendant, 'SIGKILL') } catch (error) { if (error.code !== 'ESRCH') failure ??= error } }
    await exited
    rmSync(work, { recursive: true, force: true })
  }
  if (failure !== null) throw failure
})

test('a restored tree that does not build fails the run', async () => {
  const { io } = memory()
  let builds = 0
  const run = async (command) => (command[0] === 'build' ? ++builds < 3 : io.run(command))
  await assert.rejects(() => prove(spec(), { ...io, run }), /restored tree does not build/)
})

test('a mutant nothing fails on survives', async () => {
  const { io } = memory()
  assert.deepEqual(await prove(spec(), { ...io, run: async () => true }), [{ name: 'm', verdict: 'survived', failed: [] }])
})

test('an edit that does not apply stops the run before anything is written', async () => {
  for (const start of ['fn other', 'good good']) {
    const { files, writes, io } = memory(start)
    await assert.rejects(() => prove(spec(), { ...io, run: async () => true }), /find matches/)
    assert.equal(files['a.rs'], start)
    assert.deepEqual(writes, [])
  }
})

test('the untouched tree must pass and build first', async () => {
  const { writes, io } = memory()
  await assert.rejects(() => prove(spec(), { ...io, run: async ([command]) => command === 'build' }), /fail on the untouched tree: unit/)
  await assert.rejects(() => prove(spec(), { ...io, run: async ([command]) => command !== 'build' }), /does not build/)
  assert.deepEqual(writes, [])
})

test('the file is restored when a check throws under the mutant', async () => {
  const { files, writes, io } = memory()
  let calls = 0
  const run = async () => {
    if (++calls === 4) throw new Error('killed')
    return true
  }
  await assert.rejects(() => prove(spec(), { ...io, run }), /killed/)
  assert.deepEqual(writes, ['fn bad', 'fn good'])
  assert.equal(files['a.rs'], 'fn good')
})

for (const [signal, code] of [['SIGINT', 130], ['SIGTERM', 143]]) {
  // The subprocess proof runner is supported only on macOS.
  test(`${signal} during a check restores the file`, { skip: process.platform !== 'darwin' }, async () => {
    const work = mkdtempSync(join(tmpdir(), 'revert-proof-test-'))
    try {
      const target = join(work, 'a.txt')
      writeFileSync(target, 'good')
      const hang = `if (require('fs').readFileSync(${JSON.stringify(target)}, 'utf8').includes('bad')) setTimeout(() => {}, 60000)`
      const spec = join(work, 'spec.json')
      writeFileSync(spec, JSON.stringify({
        build: ['node', '-e', ''],
        checks: [{ name: 'hangs', command: ['node', '-e', hang] }],
        mutants: [{ name: 'm', file: target, find: 'good', replace: 'bad' }],
      }))
      const tool = join(dirname(fileURLToPath(import.meta.url)), 'revert-proof.mjs')
      const child = spawn(process.execPath, [tool, spec], { stdio: 'ignore' })
      let ended = false
      const exited = new Promise((done) =>
        child.once('close', (status, killed) => {
          ended = true
          done({ status, killed })
        }),
      )
      const deadline = Date.now() + 30_000
      while (!ended && Date.now() < deadline && readFileSync(target, 'utf8') !== 'bad') {
        await new Promise((done) => setTimeout(done, 20))
      }
      assert.equal(readFileSync(target, 'utf8'), 'bad', 'the run reached its mutant')
      await new Promise((done) => setTimeout(done, 300))
      child.kill(signal)
      assert.deepEqual(await exited, { status: code, killed: null })
      assert.equal(readFileSync(target, 'utf8'), 'good')
    } finally {
      rmSync(work, { recursive: true, force: true })
    }
  })
}

test('a file left changed fails the run', async () => {
  const { io } = memory()
  const write = (file, text) => { if (text !== 'fn good') io.write(file, text); else io.write(file, 'fn changed good') }
  await assert.rejects(() => prove(spec(), { ...io, write }), /a\.rs was not restored/)
})

test('an expected check must be among those that failed', () => {
  assert.equal(caught({ expect: ['unit', 'oracle'] }, ['unit']), false)
  assert.equal(caught({ expect: ['unit'] }, ['unit', 'oracle']), true)
  assert.equal(caught({}, ['unit']), true)
  assert.equal(caught({}, []), false)
})

test('a spec that could prove nothing is refused', async () => {
  assert.deepEqual(problems(spec()), [])
  assert.match(problems({}).join(), /checks must list/)
  assert.match(problems({ checks: [{ name: 'a', command: ['a'] }] }).join(), /mutants must list/)
  assert.match(problems({ ...spec(), build: [] }).join(), /build must be a command/)
  assert.match(problems({ ...spec(), build: undefined }).join(), /build must be a command/)
  assert.match(problems(spec({ replace: 'good' })).join(), /nothing changes/)
  assert.match(problems(spec({ replace: 1 })).join(), /replace is required/)
  assert.match(problems(spec({ find: '' })).join(), /find is required/)
  assert.match(problems(spec({ expect: ['nope'] })).join(), /unknown check nope/)
  const twice = { ...spec(), checks: [{ name: 'a', command: ['a'] }, { name: 'a', command: ['b'] }] }
  assert.match(problems(twice).join(), /named twice/)
  assert.match(problems({ ...spec(), checks: [{ name: 'a' }] }).join(), /name and a command/)
  await assert.rejects(() => prove({}, memory().io), /checks must list/)
})

test('mutate replaces literally, so a `$` in the replacement stays', () => {
  assert.equal(mutate('a good b', { name: 'm', find: 'good', replace: "$&'" }), "a $&' b")
})
