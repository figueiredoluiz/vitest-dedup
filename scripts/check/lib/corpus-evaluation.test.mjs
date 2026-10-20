// Tests for the corpus evaluator's measurement. Every classification is driven with a fabricated
// observation, and each status has a neighbour one edit away so a matcher that always says
// "satisfied" (or never does) fails here rather than reading as a clean corpus.
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { buildRecallLabels } from './corpus-labels.mjs'
import {
  buildDistribution,
  buildReport,
  buildRuleTable,
  classifyEligibility,
  configuredRules,
  DEFAULT_OFF_RULES,
  evaluateCase,
  measurementProblems,
  disabledRules,
  ratio,
  renderText,
} from './corpus-evaluation.mjs'

const meta = { origin: 'synthetic', verdict: 'generated', classifiedAt: '2026-10-20' }
const reason = 'r'.repeat(40)

/** A jsonl finding record. */
function finding(overrides = {}) {
  return {
    type: 'finding',
    rule: 'duplicate-test',
    severity: 'error',
    baselined: false,
    titles: ['a', 'b'],
    locations: [
      { path: 'a.test.ts', line: 3 },
      { path: 'b.test.ts', line: 3 },
    ],
    ...overrides,
  }
}

/** A jsonl `test` record. */
function testRecord(overrides = {}) {
  return {
    type: 'test',
    path: 'a.test.ts',
    line: 3,
    title: 'a',
    comparable: true,
    trivial: false,
    alpha: 'x',
    structural: 'x',
    behavior: 'x',
    contextKey: 'x',
    modifierKey: 'x',
    ...overrides,
  }
}

/** A manifest case, satisfied by {@link observation}. */
function makeCase(overrides = {}) {
  return {
    expectedTests: 2,
    expectedIncomplete: false,
    ...meta,
    name: 'c',
    path: 'c',
    role: 'positive',
    rule: 'duplicate-test',
    expectedFindings: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] }],
    expectedAbsent: [{ rule: 'duplicate-test-name', titles: ['a', 'b'] }],
    ...overrides,
  }
}

/** The records of a two-test run, with `middle` records between the tests and the summary. */
function stream(middle = [], second = {}) {
  return [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b', ...second }), ...middle, { type: 'summary', testsAnalyzed: 2, incomplete: false }]
}

/** The observation of {@link makeCase}: two tests, one finding. */
function observation(overrides = {}) {
  const records = overrides.records ?? stream([finding()], { alpha: 'y' })
  const findings = records.filter((record) => record?.type === 'finding').length
  return {
    label: 'c (c)',
    directoryPresent: true,
    sourceFiles: ['a.test.ts'],
    summary: { type: 'summary', testsAnalyzed: 2, incomplete: false, unresolvedTests: 0, recordCount: findings },
    ...overrides,
    records,
  }
}

/** Evaluates `testCase` against `obs`. */
function evaluate(testCase, obs = observation()) {
  const labels = buildRecallLabels({ schemaVersion: 1, cases: [testCase] })
  return evaluateCase(labels.cases[0], labels.expectations, obs)
}

/** The status of the expectation with id `id` in an evaluated case. */
function statusOf(evaluated, id) {
  return evaluated.expectations.find((outcome) => outcome.id === id).status
}

test('a zero denominator is no data, never a percentage', () => {
  assert.equal(ratio(0, 0), null)
  assert.equal(ratio(0, 4), 0)
  assert.equal(ratio(3, 4), 0.75)
})

test('an expected finding is satisfied by exactly one match, missing by none, ambiguous by two', () => {
  const testCase = makeCase()
  assert.equal(statusOf(evaluate(testCase), 'c#1'), 'satisfied')
  const none = observation({
    records: stream(),
  })
  assert.equal(statusOf(evaluate(testCase, none), 'c#1'), 'missing')
  const twice = observation({
    records: stream([finding(), finding()]),
  })
  assert.equal(statusOf(evaluate(testCase, twice), 'c#1'), 'ambiguous')
  const otherSeverity = observation({
    records: stream([finding({ severity: 'warning' })]),
  })
  assert.equal(statusOf(evaluate(testCase, otherSeverity), 'c#1'), 'missing', 'a matcher that ignored severity would satisfy this')
})

test('a baselined finding does not count', () => {
  const obs = observation({
    records: stream([finding({ baselined: true })]),
  })
  assert.equal(statusOf(evaluate(makeCase(), obs), 'c#1'), 'missing')
})

test('an expected absence is held until a finding of that shape appears', () => {
  const testCase = makeCase()
  assert.equal(statusOf(evaluate(testCase), 'c#2'), 'held')
  const present = observation({
    records: stream([finding(), finding({ rule: 'duplicate-test-name', severity: 'warning' })]),
  })
  const evaluated = evaluate(testCase, present)
  assert.equal(statusOf(evaluated, 'c#2'), 'violated')
  assert.equal(evaluated.unclaimed.length, 1, 'the forbidden finding is also unclaimed')
})

test('known misses and known false positives keep or lose their label', () => {
  const miss = { id: 'm', reason, rule: 'near-duplicate-test', severity: 'warning', titles: ['a', 'b'] }
  const falsePositive = { id: 'f', reason, rule: 'duplicate-test-name', severity: 'warning', titles: ['a', 'b'], expectCount: 2 }
  const testCase = makeCase({ knownMisses: [miss], knownFalsePositives: [falsePositive] })
  const records = (extra) => stream([finding(), ...extra])
  const fp = () => finding({ rule: 'duplicate-test-name', severity: 'warning' })

  const kept = evaluate(testCase, observation({ records: records([fp(), fp()]) }))
  assert.equal(statusOf(kept, 'c#3'), 'still-missed')
  assert.equal(statusOf(kept, 'c#4'), 'still-reported')
  assert.equal(kept.unclaimed.length, 0, 'findings a known false positive owns are classified')
  assert.equal(statusOf(kept, 'c#2'), 'held', 'the absence excuses what the false positive owns')

  const fixed = evaluate(testCase, observation({ records: records([]) }))
  assert.equal(statusOf(fixed, 'c#4'), 'no-longer-reported')

  const wrongCount = evaluate(testCase, observation({ records: records([fp()]) }))
  assert.equal(statusOf(wrongCount, 'c#4'), 'count-mismatch')
  assert.equal(wrongCount.unclaimed.length, 1, 'a miscounted false positive owns nothing')

  const nowReported = evaluate(testCase, observation({ records: records([finding({ rule: 'near-duplicate-test', severity: 'warning' }), fp(), fp()]) }))
  assert.equal(statusOf(nowReported, 'c#3'), 'now-reported')
  assert.equal(nowReported.unclaimed.length, 0, 'the fixed miss is one violation, not two')
})

test('extraction facts compare against the case labels', () => {
  assert.equal(evaluate(makeCase()).extraction.matches, true)
  assert.equal(evaluate(makeCase({ expectedTests: 3 })).extraction.matches, false)
  assert.equal(evaluate(makeCase({ expectedIncomplete: true })).extraction.matches, false)
  assert.equal(evaluate(makeCase({ expectedUnresolved: 1 })).extraction.matches, false)
  assert.equal(evaluate(makeCase({ expectedUnresolved: 0 })).extraction.matches, true)
})

test('eligibility distinguishes compared, not compared and unresolved', () => {
  const byTitle = { titles: ['a', 'b'], locations: null }
  const records = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b', alpha: 'y', modifierKey: 'z' })]
  assert.deepEqual(classifyEligibility(byTitle, records), { eligibility: 'eligible', differs: ['alpha', 'modifierKey'] })

  const same = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b' })]
  assert.deepEqual(classifyEligibility(byTitle, same), { eligibility: 'eligible', differs: [] })

  for (const patch of [{ comparable: false }, { trivial: true }]) {
    const set = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b', ...patch })]
    assert.equal(classifyEligibility(byTitle, set).eligibility, 'not-compared', JSON.stringify(patch))
  }

  assert.equal(classifyEligibility(byTitle, [testRecord()]).eligibility, 'unresolved', 'one participant missing')
  const twin = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'a' }), testRecord({ path: 'c.test.ts', title: 'b' })]
  assert.equal(classifyEligibility(byTitle, twin).eligibility, 'unresolved', 'a title matching two records is ambiguous')
  assert.equal(
    classifyEligibility({ titles: ['a', 'suite > a'], locations: null }, [testRecord()]).eligibility,
    'unresolved',
    'two names answered by one record are not two participants',
  )
})

