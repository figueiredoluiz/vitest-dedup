// `validateMemoryRecord` is the chokepoint every memory number passes through before any gate
// compares it, so its job is to REJECT things -- and a rejection test that cannot fail is the
// defect this repository ships most.
//
// Each case below therefore starts from a record that validates clean and breaks exactly one
// thing, which answers "what change to the code would make this fail?" with "deleting that
// branch". The values chosen to break it are the ones a comparison would silently accept:
// `undefined` (every `>` returns false), a negative integer (clears every ceiling), a numeric
// string (`"3287" > 3287` is false), and `NaN`. `Number.isFinite` alone accepts the negative, so
// it is not a validation -- that is the specific hole three shipped bugs in this repository went
// through.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { test } from 'node:test'
import { printBreakdown } from '../check-memory.mjs'

import {
  CONTEXT_FIELDS,
  FIELDS,
  PHASE,
  SCHEMA_VERSION,
  discardCorpus,
  parseGateArguments,
  selectMemoryRecord,
  validateMemoryRecord,
  workPerformedProblems,
} from './memory-record.mjs'

const CHECKS = resolve(dirname(fileURLToPath(import.meta.url)), '..')

/** A record that validates clean: every sum balances and every cross-field invariant holds. */
function valid(overrides = {}) {
  const fields = {
    body: 500, context: 2000, suitePath: 90, title: 38, modifiers: 2, location: 53, spine: 470,
    scalars: 0,
  }
  // `moduleSetup` carries a NON-ZERO share on purpose: these buckets must sum to
  // `fields.context`, and a zero would satisfy that sum whether or not the field is counted at
  // all. 560 + 34 + 0 + 1400 + 6 + 0 = 2000.
  const contextFields = {
    hooks: 560, mockedModules: 34, fixtures: 0, imports: 1400, moduleSetup: 6, scalars: 0,
  }
  const retainedBytes = Object.values(fields).reduce((sum, value) => sum + value, 0)
  return {
    type: 'memory',
    schemaVersion: SCHEMA_VERSION,
    phase: PHASE,
    testsAnalyzed: 1,
    filesAnalyzed: 1,
    retainedBytes,
    bytesPerTest: retainedBytes,
    fields,
    contextFields,
    declarationsRetained: 1,
    sharedTextAttribution: 'proportional: …',
    peakRssBytes: retainedBytes * 4,
    peakRssNotice: '…',
    ...overrides,
  }
}

/** The owned record emitted by the binary: legacy buckets are zero and engine attribution sums. */
function owned() {
  const engineFields = { cases: 101, suites: 13, hooks: 17, sites: 19, blind: 23, modules: 29, exports: 31, scalars: 0 }
  const retainedBytes = Object.values(engineFields).reduce((a, b) => a + b, 0)
  return valid({
    fields: { body: 0, context: 0, suitePath: 0, title: 0, modifiers: 0, location: 0, spine: 0, scalars: 0, engine: retainedBytes },
    contextFields: { hooks: 0, mockedModules: 0, fixtures: 0, imports: 0, moduleSetup: 0, scalars: 0 },
    retainedBytes, bytesPerTest: retainedBytes, engineFields,
    engineMemoryNotice: 'Retained owned capacity; temporary parser state is excluded.',
  })
}

test('the gate prints every owned population with real attribution and the estimate notice', () => {
  const record = owned()
  const lines = []
  printBreakdown(record, (line) => lines.push(line))
  assert.ok(lines.some((line) => line.includes(record.engineMemoryNotice)))
  assert.ok(lines.some((line) => line.includes('owned-engine bytes by population')))
  for (const [key, bytes] of Object.entries(record.engineFields)) {
    const row = lines.find((line) => line.trim().startsWith(`${key} `))
    assert.ok(row, key)
    assert.match(row, new RegExp(`^\\s*${key}\\s+${bytes} bytes/test`))
    assert.ok(row.includes(`${(100 * bytes / record.retainedBytes).toFixed(1)}%`), row)
  }
  assert.ok(!lines.some((line) => line.includes('context`')))
})

