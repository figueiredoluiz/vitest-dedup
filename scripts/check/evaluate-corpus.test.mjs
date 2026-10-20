// Tests for the corpus evaluator's command line: argument handling, the exit contract, and what
// reaches the JSON file. The case runner is replaced, so no binary is needed.
import assert from 'node:assert/strict'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import { parseArguments, runCli } from './evaluate-corpus.mjs'

/** A one-case recall root on disk. */
function makeRoot(caseOverrides = {}) {
  const root = mkdtempSync(join(tmpdir(), 'evaluate-corpus-'))
  const testCase = {
    name: 'c',
    path: 'c',
    rule: 'duplicate-test',
    role: 'positive',
    origin: 'synthetic',
    verdict: 'generated',
    classifiedAt: '2026-10-20',
    expectedTests: 2,
    expectedIncomplete: false,
    expectedFindings: [{ rule: 'duplicate-test', severity: 'error', titles: ['a', 'b'] }],
    expectedAbsent: [{ rule: 'duplicate-test-name', titles: ['a', 'b'] }],
    ...caseOverrides,
  }
  writeFileSync(join(root, 'manifest.json'), JSON.stringify({ schemaVersion: 1, cases: [testCase] }))
  return root
}

/** The observation of a case whose labels the run satisfies. */
function satisfied({ testCase }) {
  const test_ = (path, title) => ({
    type: 'test', path, line: 3, title, comparable: true, trivial: false,
    alpha: path, structural: 's', behavior: 'b', contextKey: 'c', modifierKey: 'm',
  })
  return {
    label: testCase.name,
    directoryPresent: true,
    sourceFiles: ['a.test.ts'],
    records: [
      test_('a.test.ts', 'a'),
      test_('b.test.ts', 'b'),
      { type: 'finding', rule: 'duplicate-test', severity: 'error', baselined: false, titles: ['a', 'b'], locations: [{ path: 'a.test.ts', line: 3 }] },
    ],
    summary: { type: 'summary', testsAnalyzed: 2, incomplete: false, recordCount: 1 },
  }
}

/** An observation in which the expected finding is not reported. */
function silent(input) {
  const observation = satisfied(input)
  observation.records = observation.records.filter((record) => record.type !== 'finding')
  observation.summary.recordCount = 0
  return observation
}

test('arguments: defaults, flags and the refusals that keep a typo from evaluating something else', () => {
  const defaults = parseArguments([])
  assert.match(defaults.binary, /vitest-dedup/)
  assert.match(defaults.root, /fixtures[\\/]recall$/)
  assert.equal(defaults.json, null)
  assert.equal(defaults.failOnViolation, false)

  const parsed = parseArguments(['./bin', '--root', 'corpus', '--json', 'out.json', '--fail-on-violation'])
  assert.equal(parsed.binary, './bin')
  assert.match(parsed.root, /corpus$/)
  assert.equal(parsed.json, 'out.json')
  assert.equal(parsed.failOnViolation, true)

  assert.throws(() => parseArguments(['--nope']), /unknown option --nope/)
  assert.throws(() => parseArguments(['--root']), /--root needs a value/)
  assert.throws(() => parseArguments(['--json', '--fail-on-violation']), /--json needs a value/)
  assert.throws(() => parseArguments(['a', 'b']), /unexpected argument b/)
})

test('a satisfied corpus exits 0 and every case is run with test records requested', () => {
  const seen = []
  const result = runCli(['./bin', '--root', makeRoot()], {
    observe: (input) => {
      seen.push(input.extraArguments)
      return satisfied(input)
    },
  })
  assert.equal(result.status, 0)
  assert.deepEqual(seen, [['--emit-test-fingerprints']])
  assert.match(result.lines.join('\n'), /contract violations: 0/)
})

test('a violation is data by default and a failure with --fail-on-violation', () => {
  const root = makeRoot()
  const quiet = runCli(['./bin', '--root', root], { observe: silent })
  assert.equal(quiet.status, 0)
  assert.match(quiet.lines.join('\n'), /contract violations: 1/)
  const strict = runCli(['./bin', '--root', root, '--fail-on-violation'], { observe: silent })
  assert.equal(strict.status, 1)
  const clean = runCli(['./bin', '--root', root, '--fail-on-violation'], { observe: satisfied })
  assert.equal(clean.status, 0)
})

test('the JSON report is written when asked and only then', () => {
  const root = makeRoot()
  const writes = []
  runCli(['./bin', '--root', root], { observe: satisfied, write: (...args) => writes.push(args) })
  assert.deepEqual(writes, [])
  runCli(['./bin', '--root', root, '--json', 'out.json'], { observe: satisfied, write: (...args) => writes.push(args) })
  assert.equal(writes.length, 1)
  assert.equal(writes[0][0], 'out.json')
  const report = JSON.parse(writes[0][1])
  assert.equal(report.schemaVersion, 1)
  assert.equal(report.work.cases, 1)
  assert.equal(report.violations, 0)
})

test('a case that cannot be measured fails the command instead of shrinking the table', () => {
  const root = makeRoot()
  assert.throws(
    () => runCli(['./bin', '--root', root], { observe: (input) => ({ ...satisfied(input), summary: null }) }),
    /no summary/,
  )
  assert.throws(
    () => runCli(['./bin', '--root', root], { observe: (input) => ({ ...satisfied(input), directoryPresent: false }) }),
    /directory is missing/,
  )
})

test('an unreadable manifest fails the command', () => {
  const root = mkdtempSync(join(tmpdir(), 'evaluate-corpus-'))
  writeFileSync(join(root, 'manifest.json'), JSON.stringify({ schemaVersion: 1, cases: [] }))
  assert.throws(() => runCli(['./bin', '--root', root], { observe: satisfied }), /no cases/)
})
