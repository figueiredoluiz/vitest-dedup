#!/usr/bin/env node
// Retained-memory ratchet. Measures **bytes retained per test** over a deterministic synthetic
// corpus and fails when the number gets worse than the recorded one.
//
// Usage: `node scripts/check/check-memory.mjs [binary] [--keep]`
//
// ## The destination
//
// The tool targets legacy suites, so the dimension that matters is test count, not file count and
// not lines. The TARGET is **1,000,000 tests within 2 GB**, which is `TARGET_BYTES_PER_TEST` =
// 2048 bytes retained per test. It is the destination, not the limit anything fails on today.
//
// Three numbers, kept apart on purpose, because conflating them is how a target gets quoted as a
// limit: the MEASURED figure (what the last run of this gate printed), the ENFORCED
// `MAX_BYTES_PER_TEST` below (plus the across-scales ceiling in `check-memory-scaling.mjs`), and
// the TARGET. Only the middle one fails a build. The enforced value sits under the target on this
// corpus; the ratchet still governs so the number cannot creep back up unnoticed.
//
// This is therefore a ratchet: `MAX_BYTES_PER_TEST` is the value measured when the gate was
// written, the gate fails when the number gets worse, and tightening it is a one-line edit.
// `TARGET_BYTES_PER_TEST` is recorded separately so the ratchet has a direction and so nobody
// mistakes the current value for the goal.
//
// Nothing measured this before. The 947 MB -> 149 MB change was a six-fold improvement with no
// gate behind it, and the first `String` added to `src/model.rs` would have given it back
// silently.
//
// ## Bytes per test, not peak RSS
//
// Peak is printed as a sanity figure and never compared. It is a process high-water mark: it
// scales with the corpus, so runs at different N are not comparable and a ratchet needs comparable
// numbers; and on macOS it absorbs freed per-file transients, so it stays high after the memory is
// gone. It is comparable to `retainedBytes` in neither direction either -- retained counts
// reserved capacity and reserved-but-unwritten pages are not resident -- so no ordering between
// the two is asserted anywhere. Both external reviewers of the memory change reasoned from peak as
// though it were retained bytes. See `lib/memory-record.mjs`.
//
// ## Why a synthetic corpus
//
// A per-test constant cannot be separated from fixed process cost over `fixtures/` (~300 tests),
// and the pinned real repositories in `run-external-corpus.mjs` need network and are not
// byte-stable under a clone, so the denominator would drift under the gate. `lib/memory-corpus.mjs`
// generates a fixed tree from a seed instead, and the digest below is asserted rather than
// assumed: if the corpus changes, this gate fails and whoever changed it re-pins the number in
// the same commit.
//
// ## This gate asserts a floor on work performed
//
// A run that analyzed nothing has excellent memory. That is the failure shape four of this
// repository's own gates have shipped, so:
//
//   - the memory record must EXIST. `--debug-memory` accepted and emitting nothing is a silent
//     success, not a zero (`selectMemoryRecord`).
//   - `testsAnalyzed` must equal the exact count the corpus is pinned to produce -- not a floor
//     with slack, because the corpus is byte-identical and the answer is a constant. Extraction
//     collapsing to a fraction is then a failure rather than a smaller denominator that makes
//     the ratio look better.
//   - `filesAnalyzed` and `retainedBytes` carry their own floors.
//
// ## Validate before comparing
//
// Every field is type-checked in `lib/memory-record.mjs` BEFORE anything here compares it,
// because `undefined < MAX_BYTES_PER_TEST` is `false` and a renamed field would pass this ratchet
// and then be recorded as the new one.
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { CORPUS_SCALES, corpusRoot, generateCorpus } from './lib/memory-corpus.mjs'
import {
  CONTEXT_FIELDS,
  FIELDS,
  discardCorpus,
  mb,
  measureMemory,
  parseGateArguments,
  workPerformedProblems,
} from './lib/memory-record.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

