#!/usr/bin/env node
// Isolation gate for negative fixtures.
//
// A `rule=none` fixture passes by nothing happening, which makes every one of them a candidate for
// silent success -- the failure mode the work-performed floor exists to catch: it can stop
// discriminating and still pass forever. `check-duplicates-corpus.mjs` asserts the analyzer stayed silent between the
// declared participants; nothing asserted the participants were ever close enough for that silence
// to mean anything.
//
// This gate asserts the missing half: **a negative fixture that claims to isolate one variable must
// have participants whose fingerprints agree on everything else, and disagree exactly where the
// analyzer is supposed to see that variable.** Each `rule=none` expectation declares its variable
// with `@vitest-dedup:isolates <dimension>`; `lib/isolation.mjs` translates every dimension into a
// relation over the analyzer's own emitted views and states, per dimension, what that translation
// is derived from.
//
// The fixture that motivated it was `near-arrow-vs-function-this-twin.test.ts`, which claimed to
// isolate `this` binding while its two bodies differed in content -- a local assignment versus a
// member write. A regression collapsing lexical and dynamic `this` would have left it green.
//
// ---------------------------------------------------------------------------------------------
// The oracle: --emit-test-fingerprints
// ---------------------------------------------------------------------------------------------
//
// This gate used to tokenize fixture source and compare two callbacks after erasing the declared
// dimension. That was an approximation of the analyzer, deliberately stricter than it and never
// looser, and 29 of 61 negative expectations could not be stated in it at all.
//
//   vitest-dedup <dir> --reporters jsonl --emit-test-fingerprints
//
// now emits one `"test"` record per analyzed test and one `"hook"` record per analyzed hook,
// before the findings, carrying `alpha`, `structural`, `behavior`, `trivial` and (`"test"` only)
// `contextKey` and `modifierKey` as 16-hex-digit hashes that may only be compared for equality.
// Crucially it includes the declarations that appear in NO finding, which is by definition what a
// `rule=none` fixture's participants are -- the data no reporter emitted, and the reason this gate
// could only approximate.
//
// Three things that surface changed, beyond making the translations exact:
//
//   - Rows of one `.each` table get one record each, distinguished by rendered title, so a
//     per-row claim ("these two rows differ in their literals and nothing else") is now statable.
//     The source-level version never could: both rows share a declaration line and a callback.
//   - A test participant is resolved by the title the analyzer actually renders, then checked
//     against its declared line. Four participants in this corpus named a title no test bears.
//   - A hook participant resolves a different way: it has no title of its own and can never share
//     a declaration site the way an `.each` row does, so it is matched directly by (file, declared
//     line) instead of by title-then-ordinal -- see `resolveParticipants` in `lib/isolation.mjs`.
//
// What it did NOT do is retire the opt-outs wholesale. The stream is one record per executable TEST
// or HOOK declaration: a suite gets neither, a pair that is identical in every emitted field admits
// no assertion that can fail, and a clustering property is not a relation between two records at
// all. See `lib/isolation.mjs`'s UNCHECKED, UNSTATEABLE and NOT_ISOLABLE for the split and the
// discipline on it.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { enumerateFixtures, parseFixture } from './lib/fixture-directives.mjs'
import { checkExpectation, collectFingerprintRecords } from './lib/isolation.mjs'
import { defaultBinary } from './lib/run-analyzer.mjs'

// Every fixture directory carrying `@vitest-dedup:` expectation blocks. `fixtures/observed/` holds
// the distilled reproductions of shapes found on the external corpus; its negatives pass by nothing
// happening exactly like every other negative, so it is checked here on the same terms. Keep this
// in step with `check-duplicates-corpus.mjs`'s `TARGETS` -- a directory that gate analyzes but this
// one does not would have negatives whose isolation nobody checks.
//
// Fixtures are enumerated by `enumerateFixtures`, the same helper the corpus gate uses, so the two
// gates cannot disagree about which files carry expectations.
//
// Each directory is analyzed as ONE run, because a participant may name a test in a sibling file
// (the `-a`/`-b` corpus convention) and cross-file identity is scoped to the run.
const DIRS = ['fixtures/duplicates', 'fixtures/observed', 'fixtures/near', 'fixtures/mixed']

