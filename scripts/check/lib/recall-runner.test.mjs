// Tests for the recall runner: the pure matching helpers the gates' diagnostics and the corpus
// evaluator share, and the observation short-circuits that must not run the binary.
import assert from 'node:assert/strict'
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import {
  describeMatcher,
  findingMatches,
  observeRecallCase,
} from './recall-runner.mjs'

/** A one-participant-pair finding shaped like a jsonl finding record. */
function finding(overrides = {}) {
  return {
    type: 'finding',
    rule: 'duplicate-test',
    severity: 'error',
    baselined: false,
    titles: ['suite > a', 'suite > b'],
    locations: [
      { path: 'src/a.test.ts', line: 10 },
      { path: 'src/b.test.ts', line: 20 },
    ],
    ...overrides,
  }
}

test('findingMatches compares rule, optional severity, and sorted title and location multisets', () => {
  const expected = {
    rule: 'duplicate-test',
    severity: 'error',
    titles: ['suite > b', 'suite > a'],
    locations: ['src/b.test.ts:20', 'src/a.test.ts:10'],
  }
  assert.equal(findingMatches(finding(), expected), true)
  assert.equal(findingMatches(finding({ rule: 'name-collision' }), expected), false)
  assert.equal(findingMatches(finding({ severity: 'warning' }), expected), false)
  // An unspecified severity widens the match rather than requiring absence.
  assert.equal(findingMatches(finding({ severity: 'warning' }), { rule: 'duplicate-test' }), true)
  assert.equal(
    findingMatches(finding({ titles: ['suite > a', 'suite > c'] }), expected),
    false,
    'title multiset mismatch',
  )
  assert.equal(
    findingMatches(finding({ titles: ['suite > a'] }), expected),
    false,
    'title count mismatch',
  )
  assert.equal(
    findingMatches(
      finding({ locations: [{ path: 'src/a.test.ts', line: 10 }, { path: 'src/c.test.ts', line: 1 }] }),
      expected,
    ),
    false,
    'location multiset mismatch',
  )
})

test('describeMatcher renders the constraints a diagnostic names', () => {
  assert.equal(describeMatcher({ rule: 'duplicate-test' }), 'duplicate-test')
  assert.equal(describeMatcher({ rule: 'duplicate-test', severity: 'error' }), 'duplicate-test [error]')
  assert.equal(
    describeMatcher({
      rule: 'duplicate-test',
      severity: 'error',
      titles: ['suite > a', 'suite > b'],
      locations: ['src/a.test.ts:10'],
    }),
    'duplicate-test [error] {suite > a | suite > b} @ src/a.test.ts:10',
  )
})

test('observeRecallCase reports a missing directory as a fact and never runs the binary', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  const observed = observeRecallCase({
    binary: '/nonexistent/vitest-dedup',
    root: scratch,
    testCase: { name: 'gone', path: 'cases/gone' },
  })
  assert.equal(observed.directoryPresent, false)
  assert.deepEqual(observed.sourceFiles, [])
  assert.equal(observed.records, null)
  assert.equal(observed.summary, null)
  assert.equal(observed.label, 'gone (cases/gone)')
})

test('observeRecallCase reports a sourceless directory as a fact and never runs the binary', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'notes.md'), 'not analyzable source')
  const observed = observeRecallCase({
    binary: '/nonexistent/vitest-dedup',
    root: scratch,
    testCase: { name: 'empty', path: '.' },
  })
  assert.equal(observed.directoryPresent, true)
  assert.deepEqual(observed.sourceFiles, [])
  assert.equal(observed.records, null)
  assert.equal(observed.summary, null)
})

/** A stand-in binary that prints fixed jsonl, then a summary echoing the arguments it received. */
function fakeBinary(directory, { summary = true, exitCode = 0 } = {}) {
  const path = join(directory, 'fake-binary')
  const lines = [
    `printf '%s\\n' '{"type":"finding","rule":"duplicate-test"}'`,
    summary ? `printf '{"type":"summary","testsAnalyzed":2,"args":"%s"}\\n' "$*"` : '',
    `exit ${exitCode}`,
  ]
  writeFileSync(path, `#!/bin/sh\n${lines.join('\n')}\n`)
  chmodSync(path, 0o755)
  return path
}

test('observeRecallCase runs the binary on the case and returns its records and summary', { skip: process.platform === 'win32' }, () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  const caseDir = join(scratch, 'cases', 'nested')
  mkdirSync(join(caseDir, 'deep'), { recursive: true })
  writeFileSync(join(caseDir, 'deep', 'a.spec.mts'), 'export {}')
  const observed = observeRecallCase({
    binary: fakeBinary(scratch, { exitCode: 1 }),
    root: scratch,
    testCase: { name: 'nested', path: 'cases/nested', arguments: ['--min-lines', '3'] },
  })
  assert.equal(observed.label, 'nested (cases/nested)')
  assert.equal(observed.caseRoot, caseDir)
  assert.equal(observed.directoryPresent, true)
  assert.equal(observed.sourceFiles.length, 1, 'a source file in a subdirectory counts')
  assert.deepEqual(
    observed.records.map((record) => record.type),
    ['finding', 'summary'],
  )
  assert.equal(observed.summary, observed.records[1])
  assert.equal(observed.summary.args, `${caseDir} --min-lines 3 --reporters jsonl`)
})

test('observeRecallCase passes extra arguments after the case\'s own and only when asked', { skip: process.platform === 'win32' }, () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'a.test.ts'), 'export {}')
  const binary = fakeBinary(scratch)
  const testCase = { name: 'extra', path: '.', arguments: ['--min-lines', '3'] }
  const plain = observeRecallCase({ binary, root: scratch, testCase })
  assert.equal(plain.summary.args, `${scratch} --min-lines 3 --reporters jsonl`)
  const extra = observeRecallCase({ binary, root: scratch, testCase, extraArguments: ['--emit-test-fingerprints'] })
  assert.equal(extra.summary.args, `${scratch} --min-lines 3 --emit-test-fingerprints --reporters jsonl`)
})

test('observeRecallCase returns a null summary when the run wrote none', { skip: process.platform === 'win32' }, () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'a.test.ts'), 'export {}')
  const observed = observeRecallCase({
    binary: fakeBinary(scratch, { summary: false }),
    root: scratch,
    testCase: { name: 'quiet', path: '.' },
  })
  assert.equal(observed.records.length, 1)
  assert.equal(observed.summary, null)
})

test('observeRecallCase propagates an operational run failure instead of swallowing it', () => {
  const scratch = mkdtempSync(join(tmpdir(), 'recall-runner-'))
  writeFileSync(join(scratch, 'a.test.ts'), 'export {}')
  assert.throws(
    () =>
      observeRecallCase({
        binary: '/nonexistent/vitest-dedup',
        root: scratch,
        testCase: { name: 'runs', path: '.' },
      }),
    /ENOENT|vitest-dedup/,
    'a missing binary is an operational failure the gate dies on, never "zero findings"',
  )
})
