#!/usr/bin/env node
// The `--debug-memory` record: how to obtain one, how to refuse to believe one, and the three
// pieces of gate plumbing that would otherwise exist twice -- argument handling, the
// work-performed floor, and corpus cleanup.
//
// Shared by `check-memory.mjs` (the ratchet) and `check-memory-scaling.mjs` (the linearity test)
// so there is exactly one copy of the contract to review, or to get wrong -- the reason
// `run-analyzer.mjs` exists, and the reason it is used from here rather than re-implemented.
//
// ## The record
//
//   {"type":"memory","schemaVersion":"0.1","phase":"post-extraction",
//    "testsAnalyzed":…,"filesAnalyzed":…,"retainedBytes":…,"bytesPerTest":…,
//    "fields":{"body":…,"context":…,"suitePath":…,"title":…,"modifiers":…,"location":…,
//              "spine":…,"scalars":…},
//    "contextFields":{"hooks":…,"mockedModules":…,"fixtures":…,"imports":…,"scalars":…},
//    "declarationsRetained":…,"sharedTextAttribution":"…",
//    "peakRssBytes":…,"peakRssNotice":"…"}
//
// `fields` is disjoint and sums to `retainedBytes` exactly. `contextFields` is a further
// breakdown of `fields.context` and sums to *it*, so a consumer that sums both objects together
// double-counts `context`. Both sums are asserted below rather than assumed: they are the
// property that makes the attribution usable when a regression has to be located, and an
// attribution that silently stopped covering the total would still print plausibly.
//
// ## What is governed, and what is only reported
//
// The gate is **`bytesPerTest`**. `peakRssBytes` is reported beside it and never compared, and the
// record ships a `peakRssNotice` saying so, because peak is not the quantity anyone thinks it is:
//
//   - it scales with the corpus, so a number from a 1k run and a number from a 100k run are not
//     comparable, and a ratchet needs comparable numbers;
//   - it is a **process high-water mark**: the largest resident set the whole process reached up
//     to the moment the record was written, transients included. On macOS libmalloc does not
//     eagerly return freed per-file parser arenas, so peak stays high after the memory is gone.
//     Both external reviewers of the 947 MB -> 149 MB change read peak as though it were retained
//     bytes and drew the wrong conclusion about where the memory had gone.
//
// The two numbers are **not comparable in either direction**, and this gate deliberately asserts
// no ordering between them. `retainedBytes` counts collection *capacity*: a `Vec` that reserved
// 41,194 slots to hold 33,036 pays for all 41,194, and pages that were reserved and never written
// are not resident at all. So a capacity-counted total legitimately can exceed a high-water mark
// of resident pages. An earlier revision of this file rejected exactly that record, on the theory
// that peak bounds retained from above; it does not, and the rejection would have thrown away a
// valid measurement.
//
// Peak is a smoke alarm. `bytesPerTest` is the measurement.
//
// ## Validate before comparing
//
// `undefined < threshold` is `false`. A record that lost `bytesPerTest` to a rename, or whose
// `retainedBytes` arrived as a string, would sail through every ratchet in this repository and be
// recorded as the new floor. This project has shipped three variants of that bug (`undefined
// < 100`, `NaN === 0`, `-1 > threshold`), so every field is type-checked here, once, before any
// caller is allowed to compare it. `Number.isFinite` is not a validation: a negative
// `retainedBytes` is finite and clears every ceiling.
//
// The unknown-key checks are deliberate too. `fields` gaining a key is new retention that nobody
// budgeted; the breakdown sum would still balance and the total would still be compared, so
// without this the only symptom would be a bigger number with no name on it.
import { existsSync, rmSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { defaultBinary, runAnalyzerJsonl } from './run-analyzer.mjs'

/** The repository root, two levels up from `scripts/check/lib`. */
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..', '..')

/** Pinned. A schema bump must be a deliberate edit here, next to the numbers it invalidates. */
export const SCHEMA_VERSION = '0.1'
/** The one phase that retains IR. `src/memory.rs` explains why there is no post-analysis record. */
export const PHASE = 'post-extraction'
/** Exactly these. `spine` is the `Vec<TestCase>` backing array; `scalars` is the zero bucket. */
export const FIELDS = [
  'body', 'context', 'suitePath', 'title', 'modifiers', 'location', 'spine', 'scalars',
]
/**
 * A breakdown of `fields.context`, summing to it -- NOT further members of `fields`.
 *
 * `moduleSetup` arrived with `Context::module_setup` and was not added here, so this gate refused
 * to measure anything for three releases: its own unknown-member check fired, exactly as designed
 * ("a new member is retention nobody budgeted"). Nobody saw it because CI was failing on a billing
 * block at the time, every job in seconds, so a real red read as part of the noise. That is the
 * cost of a dark CI stated precisely: the gate worked, the signal was delivered, and there was no
 * one listening.
 *
 * It also carries `config_scope`'s bytes rather than having a bucket of its own -- see
 * `ContextFieldBytes::module_setup` in `src/memory.rs`, whose doc records that choice. So this list
 * is still exhaustive over what the binary emits, which is what the unknown-member check enforces.
 */
export const CONTEXT_FIELDS = [
  'hooks', 'mockedModules', 'fixtures', 'imports', 'moduleSetup', 'scalars',
]

/** Disjoint owned-engine populations; their sum is fields.engine, not additional retention. */
export const ENGINE_FIELDS = [
  'cases', 'suites', 'hooks', 'sites', 'blind', 'modules', 'exports', 'scalars',
]

const isCount = (value) => Number.isInteger(value) && value >= 0
const sum = (object, keys) => keys.reduce((total, key) => total + object[key], 0)

/**
 * Type-checks a `--debug-memory` record and returns every problem it has.
 *
 * Pure, so every failure mode is unit-testable without a binary -- see `memory-record.test.mjs`,
 * which is where the "what change would make this fail?" question gets its answer for each branch.
 *
 * @param {unknown} record - the candidate, straight off the jsonl stream.
 * @returns {string[]} empty means every field is present, correctly typed, and mutually
 *   consistent. A non-empty result means the value must NOT be compared against anything.
 */
export function validateMemoryRecord(record) {
  if (record === null || typeof record !== 'object' || Array.isArray(record)) {
    return [`the memory record is ${JSON.stringify(record)}, not an object`]
  }
  const problems = []
  const literal = (field, expected) => {
    if (record[field] !== expected) {
      problems.push(
        `\`${field}\` is ${JSON.stringify(record[field])}, expected ${JSON.stringify(expected)}. ` +
          'This gate and the binary disagree about the record contract, which is a failed ' +
          'measurement rather than a result.',
      )
    }
  }
  literal('type', 'memory')
  literal('schemaVersion', SCHEMA_VERSION)
  literal('phase', PHASE)

  for (const field of [
    'testsAnalyzed', 'filesAnalyzed', 'retainedBytes', 'bytesPerTest', 'declarationsRetained',
  ]) {
    if (!isCount(record[field])) {
      problems.push(
        `\`${field}\` is ${JSON.stringify(record[field])}, not a non-negative integer. A ratchet ` +
          'comparison against that value returns false whatever the threshold, so it is rejected ' +
          'rather than compared.',
      )
    }
  }
  // Reported, never compared -- but a *wrong-typed* peak would still be printed, and a printed
  // number nobody validated is how peak got mistaken for retained bytes twice already.
  if (record.peakRssBytes !== null && !isCount(record.peakRssBytes)) {
    problems.push(
      `\`peakRssBytes\` is ${JSON.stringify(record.peakRssBytes)}, neither null nor a ` +
        'non-negative integer.',
    )
  }

  // The two notices. Both are part of the record contract and neither is a number, so nothing
  // above would have noticed one going missing, getting renamed, or arriving as `null` -- the
  // same hole the unknown-key check below was built to close, left open on the two keys that
  // carry the *reading instructions* for every number beside them. `peakRssNotice` in particular
  // is the only thing on the record that says peak is not comparable to retained bytes; a record
  // that silently stopped carrying it would still print plausibly and still be read wrong.
  for (const field of ['sharedTextAttribution', 'peakRssNotice']) {
    if (typeof record[field] !== 'string' || record[field].length === 0) {
      problems.push(
        `\`${field}\` is ${JSON.stringify(record[field])}, not a non-empty string. It is how the ` +
          'numbers beside it are meant to be read, so an absent one is a contract change rather ' +
          'than a cosmetic omission.',
      )
    }
  }

  const owned = Object.hasOwn(record, 'engineFields') ||
    Object.hasOwn(record, 'engineMemoryNotice') ||
    (record.fields !== null && typeof record.fields === 'object' && Object.hasOwn(record.fields, 'engine'))
  const fieldKeys = owned && record.fields && Object.hasOwn(record.fields, 'engine')
    ? [...FIELDS, 'engine'] : FIELDS
  if (owned && (typeof record.engineMemoryNotice !== 'string' || record.engineMemoryNotice.length === 0)) {
    problems.push('`engineMemoryNotice` must explain the owned retained-memory estimate')
  }
  const breakdowns = [['fields', fieldKeys], ['contextFields', CONTEXT_FIELDS]]
  if (owned) breakdowns.push(['engineFields', ENGINE_FIELDS])
  for (const [name, keys] of breakdowns) {
    const group = record[name]
    if (group === null || typeof group !== 'object' || Array.isArray(group)) {
      problems.push(`\`${name}\` is ${JSON.stringify(group)}, not an object`)
      continue
    }
    for (const key of keys) {
      if (!isCount(group[key])) {
        problems.push(
          `\`${name}.${key}\` is ${JSON.stringify(group[key])}, not a non-negative integer. The ` +
            'breakdown is how a regression is attributed, so a missing member is a contract ' +
            'change and never a zero.',
        )
      }
    }
    const extra = Object.keys(group).filter((key) => !keys.includes(key))
    if (extra.length > 0) {
      problems.push(
        `\`${name}\` carries ${extra.map((key) => `\`${key}\``).join(', ')}, which this gate does ` +
          'not know about. A new member is retention nobody budgeted: the sums would still ' +
          'balance and the total would still be compared, so the only symptom would be a bigger ' +
          `number with no name on it. Add it to ${name === 'fields' ? 'FIELDS' : name === 'engineFields' ? 'ENGINE_FIELDS' : 'CONTEXT_FIELDS'}` +
          ' with the measurement it changes.',
      )
    }
  }

  // Cross-field invariants. None is expressible as a per-field predicate, and each one catches a
  // record that is individually well-typed and collectively impossible.
  if (problems.length > 0) return problems

  // `bytesPerTest` is checked at BOTH ends of the branch. The contract defines it as the
  // truncated quotient when there is a denominator and as exactly `0` when there is not, and the
  // zero case is the one that matters here: a record claiming to have analyzed nothing while
  // carrying a nonzero per-test figure is a record whose two halves came from different runs, and
  // leaving that arm unchecked accepted any non-negative integer at all.
  const derived =
    record.testsAnalyzed > 0 ? Math.floor(record.retainedBytes / record.testsAnalyzed) : 0
  if (record.bytesPerTest !== derived) {
    problems.push(
      record.testsAnalyzed > 0
        ? `\`bytesPerTest\` is ${record.bytesPerTest} but \`retainedBytes\`/\`testsAnalyzed\` ` +
          `truncates to ${derived}. The two disagree, so one of them is measuring something other ` +
          'than what it is named -- and the gate ratchets on the former.'
        : `\`bytesPerTest\` is ${record.bytesPerTest} with \`testsAnalyzed\` at 0, where the ` +
          'contract says 0. A per-test figure with no tests under it did not come from this run.',
    )
  }
  if (owned) {
    if (sum(record.engineFields, ENGINE_FIELDS) !== (record.fields.engine ?? 0)) {
      problems.push('`engineFields` must sum to `fields.engine`')
    }
    if (FIELDS.some((key) => record.fields[key] !== 0)) {
      problems.push('owned memory records must not mix legacy field attribution with `fields.engine`')
    }
  }
  const fieldTotal = sum(record.fields, fieldKeys)
  if (fieldTotal !== record.retainedBytes) {
    problems.push(
      `\`fields\` sums to ${fieldTotal} but \`retainedBytes\` is ${record.retainedBytes}. The ` +
        'breakdown is documented as disjoint and exhaustive; one that does not cover its own ' +
        'total cannot locate a regression, which is the only thing it is for.',
    )
  }
  const contextTotal = sum(record.contextFields, CONTEXT_FIELDS)
  if (contextTotal !== record.fields.context) {
    problems.push(
      `\`contextFields\` sums to ${contextTotal} but \`fields.context\` is ` +
        `${record.fields.context}. The context breakdown is documented as summing to the context ` +
        'field it refines.',
    )
  }
  if (record.declarationsRetained < record.testsAnalyzed) {
    problems.push(
      `\`declarationsRetained\` (${record.declarationsRetained}) is below \`testsAnalyzed\` ` +
        `(${record.testsAnalyzed}); it counts suites and benchmarks too, so it cannot be smaller.`,
    )
  }
  if (!owned && record.filesAnalyzed > record.testsAnalyzed) {
    problems.push(
      `\`filesAnalyzed\` (${record.filesAnalyzed}) exceeds \`testsAnalyzed\` ` +
        `(${record.testsAnalyzed}); a file with no test is not in the report.`,
    )
  }
  // There is deliberately NO ordering invariant between `peakRssBytes` and `retainedBytes`. See
  // the header: peak is a high-water mark of *resident* pages and retained is a sum of collection
  // *capacity*, so neither bounds the other and a record with `retainedBytes` above
  // `peakRssBytes` is valid. This file used to reject that shape.
  return problems
}

