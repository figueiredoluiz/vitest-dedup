#!/usr/bin/env node
// Retained memory must be LINEAR in test count. Measured at 1k, 10k and 100k tests.
//
// Usage: `node scripts/check/check-memory-scaling.mjs [binary] [--keep]`
//
// ## Why this exists separately from `check-memory.mjs`
//
// Every memory measurement this project has is a single point: 3,642 bytes per test over 24,147
// tests. One point fixes a constant and says nothing about an exponent, so anything super-linear
// is invisible today -- and super-linear is the failure that kills the customer this tool is for.
// A legacy monorepo is where the test count is large; a per-test cost that grows with the test
// count turns a 3.6 KB/test tool into an unusable one somewhere between the corpus it was measured
// on and the corpus it was built for, with no warning in between.
//
// `check-memory.mjs` bounds the constant. This bounds the exponent. Neither implies the other: a
// tool can be perfectly linear at 40 KB/test, and a tool can be 200 bytes/test at 1k and
// quadratic above it.
//
// ## The assertion, and why it is this one
//
// **IR bytes per test must not grow by more than `MAX_GROWTH_PER_DECADE` from one scale to the
// next**, where "IR bytes" is `retainedBytes` minus `fields.spine`. Linear retention means that
// quantity is constant in N, so a growth RATIO between consecutive decades is the direct
// statement of the property.
//
// Two decisions inside that sentence, and both came out of measuring rather than reasoning.
//
// **Why the spine is excluded, and why that is not a loophole.** `fields.spine` is
// `Vec::capacity() * size_of::<TestCase>()` for the one vector holding the IR. Capacity is the
// allocator's doubling overshoot, so the spine is a STEP function of N, not a curve: measured
// across seven scales it took exactly three values -- 533, 426 and 684 bytes per retained
// declaration -- depending only on where N fell between two doublings. Including it would inject
// a 1.6x sawtooth into a ratio whose whole job is to detect a trend, forcing the bound out to
// ~1.25 and blinding it to everything milder than that. It is not a loophole because a `Vec`
// spine cannot be super-linear by construction (capacity is bounded by a constant multiple of
// length), and because its height is governed anyway: `MAX_BYTES_PER_TEST_ANY_SCALE` below
// bounds the TOTAL, spine included, at every scale.
//
// With the spine out, the measured IR figure is 2722 / 2819 / 2819 bytes per test at 1k / 10k /
// 100k -- flat to 3.5% across two decades. That is what makes a 1.10 bound possible, and a 1.10
// bound is what makes this test sensitive.
//
// **Why a ratio and not a fitted slope.** Three alternatives were weighed:
//
//   - **A fitted log-log slope.** With three points it has one degree of freedom and is dominated
//     by the endpoints, and its tolerance has to absorb the fixed process cost that legitimately
//     makes the 1k point the odd one out. The ratio absorbs that for free, because the bound is
//     one-sided: amortizing fixed cost makes the ratio smaller than 1 and the gate does not care.
//     The fitted exponent is still computed and PRINTED, as the headline number a human wants; it
//     is not compared, because a second assertion carrying the same information as the first is a
//     test that cannot independently fail.
//   - **Absolute retained bytes against a per-scale table.** Three ratchets wearing a scaling
//     test's clothes: red on any constant-factor change rather than on a shape change.
//   - **Comparing only the endpoints, 1k against 100k.** It averages a break away. A structure
//     flat to 10k and quadratic after it -- a pairwise index built past a threshold, a hash map
//     degrading -- is the realistic shape, and only a per-decade bound names the decade.
//
// **What would trip it.** Quadratic retention gives a per-decade ratio of 10, a hundred times the
// permitted excess. `N * sqrt(N)` gives 3.16. `N log N`, the mildest super-linearity anyone ships
// by accident, gives `log(10k)/log(1k)` = 1.33 and fails with room to spare. Against the 3.5%
// spread actually observed, the bound has 3x headroom in the other direction.
//
// The second assertion is independent of the first: `bytesPerTest` -- the real governed number,
// spine included -- must clear `MAX_BYTES_PER_TEST_ANY_SCALE` at every scale. The ratio bounds
// the shape of the curve and says nothing about its height; the ceiling bounds the height and
// says nothing about the shape. Either can fail while the other passes. That ceiling is
// deliberately looser than `check-memory.mjs`'s ratchet, and for a stated reason rather than for
// comfort: at a FIXED N the spine is a constant and the ratchet can be near-zero-slack, while
// across scales it carries the doubling sawtooth described above.
//
// ## Runtime, and where this runs
//
// About 40 seconds of analysis on an M-series laptop -- 0.3s, 3.7s and 35s at the three scales --
// plus 3s to generate 72 MB of corpus. That is cheap enough to run per pull request, and it does,
// beside `check-memory.mjs`, on a release build that shares `Swatinem/rust-cache` with the corpus
// and recall jobs. The 100k scale peaks around 436 MB of RSS, well inside a GitHub runner.
//
// ## Floors on work performed, at every scale
//
// A scaling test is unusually good at silent success: a run that extracted nothing at 100k has
// wonderful memory AND a wonderful ratio. Each scale therefore asserts the corpus digest, the
// exact test count, and the file floor before its number is allowed into the fit -- see
// `lib/memory-corpus.mjs` and `lib/memory-record.mjs`.
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { CORPUS_SCALES, corpusRoot, generateCorpus } from './lib/memory-corpus.mjs'
import {
  discardCorpus,
  mb,
  measureMemory,
  parseGateArguments,
  workPerformedProblems,
} from './lib/memory-record.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