test('the gate preserves legacy attribution and prints absolute owned bytes with no tests', () => {
  const legacy = []
  printBreakdown(valid(), (line) => legacy.push(line))
  assert.ok(legacy.some((line) => /^\s*body\s+500 bytes\/test/.test(line)))
  assert.ok(legacy.some((line) => line.includes('context`')))
  assert.ok(!legacy.some((line) => line.includes('owned-engine')))
  for (const zeroBytes of [false, true]) {
    const record = owned()
    record.testsAnalyzed = record.bytesPerTest = record.declarationsRetained = 0
    if (zeroBytes) {
      for (const key of Object.keys(record.engineFields)) record.engineFields[key] = 0
      record.retainedBytes = record.fields.engine = 0
    }
    assert.deepEqual(validateMemoryRecord(record), [])
    const empty = []
    printBreakdown(record, (line) => empty.push(line))
    assert.ok(empty.some((line) => line.includes(record.engineMemoryNotice)))
    for (const [key, bytes] of Object.entries(record.engineFields)) {
      assert.ok(empty.some((line) => new RegExp(`^\\s*${key}\\s+${bytes} bytes$`).test(line)), key)
    }
    assert.ok(!empty.some((line) => /bytes\/test|NaN|Infinity|%/.test(line)))
  }
  const legacyEmpty = []
  printBreakdown(valid({ testsAnalyzed: 0 }), (line) => legacyEmpty.push(line))
  assert.ok(legacyEmpty.some((line) => line.includes('nothing was analyzed')))
})

test('owned attribution validates real populations, zero work and the unchanged legacy shape', () => {
  assert.deepEqual(validateMemoryRecord(owned()), [])
  const zero = owned()
  for (const key of Object.keys(zero.engineFields)) zero.engineFields[key] = 0
  zero.fields.engine = zero.retainedBytes = zero.bytesPerTest = zero.testsAnalyzed = zero.filesAnalyzed = zero.declarationsRetained = 0
  assert.deepEqual(validateMemoryRecord(zero), [])
  assert.deepEqual(validateMemoryRecord(valid()), [])
})

test('owned suite-only and parse-failure records count read files and omit zero engine bytes', () => {
  const suite = owned()
  suite.testsAnalyzed = suite.bytesPerTest = suite.declarationsRetained = 0
  assert.deepEqual(validateMemoryRecord(suite), [])
  const parse = owned()
  for (const key of Object.keys(parse.engineFields)) parse.engineFields[key] = 0
  parse.retainedBytes = parse.bytesPerTest = parse.testsAnalyzed = parse.declarationsRetained = 0
  delete parse.fields.engine
  assert.deepEqual(validateMemoryRecord(parse), [])
  parse.engineFields.modules = 1
  assert.ok(validateMemoryRecord(parse).some((p) => p.includes('must sum')))
  const legacy = valid({ testsAnalyzed: 0, bytesPerTest: 0 })
  assert.ok(validateMemoryRecord(legacy).some((p) => p.includes('filesAnalyzed')))
})

test('owned attribution rejects every missing, invalid and unknown bucket', () => {
  for (const key of Object.keys(owned().engineFields)) {
    for (const value of [undefined, -1, 0.5, '1', NaN, null]) {
      const record = owned()
      record.engineFields[key] = value
      assert.ok(validateMemoryRecord(record).length, `${key}: ${value}`)
    }
    const mismatch = owned()
    mismatch.engineFields[key] += 1
    assert.ok(validateMemoryRecord(mismatch).some((p) => p.includes('must sum')))
  }
  for (const value of [undefined, null, [], 'bad']) {
    assert.ok(validateMemoryRecord({ ...owned(), engineFields: value }).length)
  }
  const extra = owned()
  extra.engineFields.unaccounted = 0
  assert.ok(validateMemoryRecord(extra).length)
})

test('owned attribution requires both breakdowns and the notice without mixing legacy retention', () => {
  for (const key of ['engineFields', 'engineMemoryNotice']) {
    const record = owned()
    delete record[key]
    assert.ok(validateMemoryRecord(record).length, key)
  }
  for (const value of ['', null, 0]) {
    assert.ok(validateMemoryRecord({ ...owned(), engineMemoryNotice: value }).length)
  }
  const missing = owned()
  delete missing.fields.engine
  assert.ok(validateMemoryRecord(missing).length)
  const mixed = owned()
  mixed.fields.body = 1
  mixed.retainedBytes += 1
  mixed.bytesPerTest += 1
  assert.ok(validateMemoryRecord(mixed).some((p) => p.includes('must not mix')))
  const outer = owned()
  outer.retainedBytes += 1
  outer.bytesPerTest += 1
  assert.ok(validateMemoryRecord(outer).some((p) => p.includes('sums to')))
})

/**
 * Every named field must be NAMED in the problems when it holds any of `values`.
 *
 * Shared by the loops below rather than written twice: both are the same assertion -- one field,
 * one value a comparison would silently accept, one problem mentioning that field by name.
 */