test('a trivial body takes a test out of comparison except for the rule that compares titles', () => {
  const records = [testRecord({ trivial: true }), testRecord({ path: 'a.test.ts', line: 7, title: 'b' })]
  const claim = (rule) => ({ rule, titles: ['a', 'b'], locations: null })
  assert.equal(classifyEligibility(claim('duplicate-test'), records).eligibility, 'not-compared')
  assert.equal(classifyEligibility(claim('duplicate-test-name'), records).eligibility, 'eligible')
})

test('eligibility resolves by location, by repeated title and by suite-qualified title', () => {
  const records = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b', alpha: 'y' })]
  const byLocation = { titles: null, locations: ['a.test.ts:3', 'a.test.ts:7'] }
  assert.equal(classifyEligibility(byLocation, records).eligibility, 'eligible')
  assert.equal(classifyEligibility({ titles: null, locations: ['a.test.ts:3', 'z.test.ts:9'] }, records).eligibility, 'unresolved')

  const twins = [testRecord({ title: 'dup' }), testRecord({ path: 'a.test.ts', line: 7, title: 'dup' })]
  assert.equal(classifyEligibility({ titles: ['dup', 'dup'], locations: null }, twins).eligibility, 'eligible')
  assert.equal(classifyEligibility({ titles: ['dup', 'dup'], locations: null }, [twins[0]]).eligibility, 'unresolved')

  // Record titles already carry the suite path, so only the full title names a test.
  const nested = [testRecord({ title: 'suite > a' }), testRecord({ path: 'a.test.ts', line: 7, title: 'suite > b' })]
  assert.equal(classifyEligibility({ titles: ['suite > a', 'suite > b'], locations: null }, nested).eligibility, 'eligible')
  assert.equal(classifyEligibility({ titles: ['suite > a', 'suite > b'], locations: null }, records).eligibility, 'unresolved')
  const outer = [...records, testRecord({ path: 'c.test.ts', title: 'a' })]
  assert.equal(classifyEligibility({ titles: ['a', 'b'], locations: null }, outer).eligibility, 'unresolved', 'a second test titled a is ambiguous')
})