const SCALES = [1_000, 10_000, 100_000]

// ---------------------------------------------------------------------------------------------
// THE ASSERTIONS. See the header for why these two and not a fitted slope.
// ---------------------------------------------------------------------------------------------
/**
 * How much IR bytes per test may grow across one decade of N. Observed 2026-09-19: 1.035 from 1k
 * to 10k and 1.000 from 10k to 100k, so 1.10 is 3x the observed spread and still far below the
 * 1.33 an `N log N` structure produces or the 10 a quadratic one does.
 */
const MAX_GROWTH_PER_DECADE = 1.1

/**
 * Ceiling on TOTAL bytes per test -- spine included -- at every scale. Measured maximum across
 * the three scales was 2313 (at 100k, where the `Vec` capacity overshoot is worst).
 *
 * Looser than `check-memory.mjs`'s ratchet on purpose: that gate measures one fixed N, where the
 * spine is a constant, so it can be near-zero-slack. This one spans three, and the spine's
 * doubling sawtooth is worth about 300 bytes per test between the best and worst scale. The
 * growth bound above is what is tight here.
 */
const MAX_BYTES_PER_TEST_ANY_SCALE = 2450

const pad = (value, width) => String(value).padStart(width)

/**
 * Least-squares slope of log10(retainedBytes) against log10(N): the exponent itself, so 1.0 is
 * linear and 2.0 quadratic. It is `retainedBytes` on the y axis deliberately -- regressing
 * `bytesPerTest` instead would give the exponent MINUS one, and printing that under the label
 * "exponent" is how a reported number becomes a wrong claim.
 */
function fittedExponent(points) {
  const xs = points.map((point) => Math.log10(point.tests))
  const ys = points.map((point) => Math.log10(point.retainedBytes))
  const meanX = xs.reduce((sum, x) => sum + x, 0) / xs.length
  const meanY = ys.reduce((sum, y) => sum + y, 0) / ys.length
  const covariance = xs.reduce((sum, x, index) => sum + (x - meanX) * (ys[index] - meanY), 0)
  const variance = xs.reduce((sum, x) => sum + (x - meanX) ** 2, 0)
  return covariance / variance
}

/**
 * Generate pinned scale corpora, measure memory growth and enforce scaling limits; clean
 * generated files unless --keep was requested.
 */