function rejectsEvery(fields, values) {
  for (const value of values) {
    for (const field of fields) {
      const problems = validateMemoryRecord(valid({ [field]: value }))
      assert.ok(
        problems.some((problem) => problem.includes(`\`${field}\``)),
        `${field} = ${JSON.stringify(value)} was accepted: ${JSON.stringify(problems)}`,
      )
    }
  }
}

test('the baseline record validates clean, or every rejection below proves nothing', () => {
  assert.deepEqual(validateMemoryRecord(valid()), [])
  // The sums are the point of the baseline, so assert them here rather than trusting the helper.
  assert.equal(
    FIELDS.reduce((sum, field) => sum + valid().fields[field], 0),
    valid().retainedBytes,
  )
  assert.equal(
    CONTEXT_FIELDS.reduce((sum, field) => sum + valid().contextFields[field], 0),
    valid().fields.context,
  )
})

test('a non-object is rejected rather than indexed into', () => {
  for (const candidate of [null, undefined, 42, 'memory', []]) {
    const problems = validateMemoryRecord(candidate)
    assert.equal(problems.length, 1, `${JSON.stringify(candidate)} must be rejected`)
    assert.match(problems[0], /not an object/)
  }
})

test('the three literal fields must match exactly', () => {
  for (const [field, wrong] of [
    ['type', 'summary'], ['schemaVersion', '0.2'], ['phase', 'post-analysis'],
  ]) {
    const problems = validateMemoryRecord(valid({ [field]: wrong }))
    assert.ok(
      problems.some((problem) => problem.includes(`\`${field}\``)),
      `${field} = ${wrong} must be named in ${JSON.stringify(problems)}`,
    )
  }
})

test('every counted field rejects what a comparison would silently accept', () => {
  // `undefined <= threshold` is false; `-1 > ceiling` is false; `"3287" > 3287` is false; NaN
  // compares false against everything. All four would make a ratchet PASS.
  rejectsEvery(
    ['testsAnalyzed', 'filesAnalyzed', 'retainedBytes', 'bytesPerTest', 'declarationsRetained'],
    [undefined, -1, '3287', Number.NaN, 1.5, null],
  )
})

test('a missing member of either breakdown is a contract change, never a zero', () => {
  for (const [group, keys] of [['fields', FIELDS], ['contextFields', CONTEXT_FIELDS]]) {
    for (const key of keys) {
      const record = valid()
      delete record[group][key]
      const problems = validateMemoryRecord(record)
      assert.ok(
        problems.some((problem) => problem.includes(`\`${group}.${key}\``)),
        `${group}.${key} went missing unnoticed: ${JSON.stringify(problems)}`,
      )
    }
  }
})

test('an unknown member of either breakdown is retention nobody budgeted', () => {
  for (const group of ['fields', 'contextFields']) {
    const record = valid()
    record[group].newThing = 0
    const problems = validateMemoryRecord(record)
    assert.ok(
      problems.some((problem) => problem.includes('newThing')),
      `${group}.newThing was accepted: ${JSON.stringify(problems)}`,
    )
  }
})

test('bytesPerTest must actually be retainedBytes over testsAnalyzed', () => {
  // Individually well-typed, collectively impossible: no per-field predicate can catch this.
  const problems = validateMemoryRecord(valid({ testsAnalyzed: 2, declarationsRetained: 2 }))
  assert.ok(
    problems.some((problem) => problem.includes('`bytesPerTest`')),
    JSON.stringify(problems),
  )
})

test('a breakdown that does not cover its own total is rejected', () => {
  const short = valid()
  short.fields.body -= 1
  assert.ok(
    validateMemoryRecord(short).some((problem) => problem.includes('`fields` sums to')),
    'fields no longer summing to retainedBytes went unnoticed',
  )

  const shortContext = valid()
  shortContext.contextFields.hooks -= 1
  assert.ok(
    validateMemoryRecord(shortContext).some((problem) => problem.includes('`contextFields` sums to')),
    'contextFields no longer summing to fields.context went unnoticed',
  )
})

test('the remaining cross-field invariants each reject one impossible record', () => {
  const cases = [
    [{ declarationsRetained: 0 }, '`declarationsRetained`'],
    [{ filesAnalyzed: 2, testsAnalyzed: 1 }, '`filesAnalyzed`'],
  ]
  for (const [overrides, expected] of cases) {
    const problems = validateMemoryRecord(valid(overrides))
    assert.ok(
      problems.some((problem) => problem.includes(expected)),
      `${JSON.stringify(overrides)} was accepted: ${JSON.stringify(problems)}`,
    )
  }
})