/**
 * Picks the single memory record out of a jsonl stream.
 *
 * Zero records is the failure this whole file exists for: `--debug-memory` accepted and then
 * producing nothing is a silent success, so it is an error with its own message rather than an
 * absent comparison. More than one is equally wrong -- a second record would mean the caller is
 * grading one phase and reporting another, decided by stream order.
 */
export function selectMemoryRecord(records) {
  const found = records.filter((record) => record && record.type === 'memory')
  if (found.length === 0) {
    throw new Error(
      'the analyzer emitted no `"type":"memory"` record. `--debug-memory` was accepted and ' +
        'produced nothing, which is the silent-success shape this gate exists to catch -- not ' +
        'a run that used no memory.',
    )
  }
  if (found.length > 1) {
    throw new Error(
      `the analyzer emitted ${found.length} memory records (phases ` +
        `${found.map((record) => JSON.stringify(record.phase)).join(', ')}). This gate grades one ` +
        `phase (${PHASE}); which one it graded must not depend on stream order.`,
    )
  }
  return found[0]
}

/**
 * Runs the analyzer over `root` with `--debug-memory` and returns its validated memory record.
 *
 * `runAnalyzerJsonl` is the single place that knows the exit-code contract -- 0 and 1 are results,
 * anything else is an operational failure -- and re-implementing the spawn here to add one flag is
 * exactly the duplication its header was written about.
 *
 * @param {string} binary - path to the vitest-dedup binary.
 * @param {string} root - corpus directory to analyze.
 * @returns {{record: object, elapsedMs: number, stderr: string}}
 * @throws {Error} if the record is absent, duplicated, or fails validation. A caller never sees an
 *   unvalidated number, which is the point: the comparison cannot be reached with a value the
 *   comparison would silently accept.
 */