test('a hook record carries no comparable flag and is still resolvable', () => {
  const hooks = [
    { type: 'hook', kind: 'beforeEach', path: 'a.test.ts', line: 2, trivial: false, alpha: 'x', structural: 'x', behavior: 'x' },
    { type: 'hook', kind: 'beforeEach', path: 'a.test.ts', line: 6, trivial: false, alpha: 'x', structural: 'x', behavior: 'x' },
  ]
  const claim = { rule: 'redundant-hook', titles: ['beforeEach', 'beforeEach'], locations: null }
  assert.equal(classifyEligibility(claim, hooks).eligibility, 'eligible')
  assert.equal(classifyEligibility({ ...claim, rule: 'duplicate-test' }, hooks).eligibility, 'unresolved', 'a hook is not a test')
  const unjudged = [...hooks, testRecord({ path: 'a.test.ts', comparable: false })]
  assert.equal(classifyEligibility(claim, unjudged).eligibility, 'unresolved', 'which tests a hook governs is not in the stream')
  const elsewhere = [...hooks, testRecord({ path: 'z.test.ts', comparable: false })]
  assert.equal(classifyEligibility(claim, elsewhere).eligibility, 'unresolved', 'an imported runner\'s hooks belong to its callers')
  const sound = [...hooks, testRecord({ path: 'z.test.ts' })]
  assert.equal(classifyEligibility(claim, sound).eligibility, 'eligible')
})

test('a record without comparison keys is refused rather than read as equal keys', () => {
  const noKeys = (record) => observation({ records: [record, testRecord({ path: 'a.test.ts', line: 7, title: 'b' }), { type: 'summary', testsAnalyzed: 2 }] })
  assert.throws(() => evaluate(makeCase(), noKeys(testRecord({ alpha: undefined, behavior: '' }))), /lacks `alpha`, `behavior`/)
  assert.throws(() => evaluate(makeCase(), noKeys(testRecord({ modifierKey: undefined }))), /lacks `modifierKey`/)
  const hook = { type: 'hook', kind: 'beforeEach', path: 'a.test.ts', line: 2, trivial: false, alpha: 'x', structural: 'x' }
  assert.throws(
    () => evaluate(makeCase(), observation({ records: [hook, { type: 'summary', testsAnalyzed: 0 }] })),
    /hook record lacks `behavior`/,
  )
})

test('a claim naming one participant is not a pair and stays unresolved', () => {
  assert.equal(classifyEligibility({ rule: 'duplicate-test', titles: ['a'], locations: null }, [testRecord()]).eligibility, 'unresolved')
  assert.equal(classifyEligibility({ rule: 'duplicate-test', titles: null, locations: ['a.test.ts:3'] }, [testRecord()]).eligibility, 'unresolved')
})

test('a pair with equal keys is counted apart from one whose keys differ', () => {
  const near = { expectedAbsent: [{ rule: 'near-duplicate-test', titles: ['a', 'b'] }] }
  const cases = [makeCase({ name: 'same', ...near }), makeCase({ name: 'apart', ...near })]
  const { labels, evaluated } = evaluateAll(cases, [observation({ records: stream([finding()]) }), observation()])
  const row = buildRuleTable(evaluated).find((r) => r.rule === 'near-duplicate-test')
  assert.equal(row.expectedAbsent.eligible, 2)
  assert.equal(row.expectedAbsent.eligibleEqualKeys, 1)
  assert.match(renderText(buildReport({ binary: 'b', root: 'r', labels, cases: evaluated })).join('\n'), /eligible 2 \(1 with equal keys\)/)
})

test('the title-only rule is judged by titles, not by body fingerprints', () => {
  const claim = (titles) => ({ rule: 'duplicate-test-name', titles, locations: null })
  const records = [testRecord({ title: 'a' }), testRecord({ line: 7, title: 'b' })]
  assert.deepEqual(classifyEligibility(claim(['a', 'b']), records), { eligibility: 'eligible', differs: ['title'] })
  const same = [testRecord({ title: 'a' }), testRecord({ line: 7, title: 'a', alpha: 'y' })]
  assert.deepEqual(classifyEligibility(claim(['a', 'a']), same), { eligibility: 'eligible', differs: [] })
})

test('table rows are keyed by title as well as body', () => {
  const claim = { rule: 'duplicate-each-row', titles: ['row 1', 'row 2'], locations: null }
  const rows = [testRecord({ title: 'row 1' }), testRecord({ line: 7, title: 'row 2' })]
  assert.deepEqual(classifyEligibility(claim, rows), { eligibility: 'eligible', differs: ['title'] })
  const same = [testRecord({ title: 'row 1' }), testRecord({ line: 7, title: 'row 1' })]
  assert.deepEqual(classifyEligibility({ ...claim, titles: ['row 1', 'row 1'] }, same), { eligibility: 'eligible', differs: [] })
})

