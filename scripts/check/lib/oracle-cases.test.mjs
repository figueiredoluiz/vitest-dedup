// Unit tests for the oracle's owned cases: their shape, and how each is judged.
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { blindCaseFiles, expectedProblems, judgeCase, readCases } from './oracle-cases.mjs'

const PATH = 'a.test.ts'
const WAITS = 'exports that keep their hook rules'

/** A case whose one file meets every pin. */
const passing = (entry = {}, rest = {}) => ({
  dir: 'fixtures/oracle/x',
  expected: { files: { [PATH]: { tasks: 2, wrapperSites: ['hook-wrapper@3', 'wrapper@4'], next: {}, ...entry } }, ...rest },
})

/** What Vitest and both engines report for `passing()`. */
const observed = (over = {}) => ({
  vitest: new Map([[PATH, { tasks: [{}, {}] }]]),
  located: { compared: [PATH], disagreements: [] },
  titles: { disagreements: [] },
  sites: new Map([[PATH, [{ kind: 'wrapper', line: 4 }, { kind: 'test', line: 5 }, { kind: 'hook-wrapper', line: 3 }]]]),
  exclusions: new Map(),
  ...over,
})

/** The failure lines of judging `oracleCase` against `view`, joined. */
const failures = (oracleCase, view = observed()) => judgeCase(oracleCase, view).failures.join('\n')

test('a case that meets its pins passes and counts its file', () => {
  assert.deepEqual(judgeCase(passing(), observed()), { failures: [], held: [], files: 1 })
})

test('tasks and wrapper sites are exact', () => {
  assert.match(failures(passing({ tasks: 3 })), /Vitest collected 2 tasks, expected 3/)
  assert.match(failures(passing({ wrapperSites: ['wrapper@4'] })), /wrapperSites is \["hook-wrapper@3","wrapper@4"\], expected/)
  assert.match(failures(passing({ wrapperSites: ['wrapper@4', 'hook-wrapper@3'] })), /wrapperSites is/, 'in line order')
})

test('the next-engine ledger is exact, and no record reads as {}', () => {
  const exclusions = new Map([[PATH, { reasons: ['registered-elsewhere'], testsExcluded: 1 }]])
  assert.match(failures(passing(), observed({ exclusions })), /next is \{"reasons":\["registered-elsewhere"\],"testsExcluded":1\}, expected \{\}/)
  assert.deepEqual(judgeCase(passing({ next: exclusions.get(PATH) }), observed({ exclusions })).failures, [])
  assert.match(failures(passing({ next: { reasons: ['r'], testsExcluded: 1 } })), /next is \{\}, expected/)
})