export function measureMemory(binary, root) {
  const started = Date.now()
  const { records, stderr } = runAnalyzerJsonl(binary, [root, '--debug-memory'])
  const elapsedMs = Date.now() - started

  const record = selectMemoryRecord(records)
  const problems = validateMemoryRecord(record)
  if (problems.length > 0) {
    throw new Error(
      `the memory record from ${binary} does not satisfy the \`--debug-memory\` contract:\n` +
        problems.map((problem) => `    - ${problem}`).join('\n'),
    )
  }
  return { record, elapsedMs, stderr }
}

/**
 * The arguments both memory gates take: an optional binary path and an optional `--keep`.
 *
 * Shared rather than written twice because the two gates must REJECT the same things. An unknown
 * flag silently ignored is how `--keep` becomes `--kepe` and a developer concludes the corpus is
 * being deleted on purpose; a second positional silently ignored is how a typo'd binary path gets
 * measured against the real binary and reported as a pass.
 *
 * @param {string} usage - the script's own name, for the diagnostic.
 * @returns {{binary: string, keep: boolean}}
 */
export function parseGateArguments(usage) {
  const argv = process.argv.slice(2)
  const positional = argv.filter((flag) => !flag.startsWith('--'))
  const unknown = argv.filter((flag) => flag.startsWith('--') && flag !== '--keep')
  if (unknown.length > 0 || positional.length > 1) {
    console.error(`usage: ${usage} [binary] [--keep] (got ${argv.join(' ') || 'nothing'})`)
    process.exit(2)
  }
  const binary = positional[0] ?? defaultBinary({ root: ROOT })
  if (!existsSync(binary)) {
    throw new Error(`no analyzer binary at ${binary}. Run \`cargo build --release\` first.`)
  }
  return { binary, keep: argv.includes('--keep') }
}