// A floor on work performed: a gate that can only report "no problems" is incomplete
// by construction. If a refactor ever makes this gate check nothing -- a directory rename, a
// directive regex that stops matching, every fixture quietly opted out -- it must fail loudly
// rather than print a clean pass over zero fixtures. Raise this when the corpus grows; it is a
// floor, not a count.
//
// It floors the GENUINELY-checked number only. Neither opt-out counts toward it, which is the
// whole reason they are tracked separately: a change that converts a real check into a
// `not-isolable` opt-out to make a summary line look tidier trips this floor instead of passing.
// 34, not 35: `each-computed-key-row.test.ts` used to declare `isolates modifier`, a genuine
// fingerprint check, but its participants are the not-compared kind (a computed `.each` key
// defeats static row naming, so their context never resolves) -- exactly the not-compared-
// laundered-into-distinct shape this whole gate exists to close, just laundered into a "checked"
// isolation claim this time instead of a plain finding. The per-participant `comparable` guard
// this gate now runs caught it, and it was reclassified `not-isolable` to match
// `each-unresolved-table.test.ts`'s precedent. Lower this again if another one goes the same way.
// 36, not 34: the `not-duplicates-mock-then-unmock` pair were the corpus's two known violations
// until `vi.unmock` was modelled in `ContextKey` rather than tainting the file. Their markers are
// gone and both now check genuinely, so the floor ratchets with them -- reverting the modelling
// must fail here as well as on the verdicts, not merely go quiet by two.
// 37, not 36: `not-redundant-hook-different-body.test.ts` moved from `unchecked` to
// `isolates hook-literal` once `"hook"` records existed to state its claim -- the participants are
// two `beforeEach` hooks agreeing at `structural` and differing at `alpha`. Reverting the emission
// (`analysis::hook_fingerprints`, `JsonlReporter::with_fingerprints`) must fail here too, not just
// on the `checked`/`unchecked` counts this file also asserts.
// 41, not 37: the 2026-09-27 specification sweep (`tests-implementation.local.md`) added 21
// negative expectations in one commit. Twenty of them are not isolation claims by construction
// (17 `not-isolable` -- byte-identical hook pairs, loop iterations, unrendered-title clusters --
// and 3 `isolation-violation` markers on the S6/S8 pairs whose `isolates context` claims do not
// hold until their model changes land, ratcheting off when they do); the ONE genuinely new check
// is `near-chai-property-chain-differs`' `isolates identifier`, whose participants agree at
// `contextKey` and differ at `alpha`/`structural` exactly as declared. Reverting the sweep must
// fail here, not merely print four fewer lines.
// Ratcheted 41 -> 43 when S6/R9 landed: the two `module-scope-setup` expectations stopped being
// `@vitest-dedup:isolation-violation` and became real checks, because the awaited setup call now
// reaches `ContextKey` and their context keys genuinely differ. Known violations 3 -> 1.
// Ratcheted 43 -> 44 when S8/R10 landed: `not-duplicates-stubbed-global-sibling-describes`
// stopped being an `@vitest-dedup:isolation-violation` and became a real check, because an
// ambient `beforeAll` now reaches `ContextKey` and the two describes' context keys genuinely
// differ. Known violations 1 -> 0.
// Ratcheted 44 -> 77, which is what the corpus checks: the floor had trailed the real count by 32,
// so a regression that dropped up to 32 checks still passed. 77 includes
// `not-duplicates-literal-kind-spec-extension.spec.ts`; enumerating only `.test.ts` leaves 76.
const MIN_CHECKED_EXPECTATIONS = 77

// A ceiling on the debt bucket, which ratchets the other way. `unchecked` means "an isolation claim
// with no mechanism yet"; it went 16 -> 22 -> 29 over three PRs under the source-level checker,
// because fixtures were added faster than the approximation could verify them. A bound makes that
// drift a failure rather than a note nobody reads. Lower it when a mechanism lands; never raise it
// to admit a new fixture -- classify the fixture instead.
//
// 4, not 5: the hook record kind closed `not-redundant-hook-different-body.test.ts`'s opt-out
// (see `MIN_CHECKED_EXPECTATIONS` above).
// 0, not 4: the four near-duplicate-context expectations moved to `unstateable`, a classification
// with its own admission rule, rather than being counted there. Their claim's equal half ("bodies
// equivalent modulo phrasing") needs a view `--emit-test-fingerprints` does not emit -- the pairs
// differ at `alpha`, `structural` AND `behavior`, sharing only `modifierKey`, which is too thin an
// agreement to be a check -- so no relation over the current stream can state them and there is no
// mechanism to build at this surface. The bucket stays at zero: a new `unchecked` expectation is a
// claim this gate COULD state with work, and that is a reviewable edit, not a drift.
const MAX_UNCHECKED_EXPECTATIONS = 0

