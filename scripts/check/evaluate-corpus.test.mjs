// Tests for the corpus evaluator's command line: argument handling, the exit contract, and what
// reaches the JSON file. The case runner is replaced, so no binary is needed.
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
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

/** A recall root holding two identical cases, `first` and `second`. */
function makeTwoCaseRoot() {
  const root = makeRoot()
  const manifest = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8'))
  const [only] = manifest.cases
  manifest.cases = [
    { ...only, name: 'first', path: 'first' },
    { ...only, name: 'second', path: 'second' },
  ]
  writeFileSync(join(root, 'manifest.json'), JSON.stringify(manifest))
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

/** Two stand-in binaries on disk, so the run record can digest them. */
function binaries() {
  const dir = mkdtempSync(join(tmpdir(), 'evaluate-binaries-'))
  writeFileSync(join(dir, 'new'), 'candidate')
  writeFileSync(join(dir, 'old'), 'baseline')
  return { candidate: join(dir, 'new'), baseline: join(dir, 'old') }
}

/** An observer that runs `candidate` and `baseline` behaviours by the binary asked for. */
function twoBinaries(paths, { candidate, baseline }) {
  return (input) => (input.binary === paths.candidate ? candidate(input) : baseline(input))
}

const git = () => {
  throw new Error('no git here')
}

test('arguments: the baseline options and the refusals that keep them from being half given', () => {
  const parsed = parseArguments(['--baseline-binary', 'old', '--baseline-ref', 'v0.5.1', '--fail-on-regression'])
  assert.deepEqual([parsed.baseline, parsed.baselineRef, parsed.failOnRegression], ['old', 'v0.5.1', true])
  assert.deepEqual([parseArguments([]).baseline, parseArguments([]).baselineRef], [null, null])
  assert.throws(() => parseArguments(['--baseline-ref', 'v0.5.1']), /need --baseline-binary/)
  assert.throws(() => parseArguments(['--fail-on-regression']), /need --baseline-binary/)
  assert.throws(() => parseArguments(['--baseline-binary']), /--baseline-binary needs a value/)
  assert.throws(() => parseArguments(['--baseline-binary', '']), /--baseline-binary needs a value/)
  assert.throws(() => parseArguments(['--baseline-binary', 'old', '--baseline-ref', '-x']), /must be a revision/)
})

test('a candidate that lost a finding the baseline reported is a regression, and the command can fail on it', () => {
  const paths = binaries()
  const observe = twoBinaries(paths, { candidate: silent, baseline: satisfied })
  const argv = [paths.candidate, '--root', makeRoot(), '--baseline-binary', paths.baseline]
  const report = []
  const result = runCli([...argv, '--json', 'out.json'], { observe, git, version: () => 'v', write: (...args) => report.push(args) })
  assert.equal(result.status, 0, 'data by default')
  const text = result.lines.join('\n')
  assert.match(text, /comparison with the baseline binary: 1 regression\(s\)/)
  assert.match(text, /c#1 regressed/)
  assert.match(text, /no --baseline-ref/)
  const json = JSON.parse(report[0][1])
  assert.equal(json.comparison.regressions, 1)
  assert.equal(json.comparison.runRecord.baseline.version, 'v')
  assert.equal(runCli([...argv, '--fail-on-regression'], { observe, git, version: () => 'v' }).status, 1)
})

test('an unchanged candidate compares clean and passes --fail-on-regression', () => {
  const paths = binaries()
  const observe = twoBinaries(paths, { candidate: satisfied, baseline: satisfied })
  const result = runCli([paths.candidate, '--root', makeRoot(), '--baseline-binary', paths.baseline, '--fail-on-regression'], { observe, git, version: () => 'v' })
  assert.equal(result.status, 0)
  assert.match(result.lines.join('\n'), /0 regression\(s\), 0 unavailable/)
})

test('a baseline that cannot run or be read is unavailable, fails --fail-on-regression and never reads as zero findings', () => {
  const paths = binaries()
  const crashing = () => {
    throw new Error('binary exited 2 -- an operational failure')
  }
  const everywhere = twoBinaries(paths, { candidate: satisfied, baseline: crashing })
  assert.throws(
    () => runCli([paths.candidate, '--root', makeRoot(), '--baseline-binary', paths.baseline], { observe: everywhere, git, version: () => 'v' }),
    /no comparable run on any case/,
  )
  const oldSchema = twoBinaries(paths, {
    candidate: satisfied,
    baseline: (input) => ({ ...satisfied(input), summary: { type: 'summary', testsAnalyzed: 2 } }),
  })
  assert.throws(
    () => runCli([paths.candidate, '--root', makeRoot(), '--baseline-binary', paths.baseline], { observe: oldSchema, git, version: () => 'v' }),
    /no comparable run/,
  )
  const root = makeTwoCaseRoot()
  const partial = twoBinaries(paths, {
    candidate: satisfied,
    baseline: (input) => {
      if (input.testCase.name === 'second') throw new Error(`${paths.baseline} crashed on this case`)
      return satisfied(input)
    },
  })
  const argv = [paths.candidate, '--root', root, '--baseline-binary', paths.baseline]
  const result = runCli(argv, { observe: partial, git, version: () => 'v' })
  assert.match(result.lines.join('\n'), /unavailable: second -- the baseline binary crashed on this case/)
  assert.doesNotMatch(result.lines.join('\n'), new RegExp(paths.baseline.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
  assert.equal(runCli([...argv, '--fail-on-regression'], { observe: partial, git, version: () => 'v' }).status, 1)
})

test('a baseline that analyzed no tests is unavailable even on a pending case', () => {
  const paths = binaries()
  const root = makeTwoCaseRoot()
  const manifest = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8'))
  manifest.cases[1].pending = 'waits on a named defect in the extractor'
  writeFileSync(join(root, 'manifest.json'), JSON.stringify(manifest))
  const empty = (input) => ({ ...satisfied(input), records: [], summary: { type: 'summary', testsAnalyzed: 0, incomplete: false, recordCount: 0 } })
  const observe = twoBinaries(paths, {
    candidate: satisfied,
    baseline: (input) => (input.testCase.name === 'second' ? empty(input) : satisfied(input)),
  })
  const argv = [paths.candidate, '--root', root, '--baseline-binary', paths.baseline]
  const result = runCli(argv, { observe, git, version: () => 'v' })
  assert.match(result.lines.join('\n'), /1 unavailable case\(s\)/)
  assert.equal(runCli([...argv, '--fail-on-regression'], { observe, git, version: () => 'v' }).status, 1)
})

test('a baseline without test records is still compared, since it may predate that flag', () => {
  const paths = binaries()
  const seen = []
  const observe = twoBinaries(paths, {
    candidate: satisfied,
    baseline: (input) => {
      seen.push(input.extraArguments)
      const observation = satisfied(input)
      return { ...observation, records: observation.records.filter((record) => record.type !== 'test') }
    },
  })
  const result = runCli([paths.candidate, '--root', makeRoot(), '--baseline-binary', paths.baseline], { observe, git, version: () => 'v' })
  assert.deepEqual(seen, [[]], 'the flag is not asked of the baseline')
  assert.match(result.lines.join('\n'), /0 regression\(s\), 0 unavailable/)
})

test('--baseline-ref separates label changes from detector changes', () => {
  const paths = binaries()
  const root = makeRoot()
  const asked = []
  const readChanges = (input) => {
    asked.push([input.ref, input.manifestCases.map((c) => c.name)])
    return new Map([['c', 'new case, absent at v0.5.1']])
  }
  const observe = twoBinaries(paths, { candidate: silent, baseline: satisfied })
  const result = runCli([paths.candidate, '--root', root, '--baseline-binary', paths.baseline, '--baseline-ref', 'v0.5.1', '--fail-on-regression'], {
    observe,
    git,
    version: () => 'v',
    readChanges,
  })
  assert.deepEqual(asked, [['v0.5.1', ['c']]])
  assert.equal(result.status, 0, 'a loss on a case new since the baseline is not a detector regression')
  assert.match(result.lines.join('\n'), /label-changed: c -- new case, absent at v0\.5\.1/)
  const failing = () => {
    throw new Error('git show failed for no-such-ref')
  }
  assert.throws(
    () => runCli([paths.candidate, '--root', root, '--baseline-binary', paths.baseline, '--baseline-ref', 'no-such-ref'], { observe, git, version: () => 'v', readChanges: failing }),
    /no-such-ref/,
  )
})