// ---------------------------------------------------------------------------------------------
// THE RATCHET. To tighten: lower MAX_BYTES_PER_TEST. That is the whole workflow. Raising it is
// the ratchet running backwards and needs the reason in the commit message.
// ---------------------------------------------------------------------------------------------
/** The owner's bar: 1M tests inside 2 GB. */
const TARGET_BYTES_PER_TEST = 2048
/**
 * Measured over the 10,000-test synthetic corpus, at the padded path length
 * `lib/memory-corpus.mjs` normalizes to. This corpus's mix is its own, and the ratchet governs
 * THIS number against ITS history; it is not a figure for any real repository.
 *
 * This, with TOLERANCE, is the number a contributor is actually held to at this scale. It is the
 * value this gate measured when last re-recorded: `Context::imports` holds one shared `Arc<str>`
 * per import identity per file, and the per-test import and fixture vectors are shrunk after
 * dedup, so a body's reference count no longer dictates how many slots it retains. `hooks` is
 * now the largest context field.
 */
const MAX_BYTES_PER_TEST = 1942

/**
 * Drift allowance on the ratchet, as a fraction.
 *
 * The measurement should be bit-identical run to run: retained bytes are computed from capacities
 * over a byte-identical tree at a path normalized to a constant length, and `size_of::<TestCase>`
 * is the same on every 64-bit target. That prediction held where it was weakest: the first Linux
 * CI run measured 3287 bytes/test -- the exact figure this ratchet was pinned to on macOS. 2%
 * remains not for platforms but for the toolchain: a rustc that changes `String`/`Vec` growth
 * policy moves every capacity at once, and a gate that goes red on arrival gets routed around. If
 * it ever needs more than 2%, find out why -- do not widen it.
 */
const TOLERANCE = 0.02

/** The scale the gate runs at. Big enough for fixed process cost to amortize, ~4s to analyze. */
const GATE_TESTS = 10_000

/** Everything the corpus at that scale is pinned to. */
const PINNED = CORPUS_SCALES[GATE_TESTS]

const pct = (value) => `${(value * 100).toFixed(1)}%`

/**
 * The attribution a regression needs: where the retained bytes are, per test and as a share.
 *
 * The breakdown is asserted to be exhaustive in `lib/memory-record.mjs`, so there is deliberately
 * no "(other)" row -- a bucket that did not add up would have failed validation rather than being
 * papered over with a remainder.
 *
 * Skipped outright when nothing was analyzed. Every cell would read `NaN bytes/test  NaN%`, and a
 * table of NaNs printed beside a failure message is an invitation to read one of them as a
 * measurement.
 */
function printBreakdown(record) {
  if (record.testsAnalyzed === 0) {
    console.log('\n(no per-field breakdown: nothing was analyzed, so every figure would be NaN)')
    return
  }
  const row = (name, bytes) =>
    console.log(
      `  ${name.padEnd(12)} ${String(Math.round(bytes / record.testsAnalyzed)).padStart(6)} ` +
        `bytes/test  ${pct(bytes / record.retainedBytes).padStart(6)}  ${mb(bytes)}`,
    )
  console.log('\nretained bytes by field:')
  for (const field of FIELDS) row(field, record.fields[field])
  console.log('\n  ...of which `context`, the field hooks and mocks are retained in:')
  for (const field of CONTEXT_FIELDS) row(field, record.contextFields[field])
}

/**
 * Generate the pinned memory corpus, enforce retained-byte limits and clean it unless --keep
 * was requested.
 */