test('the manifest baselines for known entries are checked against the labelled populations', () => {
  const reason = 'r'.repeat(40)
  const miss = { id: 'm', reason, rule: 'near-duplicate-test', severity: 'warning', titles: ['a', 'b'] }
  const withMiss = makeCase({ knownMisses: [miss] })
  const base = { schemaVersion: 1, knownMissBaseline: 1, knownFalsePositiveBaseline: 0 }
  const labels = buildRecallLabels({ ...base, cases: [withMiss] })
  const { evaluated } = evaluateAll([withMiss], [observation()])
  assert.deepEqual(measurementProblems(labels, evaluated), [])
  const dropped = buildRecallLabels({ ...base, cases: [makeCase()] })
  assert.match(measurementProblems(dropped, evaluateAll([makeCase()], [observation()]).evaluated).join('\n'), /0 known misses labelled against a declared baseline of 1/)
  const extra = { ...labels, knownFalsePositiveBaseline: 1 }
  assert.match(measurementProblems(extra, evaluated).join('\n'), /0 known false positives labelled against a declared baseline of 1/)
  for (const bad of ['1', -1, 1.5]) {
    assert.throws(() => buildRecallLabels({ schemaVersion: 1, knownMissBaseline: bad, cases: [makeCase()] }), /`knownMissBaseline` must be a non-negative integer/)
  }
})

test('arguments that switch a rule off are read in both spellings, the last setting winning', () => {
  assert.deepEqual([...disabledRules(['--rule', 'near-duplicate-test=off'])].sort(), ['near-duplicate-test', 'subsumed-test'])
  assert.deepEqual([...disabledRules(['--rule=duplicate-test=off', '--min-lines', '3'])].sort(), ['duplicate-test', 'subsumed-test'])
  assert.deepEqual([...disabledRules(['--rule', 'duplicate-test=off', '--rule', 'duplicate-test=warning'])], ['subsumed-test'])
  assert.deepEqual([...disabledRules(['--rule', 'duplicate-test=warning', '--rule'])], ['subsumed-test'])
  assert.deepEqual([...disabledRules([])], DEFAULT_OFF_RULES, 'a rule that defaults to off is off until a case enables it')
  assert.deepEqual([...disabledRules(['--rule', 'subsumed-test=warning'])], [])
})

test('the default-off rules named here are exactly those src/model.rs ships off', () => {
  const model = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'src', 'model.rs'), 'utf8')
  const shipped = [...model.matchAll(/Self::(\w+) => Severity::Off,/g)].map(([, name]) =>
    name.replace(/([a-z])([A-Z])/g, '$1-$2').toLowerCase(),
  )
  assert.deepEqual([...DEFAULT_OFF_RULES].sort(), shipped.sort())
})

test('rule severities come from the nearest configuration file or --config, and the command line wins', () => {
  const root = mkdtempSync(join(tmpdir(), 'corpus-config-'))
  mkdirSync(join(root, 'a', 'b'), { recursive: true })
  writeFileSync(join(root, '.vitest-dedup.json'), JSON.stringify({ rules: { 'subsumed-test': 'warning', 'duplicate-test': 'off' } }))
  writeFileSync(join(root, 'a', 'own.json'), JSON.stringify({ rules: { 'near-duplicate-test': 'off' } }))
  writeFileSync(join(root, 'a', 'bad.json'), '{ not json')
  const below = join(root, 'a', 'b')
  assert.deepEqual(configuredRules(below, []), { 'subsumed-test': 'warning', 'duplicate-test': 'off' }, 'found walking up')
  assert.deepEqual(configuredRules(below, ['--config', join(root, 'a', 'own.json')]), { 'near-duplicate-test': 'off' })
  assert.deepEqual(configuredRules(below, [`--config=${join(root, 'a', 'own.json')}`]), { 'near-duplicate-test': 'off' })
  assert.deepEqual(configuredRules(below, ['--config', join(root, 'a', 'bad.json')]), {}, 'an unreadable file configures nothing')
  assert.deepEqual(configuredRules(below, ['--config']), {})
  const own = join(root, 'a', 'own.json')
  for (const spelling of [['-c', own], [`-c${own}`], [`-c=${own}`]]) {
    assert.deepEqual(configuredRules(below, spelling), { 'near-duplicate-test': 'off' }, spelling.join(' '))
  }
  assert.deepEqual(configuredRules(join(root, 'a'), ['--config', 'own.json']), { 'near-duplicate-test': 'off' }, 'relative to the analyzed directory')
  assert.deepEqual(configuredRules(below, ['--config', 'own.json']), {}, 'not to the working directory or an ancestor')
  assert.deepEqual(configuredRules(undefined, []), {})
  writeFileSync(join(root, 'a', '.vitest-dedup.json'), JSON.stringify({ rules: 'nope' }))
  assert.deepEqual(configuredRules(below, []), {}, 'the nearest file wins, even when its rules are malformed')

  const configured = { 'subsumed-test': 'warning', 'duplicate-test': 'off' }
  assert.deepEqual([...disabledRules([], configured)], ['duplicate-test'], 'a configured severity replaces the default')
  assert.deepEqual([...disabledRules(['--rule', 'duplicate-test=error'], configured)], [], 'the command line wins')
  assert.deepEqual([...disabledRules(['--rule', 'subsumed-test=off'], configured)].sort(), ['duplicate-test', 'subsumed-test'])
})

test('configuration is looked up from the real case directory, as the analyzer does', { skip: process.platform === 'win32' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'corpus-link-'))
  mkdirSync(join(root, 'real', 'case'), { recursive: true })
  mkdirSync(join(root, 'corpus'))
  writeFileSync(join(root, 'real', '.vitest-dedup.json'), JSON.stringify({ rules: { 'duplicate-test': 'off' } }))
  symlinkSync(join(root, 'real', 'case'), join(root, 'corpus', 'case'))
  assert.deepEqual(configuredRules(join(root, 'corpus', 'case'), []), { 'duplicate-test': 'off' })
  assert.deepEqual(configuredRules(join(root, 'missing', 'case'), []), {}, 'an unresolvable directory is used as given')
})