/**
 * Applies `--keep` to the generated corpus: removed without it, left in place with it.
 *
 * The third piece of plumbing both gates need identically, here for the same reason the other two
 * are. It also exists because `--keep` did not previously do anything: the corpus is cleaned at
 * the START of generation and was never removed afterwards, so it survived every run either way
 * and the flag changed only which sentence got printed. A flag whose message describes an effect
 * it does not have is the same defect as a validator asserting an invariant that does not hold --
 * both are a claim nobody measured.
 *
 * Made real rather than re-worded because the removal is free: the tree is regenerated
 * byte-identically from a fixed seed (`lib/memory-corpus.mjs`), so nothing is lost by deleting it
 * and the 100k scale is ~71 MB that is otherwise left behind by every run, passing or failing.
 * `--keep` is how a failed run is inspected, and re-running the generator directly is the other
 * way back to the identical tree.
 *
 * Callers invoke this from a `finally`, so an exception mid-measurement cleans up too -- with the
 * `process.exit` calls kept OUTSIDE that block, because `process.exit` does not run `finally`.
 *
 * @param {string} root - the corpus directory, or the parent holding several scales of it.
 * @param {boolean} keep - the flag as `parseGateArguments` parsed it.
 * @returns {string} what happened, for the caller to print. Returned rather than logged so the
 *   behaviour is assertable without capturing stdout.
 */