function main() {
  const { binary, keep } = parseGateArguments('check-memory.mjs')
  const failures = []

  // Padded to a constant absolute length: retained bytes include canonical paths, so an
  // un-normalized corpus path would make this gate's number depend on where the repository is
  // checked out. See PATH_BUDGET in `lib/memory-corpus.mjs`.
  const corpusDirectory = corpusRoot(join(ROOT, '.memory-corpus'), GATE_TESTS)
  // The corpus is removed in the `finally` unless `--keep` was passed, so a run that throws
  // mid-measurement cleans up as well as one that completes. `process.exit` below is deliberately
  // outside this block: it does not run `finally`.
  try {
    const corpus = generateCorpus({ root: corpusDirectory, tests: GATE_TESTS })
    // Determinism, asserted rather than claimed. The bytes-per-test number below is only a
    // measurement if the thing it was measured over is fixed; a generator edit that moves the
    // corpus must move this digest, fail here, and be re-pinned alongside the threshold.
    if (corpus.digest !== PINNED.digest) {
      throw new Error(
        `the generated corpus has digest ${corpus.digest}, pinned is ${PINNED.digest}. The ` +
          'corpus changed, so the recorded bytes-per-test was measured over a different tree. ' +
          'Re-pin both in the same commit, or revert the generator change.',
      )
    }

    console.log(
      `corpus: ${corpus.tests} tests in ${corpus.files} generated file(s), ` +
        `${mb(corpus.bytes)} of source, sha256 ${corpus.digest.slice(0, 16)}...`,
    )

    const { record, elapsedMs } = measureMemory(binary, corpusDirectory)

    // ---- the work-performed floor ----
    // A run that analyzed nothing has excellent memory. Shared with the scaling test, and the
    // reasoning for the EQUALITY on `testsAnalyzed` lives with it in `lib/memory-record.mjs`.
    failures.push(...workPerformedProblems(record, PINNED, `the ${GATE_TESTS}-test corpus`))

    // ---- the ratchet ----
    // Reached only with a record every field of which has been type-checked in
    // `lib/memory-record.mjs`; `measureMemory` throws rather than returning an unvalidated one.
    const bytesPerTest = record.bytesPerTest
    if (bytesPerTest > MAX_BYTES_PER_TEST * (1 + TOLERANCE)) {
      failures.push(
        `retained ${bytesPerTest} bytes per test, above the recorded ${MAX_BYTES_PER_TEST} ` +
          `(+${(TOLERANCE * 100).toFixed(0)}% tolerance). Memory got worse. The per-field ` +
          'breakdown below says where; fix it, or -- if the increase is justified -- raise ' +
          'MAX_BYTES_PER_TEST with the reason in the commit message.',
      )
    }

    const projected = bytesPerTest * 1_000_000
    console.log(
      `\n${record.testsAnalyzed} tests (${record.declarationsRetained} declarations retained, ` +
        `suites included) across ${record.filesAnalyzed} file(s) in ` +
        `${(elapsedMs / 1000).toFixed(1)}s\n` +
        `  retained      ${mb(record.retainedBytes)} (${bytesPerTest} bytes/test)\n` +
        `  of which      ${Math.round((record.retainedBytes - record.fields.spine) / record.testsAnalyzed)} ` +
        `bytes/test of IR and ${Math.round(record.fields.spine / record.testsAnalyzed)} of Vec spine ` +
        `(${Math.round(record.fields.spine / record.declarationsRetained)} bytes per retained ` +
        'declaration; the spine is `capacity * size_of::<TestCase>`, so it steps rather than ' +
        'grows -- see check-memory-scaling.mjs)\n' +
        `  ratchet       ${MAX_BYTES_PER_TEST} bytes/test (+${(TOLERANCE * 100).toFixed(0)}% tolerance)\n` +
        `  target        ${TARGET_BYTES_PER_TEST} bytes/test (1M tests in 2 GB), ` +
        `${(bytesPerTest / TARGET_BYTES_PER_TEST).toFixed(1)}x away\n` +
        `  at 1M tests   ${(projected / 1024 ** 3).toFixed(2)} GB projected\n` +
        `  peak RSS      ${record.peakRssBytes === null ? 'unavailable on this platform' : mb(record.peakRssBytes)}` +
        ' (SANITY FIGURE, NOT THE GATE -- a process high-water mark, transients included,' +
        ' comparable to neither the ratchet nor retained bytes)',
    )

    printBreakdown(record)
  } finally {
    console.log(`\n(${discardCorpus(corpusDirectory, keep)})`)
  }

  if (failures.length > 0) {
    console.error('\nFAIL: memory gate')
    for (const failure of failures) console.error(`  ${failure}`)
    process.exit(1)
  }
  console.log('\nok: the corpus was fully analyzed, and retention is at or below its recorded ratchet.')
}

if (resolve(process.argv[1] ?? '') === resolve(fileURLToPath(import.meta.url))) {
  try {
    main()
  } catch (error) {
    console.error(`\nFAIL: memory gate could not complete a measurement.\n  ${error.message}`)
    process.exit(1)
  }
}