test('a directory named like the configuration file is skipped, as the analyzer skips it', () => {
  const root = mkdtempSync(join(tmpdir(), 'corpus-dir-'))
  mkdirSync(join(root, 'case', '.vitest-dedup.json'), { recursive: true })
  writeFileSync(join(root, '.vitest-dedup.json'), JSON.stringify({ rules: { 'duplicate-test': 'off' } }))
  assert.deepEqual(configuredRules(join(root, 'case'), []), { 'duplicate-test': 'off' })
})

test('a case whose configuration file switches a rule off, or on, is read that way', () => {
  const root = mkdtempSync(join(tmpdir(), 'corpus-case-'))
  writeFileSync(join(root, '.vitest-dedup.json'), JSON.stringify({ rules: { 'duplicate-test-name': 'off' } }))
  const off = evaluate(makeCase(), observation({ caseRoot: root }))
  assert.equal(off.expectations.find((o) => o.kind === 'expected-absent').eligibility, 'not-compared')
  writeFileSync(join(root, '.vitest-dedup.json'), JSON.stringify({ rules: { 'duplicate-test-name': 'warning' } }))
  const on = evaluate(makeCase(), observation({ caseRoot: root }))
  assert.equal(on.expectations.find((o) => o.kind === 'expected-absent').eligibility, 'eligible')
})

test('a trivial hook is a definite exclusion even when a test elsewhere was set aside', () => {
  const hook = (line, trivial) => ({ type: 'hook', kind: 'beforeEach', path: 'a.test.ts', line, trivial, alpha: 'x', structural: 'x', behavior: 'x' })
  const claim = { rule: 'redundant-hook', titles: null, locations: ['a.test.ts:2', 'a.test.ts:4'] }
  const aside = testRecord({ path: 'z.test.ts', comparable: false })
  assert.equal(classifyEligibility(claim, [hook(2, true), hook(4, false), aside]).eligibility, 'not-compared')
  assert.equal(classifyEligibility(claim, [hook(2, false), hook(4, false), aside]).eligibility, 'unresolved')
})

test('a silence from a rule the case switches off was never compared', () => {
  const records = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b', alpha: 'y' })]
  const claim = { rule: 'near-duplicate-test', titles: ['a', 'b'], locations: null }
  assert.equal(classifyEligibility(claim, records).eligibility, 'eligible')
  assert.equal(classifyEligibility(claim, records, new Set(['near-duplicate-test'])).eligibility, 'not-compared')
  assert.equal(classifyEligibility(claim, records, new Set(['duplicate-test'])).eligibility, 'eligible')
  const off = evaluate(makeCase({ arguments: ['--rule', 'duplicate-test-name=off'] }))
  assert.equal(off.expectations.find((o) => o.kind === 'expected-absent').eligibility, 'not-compared')
})

test('a pair spanning two registering files is never compared, except by the rule that spans files', () => {
  const across = [testRecord(), testRecord({ path: 'b.test.ts', title: 'b', alpha: 'y' })]
  const claim = (rule) => ({ rule, titles: ['a', 'b'], locations: null })
  assert.equal(classifyEligibility(claim('subsumed-test'), across).eligibility, 'not-compared')
  assert.equal(classifyEligibility(claim('cross-file-duplicate'), across).eligibility, 'eligible')
  const viaRunner = [testRecord({ registrationFile: 'one.test.ts' }), testRecord({ path: 'b.test.ts', title: 'b', registrationFile: 'one.test.ts' })]
  assert.equal(classifyEligibility(claim('duplicate-test'), viaRunner).eligibility, 'eligible', 'the registering file decides, not the declaring one')
})

test('hooks registered by different files are never paired', () => {
  const hook = (path, line) => ({ type: 'hook', kind: 'beforeEach', path, line, trivial: false, alpha: 'x', structural: 'x', behavior: 'x' })
  const claim = { rule: 'redundant-hook', titles: null, locations: ['a.test.ts:2', 'b.test.ts:2'] }
  assert.equal(classifyEligibility(claim, [hook('a.test.ts', 2), hook('b.test.ts', 2)]).eligibility, 'unresolved', 'a helper can place hooks of two files in one chain')
  assert.equal(classifyEligibility(claim, [hook('a.test.ts', 2), hook('a.test.ts', 4)]).eligibility, 'unresolved', 'no hook at b.test.ts:2')
})

test('a label set smaller than the manifest declares is refused', () => {
  const { labels, evaluated } = evaluateAll([makeCase()], [observation()])
  assert.deepEqual(measurementProblems({ ...labels, minimumCases: 1 }, evaluated), [])
  assert.match(measurementProblems({ ...labels, minimumCases: 2 }, evaluated)[0], /1 labelled case\(s\) against a declared floor of 2/)
  assert.equal(buildRecallLabels({ schemaVersion: 1, minimumCases: 4, cases: [makeCase()] }).minimumCases, 4)
  assert.equal(buildRecallLabels({ schemaVersion: 1, cases: [makeCase()] }).minimumCases, null)
  for (const bad of ['2', 2.5, 0, -1]) {
    assert.throws(() => buildRecallLabels({ schemaVersion: 1, minimumCases: bad, cases: [makeCase()] }), /`minimumCases` must be a positive integer/, String(bad))
  }
})