test('retainedBytes ABOVE peakRssBytes is a valid record, not a double-count', () => {
  // The invariant this replaces rejected exactly this record. It was wrong: `retainedBytes` sums
  // collection CAPACITY and `peakRssBytes` is a high-water mark of RESIDENT pages, so reserved-
  // but-never-written capacity is counted by the first and not by the second. Neither number
  // bounds the other, and a gate that rejected this would have discarded a valid measurement.
  const record = valid()
  assert.ok(record.peakRssBytes > record.retainedBytes, 'the baseline must start peak-above')
  assert.deepEqual(validateMemoryRecord(valid({ peakRssBytes: 1 })), [])
  assert.deepEqual(validateMemoryRecord(valid({ peakRssBytes: 0 })), [])
  assert.deepEqual(
    validateMemoryRecord(valid({ peakRssBytes: record.retainedBytes - 1 })),
    [],
  )
})

test('both record notices must be present and non-empty, or the numbers get read wrong', () => {
  // Documented members of the contract that no per-number check covers: a rename, a drop, or a
  // `null` would leave the record printing plausibly with nothing on it saying how to read peak.
  const notices = ['sharedTextAttribution', 'peakRssNotice']
  rejectsEvery(notices, [undefined, null, '', 42, {}])
  for (const field of notices) {
    const record = valid()
    delete record[field]
    assert.ok(
      validateMemoryRecord(record).some((problem) => problem.includes(`\`${field}\``)),
      `a missing ${field} went unnoticed`,
    )
  }
})

test('bytesPerTest must be 0 when nothing was analyzed, not merely non-negative', () => {
  // The contract defines the zero-denominator case as 0. Unchecked, a per-test figure with no
  // tests under it -- two halves of two different runs -- validated clean.
  for (const bytesPerTest of [1, 3287, 999_999]) {
    const problems = validateMemoryRecord(valid({ testsAnalyzed: 0, bytesPerTest }))
    assert.ok(
      problems.some((problem) => problem.includes('`bytesPerTest`')),
      `bytesPerTest ${bytesPerTest} with 0 tests was accepted: ${JSON.stringify(problems)}`,
    )
  }
  // And 0 with 0 tests is the contract, so it must NOT be reported as a bytesPerTest problem.
  const clean = validateMemoryRecord(valid({ testsAnalyzed: 0, bytesPerTest: 0 }))
  assert.ok(
    !clean.some((problem) => problem.includes('`bytesPerTest`')),
    `the contract's own zero case was rejected: ${JSON.stringify(clean)}`,
  )
})

test('peakRssBytes may be null, because a platform without a reader is not a failure', () => {
  assert.deepEqual(validateMemoryRecord(valid({ peakRssBytes: null })), [])
  assert.ok(validateMemoryRecord(valid({ peakRssBytes: 'lots' })).length > 0)
})

test('an absent memory record is a silent success, not a run that used no memory', () => {
  assert.throws(
    () => selectMemoryRecord([{ type: 'summary' }, { type: 'finding' }]),
    /emitted no .*memory.* record/,
  )
  assert.throws(() => selectMemoryRecord([]), /emitted no/)
})

test('two memory records would make the graded phase depend on stream order', () => {
  assert.throws(
    () => selectMemoryRecord([valid(), valid({ phase: 'post-analysis' })]),
    /2 memory records/,
  )
})

test('selectMemoryRecord returns the one record and ignores everything else on the stream', () => {
  const record = valid()
  assert.equal(selectMemoryRecord([{ type: 'finding' }, record, { type: 'summary' }]), record)
})

// ---------------------------------------------------------------------------------------------
// The work-performed floor. Every case here is a run that measured nothing while looking healthy,
// which is the shape four of this repository's own gates have shipped.
// ---------------------------------------------------------------------------------------------

const PINNED = { tests: 10_000, analyzedFiles: 727 }
const measured = (overrides) => ({
  testsAnalyzed: 10_000, filesAnalyzed: 727, retainedBytes: 32_871_542, ...overrides,
})

test('a run that measured everything it was supposed to clears the floor', () => {
  assert.deepEqual(workPerformedProblems(measured({}), PINNED, 'gate'), [])
})