function main() {
  const { binary, keep } = parseGateArguments('check-memory-scaling.mjs')
  const failures = []
  const points = []
  const corpora = join(ROOT, '.memory-corpus')

  // Every scale is generated under `corpora`, and the whole tree is removed in the `finally`
  // unless `--keep` was passed -- including when a measurement throws. `process.exit` below stays
  // outside that block, because `process.exit` does not run `finally`. ~79 MB across the three
  // scales, regenerated byte-identically from the seed, so there is nothing to preserve.
  try {
    for (const scale of SCALES) {
      const pinned = CORPUS_SCALES[scale]
      if (pinned === undefined) {
        throw new Error(`scale ${scale} has no pinned corpus in CORPUS_SCALES`)
      }
      // Fixed-length path, and the same one `check-memory.mjs` uses at 10k, so the two gates
      // measure the same tree at the same path and report the same number. See PATH_BUDGET.
      const root = corpusRoot(corpora, scale)
      const corpus = generateCorpus({ root, tests: scale })
      if (corpus.digest !== pinned.digest) {
        throw new Error(
          `the ${scale}-test corpus has digest ${corpus.digest}, pinned is ${pinned.digest}. The ` +
            'shape of the corpus decides the slope, so a changed corpus invalidates every number ' +
            'here. Re-pin it and re-measure in the same commit.',
        )
      }

      const { record, elapsedMs } = measureMemory(binary, root)

      // The floor on work performed, per scale. A scale that extracted nothing would otherwise
      // report a tiny `bytesPerTest` and FLATTEN the curve -- a silent success that also hides the
      // regression the next scale would have shown.
      failures.push(...workPerformedProblems(record, pinned, `scale ${scale}`))

      points.push({
        tests: record.testsAnalyzed,
        files: record.filesAnalyzed,
        retainedBytes: record.retainedBytes,
        bytesPerTest: record.bytesPerTest,
        // The sawtooth-free quantity the growth bound is applied to. See the header.
        irBytesPerTest: (record.retainedBytes - record.fields.spine) / record.testsAnalyzed,
        spinePerDeclaration: record.fields.spine / record.declarationsRetained,
        peakBytes: record.peakRssBytes,
        elapsedMs,
      })
    }

    // Nothing is compared until every scale has cleared its floor. A curve fitted through a point
    // that failed validation is a number, not a measurement.
    if (failures.length === 0) {
      console.log(
        '     tests    files      retained  bytes/test   IR/test  vs prev  spine/decl' +
          '  peak RSS (sanity)    time',
      )
      let previous = null
      for (const point of points) {
        const growth = previous === null ? null : point.irBytesPerTest / previous.irBytesPerTest
        console.log(
          `  ${pad(point.tests, 8)} ${pad(point.files, 8)} ${pad(mb(point.retainedBytes), 13)} ` +
            `${pad(point.bytesPerTest, 11)} ${pad(point.irBytesPerTest.toFixed(0), 9)} ` +
            `${pad(growth === null ? '--' : `${growth.toFixed(3)}x`, 8)} ` +
            `${pad(point.spinePerDeclaration.toFixed(0), 11)} ` +
            `${pad(point.peakBytes === null ? 'unavailable' : mb(point.peakBytes), 18)} ` +
            `${pad(`${(point.elapsedMs / 1000).toFixed(1)}s`, 7)}`,
        )
        previous = point
      }

      for (let index = 1; index < points.length; index += 1) {
        const from = points[index - 1]
        const to = points[index]
        const growth = to.irBytesPerTest / from.irBytesPerTest
        if (growth > MAX_GROWTH_PER_DECADE) {
          failures.push(
            `IR bytes per test grew ${growth.toFixed(3)}x from ${from.tests} tests ` +
              `(${from.irBytesPerTest.toFixed(0)}) to ${to.tests} ` +
              `(${to.irBytesPerTest.toFixed(0)}), above the permitted ` +
              `${MAX_GROWTH_PER_DECADE}x per decade. Retention is not linear in test count: ` +
              `${from.tests} -> ${to.tests} is ${(to.tests / from.tests).toFixed(0)}x the tests and ` +
              `${(to.retainedBytes / from.retainedBytes).toFixed(1)}x the memory. Something in the ` +
              'extraction path costs more per test the more tests there are, which is the failure ' +
              'that makes this tool unusable on exactly the monorepo it was built for.',
          )
        }
      }
      for (const point of points) {
        if (point.bytesPerTest > MAX_BYTES_PER_TEST_ANY_SCALE) {
          failures.push(
            `at ${point.tests} tests, retention is ${point.bytesPerTest} bytes per test, above the ` +
              `ceiling of ${MAX_BYTES_PER_TEST_ANY_SCALE}. The growth bound above constrains the ` +
              'shape of the curve and says nothing about its height; this is the height.',
          )
        }
      }

      const exponent = fittedExponent(points)
      console.log(
        `\n  fitted exponent    ${exponent.toFixed(4)} (retained ~ N^${exponent.toFixed(3)}; 1.0 is ` +
          'linear, 2.0 quadratic) -- REPORTED, not compared; see the header\n' +
          `  growth bound       ${MAX_GROWTH_PER_DECADE}x per decade of N on IR bytes/test, which ` +
          'is the assertion\n' +
          `  ceiling            ${MAX_BYTES_PER_TEST_ANY_SCALE} bytes/test at every scale, spine ` +
          'included\n' +
          `  at 1M tests        ${(points[points.length - 1].bytesPerTest * 1e6 / 1024 ** 3).toFixed(2)} GB ` +
          'projected from the largest scale, against a 2 GB budget',
      )
    }
  } finally {
    console.log(`\n(${discardCorpus(corpora, keep)})`)
  }

  if (failures.length > 0) {
    console.error('\nFAIL: memory scaling gate')
    for (const failure of failures) console.error(`  ${failure}`)
    process.exit(1)
  }
  console.log('\nok: retention is linear in test count across three decades, and bounded at each.')
}

if (resolve(process.argv[1] ?? '') === resolve(fileURLToPath(import.meta.url))) {
  try {
    main()
  } catch (error) {
    console.error(`\nFAIL: memory scaling gate could not complete a measurement.\n  ${error.message}`)
    process.exit(1)
  }
}