test('titles that locations alone would satisfy must still account for the records found', () => {
  const twice = [testRecord({ line: 3 }), testRecord({ line: 7 })]
  const claim = { titles: ['a', 'b'], locations: ['a.test.ts:3', 'a.test.ts:7'] }
  assert.equal(classifyEligibility(claim, twice).eligibility, 'unresolved', 'no participant is named b')
  const named = [testRecord({ line: 3 }), testRecord({ line: 7, title: 'b' })]
  assert.equal(classifyEligibility(claim, named).eligibility, 'eligible')
})

test('a case that analyzed no test at all measured nothing and fails the evaluation', () => {
  const none = observation({ records: [{ type: 'summary', testsAnalyzed: 0 }], summary: { type: 'summary', testsAnalyzed: 0, recordCount: 0, incomplete: false } })
  assert.throws(() => evaluate(makeCase(), none), /analyzed zero tests/)
  const held = evaluate(makeCase({ pending: 'waits on a named defect in the extractor' }), none)
  assert.equal(held.extraction.matches, false, 'a pending case may hold zero tests and is reported, not fatal')
})

test('a summary without a boolean incomplete flag, or with a non-integer unresolved count, is refused', () => {
  const summary = (patch) => ({ type: 'summary', testsAnalyzed: 2, recordCount: 1, incomplete: false, ...patch })
  assert.throws(() => evaluate(makeCase(), observation({ summary: summary({ incomplete: undefined }) })), /boolean `incomplete`/)
  assert.throws(() => evaluate(makeCase(), observation({ summary: summary({ incomplete: 'false' }) })), /boolean `incomplete`/)
  assert.throws(() => evaluate(makeCase(), observation({ summary: summary({ unresolvedTests: '0' }) })), /unresolvedTests/)
  assert.throws(() => evaluate(makeCase(), observation({ summary: summary({ unresolvedTests: -1 }) })), /non-negative integer/)
})

test('titles pick the participants out of several tests at one location, and hook kinds are compared', () => {
  const crowded = [
    testRecord({ path: 'same.test.ts', line: 1, title: 'a' }),
    testRecord({ path: 'same.test.ts', line: 1, title: 'b', alpha: 'y' }),
    testRecord({ path: 'same.test.ts', line: 1, title: 'c' }),
  ]
  const claim = { titles: ['a', 'b'], locations: ['same.test.ts:1', 'same.test.ts:1'] }
  assert.deepEqual(classifyEligibility(claim, crowded), { eligibility: 'eligible', differs: ['alpha'] })
  assert.equal(classifyEligibility({ ...claim, titles: null }, crowded).eligibility, 'unresolved')

  const hook = (kind, line) => ({ type: 'hook', kind, path: 'a.test.ts', line, trivial: false, alpha: 'x', structural: 'x', behavior: 'x' })
  const hookClaim = { rule: 'redundant-hook', titles: null, locations: ['a.test.ts:2', 'a.test.ts:4'] }
  assert.deepEqual(
    classifyEligibility(hookClaim, [hook('beforeAll', 2), hook('beforeEach', 4)]),
    { eligibility: 'eligible', differs: ['kind'] },
  )
  assert.deepEqual(classifyEligibility(hookClaim, [hook('beforeEach', 2), hook('beforeEach', 4)]), { eligibility: 'eligible', differs: [] })
})

test('a test or hook record without its identity is refused', () => {
  const plain = (record) => observation({ records: [record, testRecord({ path: 'a.test.ts', line: 7, title: 'b' }), { type: 'summary', testsAnalyzed: 2 }] })
  assert.throws(() => evaluate(makeCase(), plain(testRecord({ title: undefined }))), /test record needs a string `title`/)
  const hook = { type: 'hook', path: 'a.test.ts', line: 2, trivial: false, alpha: 'x', structural: 'x', behavior: 'x' }
  assert.throws(() => evaluate(makeCase(), plain(hook)), /hook record needs a string `kind`/)
})

test('one finding that an expected finding and a known false positive both claim is shared', () => {
  const falsePositive = { id: 'f', reason, rule: 'duplicate-test', severity: 'error', locations: ['a.test.ts:3', 'b.test.ts:3'] }
  const evaluated = evaluate(makeCase({ knownFalsePositives: [falsePositive] }))
  assert.deepEqual(evaluated.expectations.map((o) => o.status), ['shared', 'held', 'still-reported'])
})

test('a finding stream shorter than the summary says is refused, and so is a non-boolean baseline flag', () => {
  const short = observation({ records: stream(), summary: { type: 'summary', testsAnalyzed: 2, incomplete: false, recordCount: 1 } })
  assert.throws(() => evaluate(makeCase(), short), /counts 1 finding\(s\) but 0 `finding` record/)
  assert.throws(
    () => evaluate(makeCase(), observation({ records: stream([finding({ baselined: 'false' })]) })),
    /`baselined` must be a boolean/,
  )
})