test('a collapsed extraction fails, even though it IMPROVES the governed ratio', () => {
  // This is the whole argument for an equality rather than a floor: fewer tests is a smaller
  // denominator, so `bytesPerTest` goes DOWN and every ceiling in both gates is cleared. A run
  // that extracted one test would otherwise be the best result this gate has ever seen.
  for (const testsAnalyzed of [0, 1, 9_999]) {
    const problems = workPerformedProblems(measured({ testsAnalyzed }), PINNED, 'gate')
    assert.ok(
      problems.some((problem) => problem.includes('pinned at 10000')),
      `${testsAnalyzed} analyzed test(s) was accepted: ${JSON.stringify(problems)}`,
    )
  }
  // And an equality catches the other direction too, which a floor would not: more tests than the
  // corpus can contain means the gate and the corpus disagree about what was measured.
  assert.ok(workPerformedProblems(measured({ testsAnalyzed: 10_001 }), PINNED, 'gate').length > 0)
})

test('collapsed discovery and zero retention each fail on their own', () => {
  assert.ok(
    workPerformedProblems(measured({ filesAnalyzed: 0 }), PINNED, 'gate')
      .some((problem) => problem.includes('Discovery')),
  )
  assert.ok(
    workPerformedProblems(measured({ retainedBytes: 0 }), PINNED, 'gate')
      .some((problem) => problem.includes('failed measurement, not a perfect one')),
  )
})

test('the label reaches every message, so a failure names which scale it came from', () => {
  const problems = workPerformedProblems(
    measured({ testsAnalyzed: 0, filesAnalyzed: 0, retainedBytes: 0 }),
    PINNED,
    'scale 100000',
  )
  assert.equal(problems.length, 3)
  for (const problem of problems) assert.match(problem, /^scale 100000: /)
})

// ---------------------------------------------------------------------------------------------
// Argument handling. Shared by both gates because both must REJECT the same things.
// ---------------------------------------------------------------------------------------------

test('an explicit binary path is accepted, and a missing one is named', () => {
  const argv = process.argv
  try {
    process.argv = ['node', 'check-memory.mjs', argv[0]] // argv[0] is a real executable
    assert.deepEqual(parseGateArguments('check-memory.mjs'), { binary: argv[0], keep: false })
    process.argv = ['node', 'check-memory.mjs', argv[0], '--keep']
    assert.equal(parseGateArguments('check-memory.mjs').keep, true)
    process.argv = ['node', 'check-memory.mjs', '/nonexistent/vitest-dedup']
    assert.throws(() => parseGateArguments('check-memory.mjs'), /no analyzer binary at/)
  } finally {
    process.argv = argv
  }
})

test('both gates reject an unknown flag rather than ignoring it', () => {
  // Spawned, because the rejection is a `process.exit(2)`. Ignoring an unknown flag is how
  // `--keep` becomes `--kepe` and a developer concludes the corpus is deleted on purpose. The
  // check runs before the binary is resolved, so this needs no release build.
  for (const gate of ['check-memory.mjs', 'check-memory-scaling.mjs']) {
    const result = spawnSync(process.execPath, [join(CHECKS, gate), '--kepe'], { encoding: 'utf8' })
    assert.equal(result.status, 2, `${gate} accepted --kepe: ${result.stdout}${result.stderr}`)
    assert.match(result.stderr, /--kepe/)
  }
})

test('--keep really keeps, and its absence really removes', () => {
  // The flag used to change only which sentence was printed: the corpus is cleaned at the START
  // of generation and was never removed after measurement, so it survived every run either way.
  // A message describing an effect the code does not have is the reason this asserts on the
  // filesystem rather than on the string.
  const root = mkdtempSync(join(tmpdir(), 'memory-corpus-'))
  writeFileSync(join(root, 'case.test.ts'), 'test("a", () => {})\n')

  const kept = discardCorpus(root, true)
  assert.ok(existsSync(root), `--keep removed the corpus anyway: ${kept}`)
  assert.match(kept, /kept/)

  const removed = discardCorpus(root, false)
  assert.equal(existsSync(root), false, `the corpus survived without --keep: ${removed}`)
  assert.match(removed, /removed/)

  // Removing what is already gone is how the `finally` behaves when generation itself threw.
  assert.doesNotThrow(() => discardCorpus(root, false))
})

test('both gates reject a second positional rather than measuring the first', () => {
  for (const gate of ['check-memory.mjs', 'check-memory-scaling.mjs']) {
    const result = spawnSync(process.execPath, [join(CHECKS, gate), 'a', 'b'], { encoding: 'utf8' })
    assert.equal(result.status, 2, `${gate} accepted two binaries`)
  }
})