// The same bound for `unstateable`, for the same reason: a bucket with an admission rule but no
// ceiling is where the next drift goes once `unchecked` is capped -- the shape of the 16 -> 22 ->
// 29 history above, one fixture at a time. Never raise it to admit a new fixture; an expectation
// whose equal half genuinely has no emitted view is a reviewable edit to this number with the
// structural argument in the fixture's reason, not a quiet addition.
const MAX_UNSTATEABLE_EXPECTATIONS = 4

const DEFAULT_BINARY = defaultBinary()

/**
 * Checks every `rule=none` expectation across the four fixture directories and returns the
 * tallies `report` prints.
 *
 * Each directory is analyzed as ONE analyzer run (a participant may name a test in a sibling
 * file), and every parsed expectation is dispatched through `checkExpectation`. The returned
 * buckets are disjoint by construction: `failures` (broken declarations, unmet relations,
 * ceiling breaches) are the only bucket that makes the gate exit 1; `violations`,
 * `unchecked`, `unstateable` and `notIsolable` are counted, printed, and passed.
 *
 * @param {string} binary path to the vitest-dedup binary; must be built from current source so
 *   the fingerprint records carry the fields this gate refuses to default.
 * @returns {{failures: string[], unchecked: string[], unstateable: string[],
 *   notIsolable: string[], violations: string[], checked: number, negatives: number}}
 */
export function run(binary = DEFAULT_BINARY) {
  const failures = []
  const unchecked = []
  const unstateable = []
  const notIsolable = []
  const violations = []
  let checked = 0
  let negatives = 0

  for (const dir of DIRS) {
    // Participants here must be fixtures: a helper sibling has no fingerprint records and no source
    // loaded, so naming one is rejected at parse time rather than failing later.
    const { fixtureNames: names } = enumerateFixtures(dir)
    const knownFiles = new Set(names)
    const sources = new Map(names.map((name) => [name, readFileSync(join(dir, name), 'utf8')]))
    const readSource = (file) => sources.get(file) ?? null

    // One analyzer run per directory, reused by every expectation in it. The records are a pure
    // function of the directory's contents, so running per fixture would be the same answer at
    // sixty times the cost.
    let index
    try {
      index = collectFingerprintRecords(binary, dir)
    } catch (error) {
      failures.push(
        `FAIL ${dir}: could not collect test fingerprints -- ${error.message}\n` +
          `  This gate needs a binary built from the current source; run \`cargo build --release\`.`,
      )
      continue
    }

    for (const name of names) {
      const parsed = parseFixture(sources.get(name), name, knownFiles)
      if (parsed.error) {
        failures.push(`FAIL ${join(dir, name)}: ${parsed.error}`)
        continue
      }
      for (const expectation of parsed.expectations) {
        if (expectation.rule !== 'none') continue
        negatives += 1
        const result = checkExpectation(expectation, index, readSource)
        if (result.status === 'fail') {
          failures.push(`FAIL ${join(dir, name)}: ${result.message}`)
        } else if (result.status === 'unchecked') {
          unchecked.push(`${join(dir, name)}: ${result.message}`)
        } else if (result.status === 'unstateable') {
          unstateable.push(`${join(dir, name)}: ${result.message}`)
        } else if (result.status === 'not-isolable') {
          notIsolable.push(`${join(dir, name)}: ${result.message}`)
        } else if (result.status === 'violation') {
          violations.push(`${join(dir, name)}: ${result.message}`)
        } else {
          checked += 1
        }
      }
    }
  }

  if (checked < MIN_CHECKED_EXPECTATIONS) {
    failures.push(
      `FAIL (gate): only ${checked} negative expectation(s) were actually checked, below the floor ` +
        `of ${MIN_CHECKED_EXPECTATIONS}. A gate that checks nothing reports a clean pass, which is ` +
        `the failure mode this gate exists to close.`,
    )
  }
  if (unchecked.length > MAX_UNCHECKED_EXPECTATIONS) {
    failures.push(
      `FAIL (gate): ${unchecked.length} expectation(s) are "unchecked", above the ceiling of ` +
        `${MAX_UNCHECKED_EXPECTATIONS}. That bucket is debt -- an isolation claim with no mechanism ` +
        `yet -- and it is bounded so it cannot drift upward one fixture at a time the way it did ` +
        `before. Either build the mechanism, or establish that the new fixture's claim is not an ` +
        `isolation claim at all and mark it not-isolable`,
    )
  }
  if (unstateable.length > MAX_UNSTATEABLE_EXPECTATIONS) {
    failures.push(
      `FAIL (gate): ${unstateable.length} expectation(s) are "unstateable", above the ceiling of ` +
        `${MAX_UNSTATEABLE_EXPECTATIONS}. That bucket is bounded for the same reason "unchecked" ` +
        `is: an opt-out with an admission rule but no ceiling is where drift goes once the other ` +
        `bucket is capped. Raising this number is a reviewed edit carrying the structural argument ` +
        `for why the new claim's equal half has no emitted view`,
    )
  }

  return { failures, unchecked, unstateable, notIsolable, violations, checked, negatives }
}