test('a suite claim has no record to resolve, and a test claim does not resolve against hooks or suites', () => {
  const records = [testRecord(), testRecord({ path: 'a.test.ts', line: 7, title: 'b' })]
  assert.equal(classifyEligibility({ rule: 'duplicate-suite', titles: ['a', 'b'], locations: null }, records).eligibility, 'unresolved')
  assert.equal(classifyEligibility({ rule: 'duplicate-test', titles: ['a', 'b'], locations: null }, records).eligibility, 'eligible')
})

test('a partial stream of test records is refused', () => {
  const partial = observation({ records: stream().slice(1) })
  assert.throws(() => evaluate(makeCase(), partial), /2 test\(s\) analyzed but 1 `test` record/)
})

test('one finding satisfying two expected findings is shared, not two satisfactions', () => {
  const twice = makeCase({
    expectedFindings: [
      { rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] },
      { rule: 'duplicate-test', severity: 'error', locations: ['a.test.ts:3', 'b.test.ts:3'] },
    ],
  })
  const evaluated = evaluate(twice)
  assert.deepEqual(evaluated.expectations.map((o) => o.status), ['shared', 'shared', 'held'])
  const { evaluated: all } = evaluateAll([twice], [observation()])
  assert.equal(buildRuleTable(all).find((r) => r.rule === 'duplicate-test').violations, 2)
})

test('evidence that cannot be scored is an error, not a miss', () => {
  const testCase = makeCase()
  assert.throws(() => evaluate(testCase, observation({ directoryPresent: false })), /directory is missing/)
  assert.throws(() => evaluate(testCase, observation({ sourceFiles: [] })), /no analyzable source/)
  assert.throws(() => evaluate(testCase, observation({ summary: null })), /no summary/)
  assert.throws(
    () => evaluate(testCase, observation({ summary: { type: 'summary', testsAnalyzed: '2' } })),
    /testsAnalyzed/,
  )
  assert.throws(
    () => evaluate(testCase, observation({ records: [finding({ titles: 'a' }), { type: 'summary', testsAnalyzed: 2 }] })),
    /titles/,
  )
  assert.throws(
    () => evaluate(testCase, observation({ records: [finding({ rule: undefined }), { type: 'summary', testsAnalyzed: 2 }] })),
    /rule/,
  )
  for (const patch of [{ severity: '' }, { severity: 'critical' }, { rule: 'invented-rule' }]) {
    assert.throws(
      () => evaluate(testCase, observation({ records: [finding(patch), { type: 'summary', testsAnalyzed: 2 }] })),
      /known `rule` and a `severity`/,
      JSON.stringify(patch),
    )
  }
  assert.throws(
    () => evaluate(testCase, observation({ records: [finding({ locations: [{ path: 'a' }] }), { type: 'summary', testsAnalyzed: 2 }] })),
    /locations/,
  )
  assert.throws(
    () => evaluate(testCase, observation({ records: [testRecord({ comparable: 'yes' }), { type: 'summary', testsAnalyzed: 2 }] })),
    /comparable/,
  )
  assert.throws(
    () => evaluate(testCase, observation({ records: [null, { type: 'summary', testsAnalyzed: 2 }] })),
    /typed object/,
  )
  assert.throws(
    () => evaluate(testCase, observation({ records: [finding(), { type: 'summary', testsAnalyzed: 2 }] })),
    /--emit-test-fingerprints/,
  )
})

/** Evaluates a list of cases the way the command does. */
function evaluateAll(cases, observations) {
  const labels = buildRecallLabels({ schemaVersion: 1, cases })
  return {
    labels,
    evaluated: labels.cases.map((label, index) =>
      evaluateCase(label, labels.expectations.filter((e) => e.caseName === label.name), observations[index]),
    ),
  }
}

test('the rule table counts denominators, excludes pending cases and tallies violations', () => {
  const { labels, evaluated } = evaluateAll(
    [
      makeCase({ name: 'ok' }),
      makeCase({ name: 'broken', expectedTests: 5 }),
      makeCase({ name: 'held', pending: 'waits on a named defect in the extractor', expectedTests: 5 }),
    ],
    [observation(), observation(), observation()],
  )
  const rows = buildRuleTable(evaluated)
  const row = rows.find((r) => r.rule === 'duplicate-test')
  assert.equal(row.cases, 2, 'the pending case is not in the table')
  assert.deepEqual(row.expectedFindings, { satisfied: 2, total: 2 })
  assert.equal(row.extractionMismatches, 1)
  assert.equal(row.violations, 1)
  // An absence is counted under the rule it forbids, not the rule of the case that holds it.
  const named = rows.find((r) => r.rule === 'duplicate-test-name')
  assert.deepEqual(named.expectedAbsent, { held: 2, total: 2, eligible: 2, eligibleEqualKeys: 0, notCompared: 0, unresolved: 0 })
  assert.equal(named.cases, 0)
  const report = buildReport({ binary: 'b', root: 'r', labels, cases: evaluated })
  assert.equal(report.work.pendingCases, 1)
  assert.equal(report.pending[0].held, 1)
})

test('findings nobody labelled are counted against their own rule', () => {
  const obs = observation({
    records: [
      testRecord(),
      testRecord({ path: 'a.test.ts', line: 7, title: 'b' }),
      finding(),
      finding({ rule: 'redundant-hook', severity: 'warning', titles: ['x', 'y'] }),
      { type: 'summary', testsAnalyzed: 2, incomplete: false },
    ],
  })
  const { evaluated } = evaluateAll([makeCase()], [obs])
  const rows = buildRuleTable(evaluated)
  assert.equal(rows.find((r) => r.rule === 'redundant-hook').unclaimedEmitted, 1)
  assert.equal(rows.find((r) => r.rule === 'duplicate-test').unclaimedEmitted, 0)
})

