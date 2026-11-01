// Unit tests for the probe's judgement and the revert proof's guarantees.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

import { analyze, disagreements, parseArguments, project, render, summarize } from './probe.mjs'
import { caught, compileStep, mutate, problems, prove } from './revert-proof.mjs'

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
  // Windows ends a child without delivering either signal, so no handler could run there.
  test(`${signal} during a check restores the file`, { skip: process.platform === 'win32' }, async () => {
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