export function discardCorpus(root, keep) {
  if (keep) return `corpus kept at ${root}, as --keep asked`
  rmSync(root, { recursive: true, force: true })
  return (
    `corpus at ${root} removed; it regenerates byte-identically from the seed, and --keep ` +
    'leaves it in place for inspection'
  )
}

/**
 * The work-performed floor: every way a run can have measured nothing while looking healthy.
 *
 * A run that analyzed nothing has excellent memory, and that is the silent-success shape four of
 * this repository's own gates have shipped. Both gates apply this, so it lives here once.
 *
 * `testsAnalyzed` is an EQUALITY, not a floor with slack, and that is the substance of it: the
 * corpus is byte-identical and its digest is pinned, so the extracted count is a constant. A floor
 * would accept a collapsed extraction, and a collapsed extraction shrinks the denominator of
 * `bytesPerTest` -- so it would make the governed number look BETTER, which is the worst possible
 * direction for a failure to push a gate.
 *
 * @param {object} record - a record already through `validateMemoryRecord`.
 * @param {{tests: number, analyzedFiles: number}} pinned - the corpus's pinned properties.
 * @param {string} label - how to name this measurement in a message.
 * @returns {string[]} one message per violated floor; empty means the work really happened.
 */
export function workPerformedProblems(record, pinned, label) {
  const problems = []
  if (record.testsAnalyzed !== pinned.tests) {
    problems.push(
      `${label}: analyzed ${record.testsAnalyzed} test(s); this corpus is pinned at ` +
        `${pinned.tests}. A smaller test count is a smaller denominator, so extraction ` +
        'collapsing would make bytes-per-test look BETTER. That is why this is an equality.',
    )
  }
  if (record.filesAnalyzed < pinned.analyzedFiles) {
    problems.push(
      `${label}: analyzed ${record.filesAnalyzed} file(s), pinned at ${pinned.analyzedFiles}. ` +
        'Discovery over the generated corpus has collapsed.',
    )
  }
  if (record.retainedBytes <= 0) {
    problems.push(
      `${label}: retained ${record.retainedBytes} byte(s) after extracting ` +
        `${record.testsAnalyzed} test(s), which cannot be true. A zero here is a failed ` +
        'measurement, not a perfect one.',
    )
  }
  return problems
}

/** `87962574` -> `83.9 MB`. Reporting only. */
export const mb = (bytes) => `${(bytes / 1024 / 1024).toFixed(1)} MB`