test('absences held split by whether the pair was eligible for comparison', () => {
  const different = observation({
    records: stream([finding()], { alpha: 'y' }),
  })
  const skipped = observation({
    records: stream([finding()], { comparable: false }),
  })
  const { evaluated } = evaluateAll([makeCase({ name: 'one' }), makeCase({ name: 'two' })], [different, skipped])
  const row = buildRuleTable(evaluated).find((r) => r.rule === 'duplicate-test-name')
  assert.deepEqual(row.expectedAbsent, { held: 2, total: 2, eligible: 1, eligibleEqualKeys: 0, notCompared: 1, unresolved: 0 })
})

test('a run that did not measure the corpus is refused, not summarized', () => {
  const { labels, evaluated } = evaluateAll([makeCase()], [observation()])
  assert.deepEqual(measurementProblems(labels, evaluated), [])
  assert.match(measurementProblems(labels, [])[0], /0 of 1 labelled case/)
  assert.match(measurementProblems({ cases: [], expectations: [] }, [])[0], /empty/)
  const dropped = [{ ...evaluated[0], expectations: [] }]
  assert.match(measurementProblems(labels, dropped).join('\n'), /0 of 2 labelled expectation/)
  const none = [{ ...evaluated[0], extraction: { ...evaluated[0].extraction, tests: 0 }, testRecords: 0 }]
  assert.match(measurementProblems(labels, none).join('\n'), /zero tests/)
  assert.match(measurementProblems(labels, none).join('\n'), /no `test` record/)
  assert.throws(() => buildReport({ binary: 'b', root: 'r', labels, cases: [] }), /did not measure the corpus/)
})

test('the distribution counts cases along each label axis', () => {
  const { labels } = evaluateAll(
    [makeCase({ name: 'p' }), makeCase({ name: 'n', role: 'negative', origin: 'real', verdict: 'reviewed' })],
    [observation(), observation()],
  )
  const out = buildDistribution(labels)
  assert.deepEqual(out.byRole, { positive: 1, negative: 1 })
  assert.deepEqual(out.byOrigin, { synthetic: 1, real: 1 })
  assert.deepEqual(out.byVerdict, { generated: 1, reviewed: 1 })
  assert.deepEqual(out.byRule, { 'duplicate-test': 2 })
  assert.deepEqual(out.byKind, { 'expected-finding': 2, 'expected-absent': 2 })
  assert.deepEqual(out.pending, { active: 2 })
})

test('the text report states violations, prints n/a for an empty population and lists pending cases', () => {
  const { labels, evaluated } = evaluateAll(
    [makeCase({ name: 'broken', expectedTests: 5 }), makeCase({ name: 'held', pending: 'waits on a named defect in the extractor', expectedTests: 5 })],
    [observation(), observation()],
  )
  const text = renderText(buildReport({ binary: './b', root: 'r', labels, cases: evaluated })).join('\n')
  assert.match(text, /contract violations: 1/)
  assert.match(text, /known misses kept {3}n\/a/)
  assert.match(text, /expected findings {3}1\/1 \(100\.0%\)/)
  assert.match(text, /broken: extraction differs from the label/)
  assert.match(text, /pending: held \(1 assertion\(s\) still failing\)/)
  assert.match(text, /byRule: duplicate-test 2/)
  const clean = renderText(buildReport({ binary: './b', root: 'r', labels: evaluateAll([makeCase()], [observation()]).labels, cases: evaluateAll([makeCase()], [observation()]).evaluated })).join('\n')
  assert.match(clean, /contract violations: 0/)
  assert.doesNotMatch(clean, /cases with violations/)
})

test('known entries are tallied per rule and a record without a location is refused', () => {
  const miss = { id: 'm', reason, rule: 'near-duplicate-test', severity: 'warning', titles: ['a', 'b'] }
  const falsePositive = { id: 'f', reason, rule: 'duplicate-test-name', severity: 'warning', titles: ['a', 'b'] }
  const testCase = makeCase({ knownMisses: [miss], knownFalsePositives: [falsePositive] })
  const records = stream([finding(), finding({ rule: 'duplicate-test-name', severity: 'warning' })])
  const { evaluated } = evaluateAll([testCase], [observation({ records })])
  const rows = buildRuleTable(evaluated)
  assert.deepEqual(rows.find((r) => r.rule === 'near-duplicate-test').knownMisses, { stillMissed: 1, total: 1 })
  assert.deepEqual(rows.find((r) => r.rule === 'duplicate-test-name').knownFalsePositives, { stillReported: 1, total: 1 })

  assert.throws(
    () => evaluate(makeCase(), observation({ records: [testRecord({ path: undefined }), { type: 'summary', testsAnalyzed: 2 }] })),
    /needs a `path` and a `line`/,
  )
})

test('an expectation kind the evaluator does not know is an error', () => {
  const labels = buildRecallLabels({ schemaVersion: 1, cases: [makeCase()] })
  const odd = [{ ...labels.expectations[0], kind: 'invented' }]
  assert.throws(() => evaluateCase(labels.cases[0], odd, observation()), /unknown expectation kind "invented"/)
})