test('a held value holds while it is today, fails when the target is met, and fails on anything else', () => {
  const held = passing({ wrapperSites: ['hook-wrapper@3'], wrapperSitesToday: ['hook-wrapper@3', 'wrapper@4'], waitsOn: WAITS })
  const result = judgeCase(held, observed())
  assert.deepEqual(result.failures, [])
  assert.match(result.held.join('\n'), /wrapperSites waits on "exports that keep their hook rules"/)

  const landed = observed({ sites: new Map([[PATH, [{ kind: 'hook-wrapper', line: 3 }]]]) })
  assert.match(failures(held, landed), /wrapperSites reached its target; "exports that keep their hook rules" landed/)

  const neither = observed({ sites: new Map([[PATH, [{ kind: 'wrapper', line: 9 }]]]) })
  assert.match(failures(held, neither), /wrapperSites is \["wrapper@9"\], neither its target nor today's/)

  const ledger = passing({ next: { reasons: ['r'], testsExcluded: 1 }, nextToday: {}, waitsOn: WAITS })
  assert.match(judgeCase(ledger, observed()).held.join('\n'), /next waits on/)
})

test('an undeclared file fails, and a declared file that was not compared fails', () => {
  const other = observed({ vitest: new Map([[PATH, { tasks: [{}, {}] }], ['b.test.ts', { tasks: [] }]]) })
  assert.match(failures(passing(), other), /b\.test\.ts: collected or analysed, but not declared/)
  const analysed = observed({ sites: new Map([...observed().sites, ['c.test.ts', []]]) })
  assert.match(failures(passing(), analysed), /c\.test\.ts: collected or analysed, but not declared/)
  const skipped = judgeCase(passing(), observed({ located: { compared: [], disagreements: [] } }))
  assert.match(skipped.failures.join('\n'), /declared, but not compared/)
  assert.equal(skipped.files, 0)
})

test('a file declared not collected must not be collected, and still pins its ledger', () => {
  const bench = { dir: 'd', expected: { files: { [PATH]: { oracle: 'not-collected', next: { reasons: ['unsupported-benchmark'], testsExcluded: 1 } } } } }
  const exclusions = new Map([[PATH, { reasons: ['unsupported-benchmark'], testsExcluded: 1 }]])
  const empty = { vitest: new Map(), located: { compared: [], disagreements: [] }, titles: { disagreements: [] }, sites: new Map() }
  assert.deepEqual(judgeCase(bench, { ...empty, exclusions }), { failures: [], held: [], files: 0 })
  assert.match(failures({ dir: 'd', expected: { files: { [PATH]: { oracle: 'not-collected', next: {} } } } }, { ...empty, exclusions: new Map() }), /declared, but the analyzer reports nothing/)
  assert.match(failures(bench, { ...empty, exclusions: new Map() }), /next is \{\}/)
  assert.match(failures(bench, { ...empty, exclusions, vitest: new Map([[PATH, { tasks: [] }]]) }), /declared not collected, and Vitest collected it/)
})

test('site and title disagreements are matched with their lists, unlisted and stale both failing', () => {
  const disagreement = { path: PATH, kind: 'missing-site', item: 'test@4' }
  const listed = { ...disagreement, reason: 'r', waitsOn: WAITS }
  const located = { compared: [PATH], disagreements: [disagreement] }
  assert.match(failures(passing(), observed({ located })), /new site disagreement: missing-site "test@4"/)
  const held = judgeCase(passing({}, { knownSites: [listed] }), observed({ located }))
  assert.deepEqual(held.failures, [])
  assert.match(held.held.join('\n'), /site missing-site waits on/)
  assert.match(failures(passing({}, { knownSites: [listed] })), /no longer occurs .*"exports that keep their hook rules" landed/)
  const titled = { ...listed, kind: 'missing-test', item: 'x > y' }
  const titles = { disagreements: [{ path: PATH, kind: 'missing-test', item: 'x > y' }] }
  assert.match(failures(passing(), observed({ titles })), /new title disagreement/)
  assert.deepEqual(judgeCase(passing({}, { knownTitles: [titled] }), observed({ titles })).failures, [])
})

test('a malformed expectation is a problem, each named', () => {
  const problems = (entry, rest = {}) => expectedProblems(passing(entry, rest).expected, 'c').join('\n')
  assert.equal(problems({}), '')
  assert.match(expectedProblems([], 'c').join(), /must be an object/)
  assert.match(expectedProblems({ files: {} }, 'c').join(), /at least one file/)
  assert.match(expectedProblems({ files: { a: 1 } }, 'c').join(), /a: must be an object/)
  assert.match(problems({}, { extra: 1 }), /unknown key "extra"/)
  assert.match(problems({ extra: 1 }), /unknown key "extra"/)
  assert.match(problems({ tasks: 0 }), /tasks must be a positive count/)
  assert.match(problems({ wrapperSites: ['test@1'] }), /wrapperSites must list/)
  assert.match(problems({ wrapperSitesToday: ['test@1'], waitsOn: WAITS }), /wrapperSitesToday must list/)
  assert.match(problems({ next: [] }), /next: must be an object/)
  assert.match(problems({ next: { reasons: [], testsExcluded: 1 } }), /reasons must be a non-empty list/)
  assert.match(problems({ next: { reasons: ['r'], testsExcluded: -1 } }), /testsExcluded must be a count/)
  assert.match(problems({ nextToday: { reasons: ['r'] }, waitsOn: WAITS }), /nextToday: testsExcluded must be a count/)
  assert.match(problems({ oracle: 'yes' }), /oracle must be "not-collected"/)
  assert.match(problems({ oracle: 'not-collected' }), /a file not collected pins no tasks or sites/)
  const today = { oracle: 'not-collected', next: {}, wrapperSitesToday: ['wrapper@1'], waitsOn: WAITS }
  assert.match(expectedProblems({ files: { [PATH]: today } }, 'c').join(), /a file not collected pins no tasks or sites/)
  assert.match(problems({}, { knownSites: {} }), /knownSites: must be a list/)
  assert.match(problems({}, { knownTitles: [1] }), /knownTitles\[0\]: must be an object/)
  assert.match(problems({}, { knownSites: [{ path: 'a', kind: 'k', item: 'i', waitsOn: WAITS }] }), /reason is required/)
  assert.match(problems({}, { blindUnderCurrent: 'a' }), /blindUnderCurrent must list paths/)
})

test('a held value needs a reason to wait in plain words, and must differ from its target', () => {
  const problems = (entry) => expectedProblems(passing(entry).expected, 'c').join('\n')
  assert.match(problems({ wrapperSitesToday: ['wrapper@1'] }), /wrapperSitesToday needs waitsOn/)
  assert.match(problems({ nextToday: { reasons: ['r'], testsExcluded: 1 } }), /nextToday needs waitsOn/)
  for (const row of ['NC41', 'nc41', 'NC-41', 'M12 lands', 'RV1']) {
    assert.match(problems({ wrapperSitesToday: ['wrapper@1'], waitsOn: row }), /needs waitsOn, in plain words/, row)
  }
  assert.equal(problems({ wrapperSitesToday: ['wrapper@1'], waitsOn: 'V8 source maps past ES6 and UTF8' }), '')
  assert.match(problems({ wrapperSitesToday: ['hook-wrapper@3', 'wrapper@4'], waitsOn: WAITS }), /equals wrapperSites; nothing is held/)
  assert.match(problems({ waitsOn: WAITS }), /waitsOn without a held value/)
  const known = { path: 'a', kind: 'k', item: 'i', reason: 'r', waitsOn: 'M12 lands' }
  assert.match(expectedProblems(passing({}, { knownTitles: [known] }).expected, 'c').join(), /waitsOn is required, in plain words/)
})

test('every directory is read, and one with no usable expected.json is a problem', () => {
  const root = mkdtempSync(join(tmpdir(), 'oracle-cases-'))
  const dir = (name) => {
    mkdirSync(join(root, 'fixtures', 'oracle', name), { recursive: true })
    return join(root, 'fixtures', 'oracle', name, 'expected.json')
  }
  try {
    assert.deepEqual(readCases(root), { cases: [], problems: [] })
    writeFileSync(dir('good'), JSON.stringify({ ...passing().expected, blindUnderCurrent: ['b.test.ts'] }))
    writeFileSync(dir('broken'), '{')
    writeFileSync(dir('malformed'), '{}')
    dir('missing')
    writeFileSync(join(root, 'fixtures', 'oracle', 'README.md'), 'a file is not a case')
    const { cases, problems } = readCases(root)
    assert.deepEqual(cases.map((c) => c.dir), ['fixtures/oracle/good'])
    assert.deepEqual(blindCaseFiles(cases), ['oracle/good/b.test.ts'])
    assert.equal(problems.length, 3)
    assert.match(problems.join('\n'), /broken: expected\.json does not parse/)
    assert.match(problems.join('\n'), /malformed: files must declare/)
    assert.match(problems.join('\n'), /missing: no expected\.json/)
  } finally {
    rmSync(root, { recursive: true, force: true })
  }
})