/**
 * Prints one `run` result and returns the process exit code: 0 when `failures` is empty, 1
 * otherwise. The four opt-out buckets print as notes with every entry listed, so a reader
 * sees WHICH expectations are only held rather than checked -- the same visibility that keeps
 * an opt-out from becoming a suppression list.
 *
 * @param {ReturnType<typeof run>} counts the value `run` returned.
 * @returns {number} the exit code for `process.exit`.
 */
export function report({ failures, unchecked, unstateable, notIsolable, violations, checked, negatives }) {
  if (violations.length > 0) {
    console.log(
      `note: ${violations.length} negative expectation(s) are KNOWN NOT to isolate what they ` +
        'declare (marked @vitest-dedup:isolation-violation). Each passes the corpus gate while ' +
        'proving nothing about the guard it names:',
    )
    for (const line of violations.sort()) console.log(`  ${line}`)
  }
  if (unchecked.length > 0) {
    console.log(
      `note: ${unchecked.length} negative expectation(s) are UNCHECKED: a real isolation claim ` +
        'with no mechanism to state it yet. This count is debt and should reach zero; each reason ' +
        'names the missing mechanism:',
    )
    for (const line of unchecked.sort()) console.log(`  ${line}`)
  }
  if (unstateable.length > 0) {
    console.log(
      `note: ${unstateable.length} negative expectation(s) are UNSTATEABLE: real isolation ` +
        'claims whose equal half needs a view the fingerprint stream does not emit. Not debt at ' +
        'this surface -- each reason names the missing view and the analyzer event that produces ' +
        'it, and restates as a dimension when that event lands:',
    )
    for (const line of unstateable.sort()) console.log(`  ${line}`)
  }
  if (notIsolable.length > 0) {
    console.log(
      `note: ${notIsolable.length} negative expectation(s) are NOT-ISOLABLE: they are not ` +
        'isolation claims and never will be (policy silencing, participants the analyzer cannot ' +
        'distinguish by construction, or a property over a cluster rather than a pair). This ' +
        'count is not debt:',
    )
    for (const line of notIsolable.sort()) console.log(`  ${line}`)
  }
  if (failures.length > 0) {
    for (const line of failures) console.error(line)
    console.error(
      `\n${failures.length} isolation problem(s) across ${negatives} negative expectation(s).`,
    )
    return 1
  }
  console.log(
    `ok: ${checked} of ${negatives} negative expectation(s) verified against the analyzer's own ` +
      `fingerprints to isolate exactly the variable they declare (${violations.length} known ` +
      `violation(s), ${unchecked.length} unchecked, ${unstateable.length} unstateable, ` +
      `${notIsolable.length} not-isolable).`,
  )
  return 0
}

// Runnable standalone (`node scripts/check/check-fixture-isolation.mjs [binary]`) and importable, so
// `check-duplicates-corpus.mjs` runs the same check in CI without a second workflow step.
if (process.argv[1] && process.argv[1].endsWith('check-fixture-isolation.mjs')) {
  process.exit(report(run(process.argv[2] ?? DEFAULT_BINARY)))
}
