#!/usr/bin/env node
// Corpus gate for fixtures/duplicates/ and fixtures/observed/ — plus, since V1, a whole-tree
// blindness ratchet over ALL of fixtures/ (see `DELIBERATE_BLIND_FILES`): the per-fixture
// expectations below cover three directories, while the blind set is pinned across the entire
// tree, because blindness can be manufactured anywhere a fixture's extraction can degrade.
//
// `fixtures/observed/` was added when the six pinned reference repositories moved to scheduled
// monitoring: each of its fixtures distils one shape actually found on that corpus into something
// this repository owns, reads in one screen, and can check offline. See `TARGETS` below. Both
// fixture directories are on the blocking merge path; the external reference repositories they
// were distilled from are not -- those are scheduled monitoring. Everything else in this file
// applies to both directories identically.
//
// Historically this gate inferred a fixture's expected verdict from its filename
// (`duplicates-*` "must fire", everything else "must stay silent") and only checked whether *any*
// finding mentioned the file. An external review found that structurally incapable of protecting
// the near-duplication corpus it was about to be handed:
//
//   - Every planned `near-*`/`parameterization-*` fixture would be read as expecting silence,
//     because the prefix check only recognized `duplicates-` and `duplicate-test-name`.
//   - "Some finding mentions this file" is satisfied by the wrong rule firing between the wrong
//     tests -- a `duplicate-test-name` warning would satisfy a near-duplication positive while the
//     rule under test found nothing at all.
//   - There was no way to tell "compared and found different" (`distinct`) from "never compared"
//     (`not-compared`); a fixture could pass because its tests were silently skipped.
//
// This version replaces the filename heuristic with a machine-readable expectation block in each
// fixture's header comment (see `CONTRIBUTING.md`'s fixture conventions for the verdict
// vocabulary). The block states, per fixture: how many tests must be extracted, and for every pair
// (or cluster edge) under test, which rule must fire, at what severity, naming which participants
// by file-relative line and title -- or, for a pair expected to produce no finding, whether that
// silence is `distinct` (compared, found different) or `not-compared` (never eligible).
//
// A fixture with no block, or a block that fails to parse, is a gate FAILURE, never a silent
// default -- see `parseFixture` below. This is deliberate: it is what lets another agent drop a
// new fixture into this directory without this gate quietly waving it through.
//
// Each directory is judged pairwise by `lib/next-duplicates.mjs`: every required pair linked by its
// rule, every declared silence kept, and no other link. A delta is a failure unless
// `next-duplicates-known.json` lists it with a reason; a listed delta that no longer occurs fails
// too. Per-fixture extraction counts and solo-run eligibility are not judged yet: the headers
// still state the earlier analyzer's facts, and their deltas are the known list's.
//
// ---------------------------------------------------------------------------------------------
// Expectation block format
// ---------------------------------------------------------------------------------------------
//
// Parsed by `lib/fixture-directives.mjs`, which owns the format and documents it in full. The short
// version: plain `//` comment lines carrying `@vitest-dedup:extracted`, `@vitest-dedup:expect`,
// `@vitest-dedup:isolates`, and `@vitest-dedup:participant`, stating per fixture how many tests must
// be extracted and, for every pair under test, which rule must fire at what severity between which
// participants -- or, for a pair expected to produce no finding, whether that silence is `distinct`
// (compared, found different) or `not-compared` (never eligible), and which single variable the pair
// isolates.
//
// That parser is shared with `check-fixture-isolation.mjs`, which this gate also runs (see the end
// of `main`). The two checks are complementary and neither is sufficient alone: this one asserts the
// analyzer stayed silent between the declared participants, and that one asserts the participants
// were close enough for the silence to mean something. A negative fixture whose participants differ
// in more than the one variable it claims to isolate passes this gate while proving nothing -- the
// silent-success shape the work-performed floor exists to reject.
//
// ---------------------------------------------------------------------------------------------
// distinct vs not-compared: what this gate can and cannot verify
// ---------------------------------------------------------------------------------------------
//
// No reporter exposes per-test comparison eligibility -- neither the JSON nor the JSONL report
// enumerates tests that did not appear in a finding, so there is no direct way to ask "was this
// specific pair ever eligible for comparison". What IS exposed is `AnalysisResult::incomplete`
// (`summary.incomplete` in both reports), and `analysis::analyze` sets it whenever ANY test in the
// run has an unresolved `Context` (dropped mock, unreadable `test.extend` factory, unresolvable
// `.each` table, dynamic title, opaque body -- see `model.rs`'s `Context::unresolved` and
// `analysis.rs`'s `has_unresolved_context`), independent of whether any rule fires.
//
// Run alone (this gate already isolates every fixture into its own directory to attribute
// extraction counts -- see `checkExtraction`), that per-file `incomplete` flag becomes a usable,
// if coarse, proxy:
//
//   - `not-compared` asserts the solo run's `incomplete` is `true`: something in the file was
//     genuinely unresolved, so absence of a finding proves nothing.
//   - `distinct` asserts the solo run's `incomplete` is `false`: nothing was silently excluded on
//     those grounds, so the tests were actually compared and found different.
//
// This only catches the `Context::unresolved` family of exclusions. A body excluded from
// `duplicate-test`/`parameterization-candidate` for being a recognized trivial stub
// (`Fingerprints::trivial`) does NOT set `incomplete`, so `summary.incomplete` alone cannot
// distinguish "trivial body, never compared" from "compared, found different". It no longer has
// to: `Fingerprints::trivial` is already on the `--emit-test-fingerprints` stream (`"test"` and
// `"hook"` records alike), so this gate runs that stream once per directory and uses it as a
// SECOND eligibility signal -- a `not-compared` verdict may also rest on a participant whose
// record is trivial (excluded from comparison for that reason, run complete or not), and a
// `distinct` verdict is rejected when a participant's record says trivial, because "compared and
// found different" is then false: the pair was never compared at all. A participant with no
// resolvable record (a suite, resolved through its children) asserts nothing here, the same way
// it asserts nothing in the isolation gate.
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { report as reportIsolation, run as runIsolation } from './check-fixture-isolation.mjs'
import { enumerateFixtures, FIXTURE_NAME, parseFixture } from './lib/fixture-directives.mjs'
import { blindRecallFiles } from './lib/corpus-labels.mjs'
import { blindCaseFiles, readCases } from './lib/oracle-cases.mjs'
import { expectationEdges, judge, knownProblems, pairEdges, pairSeverities, strayFindings } from './lib/next-duplicates.mjs'
import { defaultBinary, runAnalyzerJsonl } from './lib/run-analyzer.mjs'

const binary = process.argv[2] ?? defaultBinary()

// Every fixture directory this gate reads, with the rule overrides its run is analyzed under.
//
// `fixtures/observed/` is the newer half: each of its fixtures is a distilled, self-contained
// reproduction of a shape actually found on the pinned external corpus -- a distilled fixture can
// only pin a shape someone already found, which is why the corpus is monitored at all. The
// reference repositories moved to scheduled monitoring, so what was learned from them has to be
// pinned by fixtures this repository owns and can run offline -- which is worth nothing unless a
// gate reads them, hence this list rather than a second script.
//
// Every directory is analyzed with the same judgment and the isolation gate at the end, under the
// same `rules`.
//
// `subsumed-test` is enabled everywhere, and is the only override left. It ships Off on measured
// precision, and this gate pins rule *behavior* (which participants, which verdict, which severity
// when run) while the default itself is pinned by `tests/docs_contract.rs`'s severity contract
// against README's table. A rule whose default is a product decision must not become untested the
// moment that decision lands.
//
// `cross-file-duplicate` used to be the second entry, enabled for `fixtures/observed/` only,
// because `fixtures/duplicates/` held a pair whose declared contract was silence under the default
// config. Both halves of that arrangement are gone: the rule ships `Warning` on an exhaustive
// adjudication (9/9 `redundant` over 15,980 tests), and `fixtures/duplicates/` now declares eight
// real `cross-file-duplicate` positives -- the promoted `cross-file-duplicate-would-fire-{a,b}`
// pair plus the seven edges that other fixtures in that directory form with each other by sharing
// a body. Keeping the override would have been worse than redundant: with the rule on by default,
// `--rule cross-file-duplicate=warning` would keep every one of those expectations passing even if
// the shipped default were reverted, which is the exact shape of a gate that cannot fail.
const TARGETS = [
  { dir: 'fixtures/duplicates', rules: ['subsumed-test=warning'] },
  { dir: 'fixtures/observed', rules: ['subsumed-test=warning'] },
  // `fixtures/mixed` holds fixtures where TWO named defects interact in one file, so a fix for
  // one cannot be validated without showing it did not silence the other. Same overrides as the
  // other two directories -- no shipped default is masked by a flag -- and the empty-directory
  // floor in `main()` covers it, which matters because this directory was created empty and
  // stayed that way for a while.
  { dir: 'fixtures/mixed', rules: ['subsumed-test=warning'] },
]

/** Known deltas between the fixture headers and the analyzer's output, each with its reason. */
const KNOWN_PATH = 'scripts/check/next-duplicates-known.json'

/**
 * One whole-directory run of `target`, judged pairwise against its fixtures' expectation blocks:
 * every required pair linked, every declared silence kept, every other delta listed in
 * `next-duplicates-known.json`. Appends to `failures` and `held`; records consumed known entries.
 *
 * @returns {number} how many fixtures the directory holds, for the summary line.
 */
function checkTarget(target, known, { failures, held, consumed }) {
  const { dir } = target
  // A participant may name any analyzable sibling, but only files named like tests carry a block.
  const { fixtureNames, knownFiles } = enumerateFixtures(dir)
  const parsed = new Map()
  for (const name of fixtureNames) {
    const result = parseFixture(readFileSync(join(dir, name), 'utf8'), name, knownFiles)
    if (result.error) failures.push(`FAIL ${join(dir, name)}: ${result.error}`)
    else parsed.set(name, result)
  }
  // One whole-directory run: cross-file verdicts need every fixture visible at once.
  const rules = target.rules.flatMap((rule) => ['--rule', rule])
  const { records } = runAnalyzerJsonl(binary, [dir, ...rules])
  const summary = records.find((record) => record.type === 'summary')
  if (!summary) {
    failures.push(`FAIL ${dir}: the run produced no summary record`)
    return fixtureNames.length
  }
  if (!(summary.testsAnalyzed > 0)) {
    failures.push(`FAIL ${dir}: ${summary.testsAnalyzed} test(s) analyzed; a run that measured nothing cannot pass`)
  }
  for (const finding of strayFindings(records)) {
    failures.push(`FAIL ${dir}: ${finding.rule} reported a finding, and the judge reads no such rule`)
  }
  const { required, declared, forbidden } = expectationEdges(parsed)
  if (required.length === 0) {
    failures.push(`FAIL ${dir}: no duplicate-rule expectation was asserted; a directory that judges nothing is a silent success`)
  }
  const outcome = judge({ required, declared, forbidden, linked: pairEdges(records), severities: pairSeverities(records), known, scope: dir })
  failures.push(...outcome.failures.map((failure) => failure.message))
  held.push(...outcome.held)
  for (const id of outcome.consumed) consumed.add(id)
  return fixtureNames.length
}

// V1's remainder: the deliberate-blind ratchet, over the WHOLE fixture tree. Blind files --
// files whose every executable test was excluded -- are not a defect in the fixtures that carry
// them: each was written to exercise an exclusion (a tainted mock, a dynamic title, an unreadable
// table). What WOULD be a defect is a fixture going blind for a reason nobody chose, and the
// per-fixture `incomplete` expectations above cannot catch it: a fixture already `incomplete:
// true` for its deliberate cause can go blind on top without flipping anything the expectations
// read.
//
// The ratchet compares the SET, not the count (Copilot review, PR #32): a count can pass a swap
// -- one deliberate fixture un-blinding while another goes blind accidentally keeps 13 at 13.
// The set is derived from the analyzer's own per-test fingerprint stream (`--emit-test-
// fingerprints`, which filters to executable declarations exactly as `analyze` does), so the
// comparison needs no new report surface: group the stream's `test` records by path, and a file
// is blind when every record says `comparable: false`. The stream's own aggregate
// (`summary.blindFiles`) is cross-checked against the derived set for free -- the analyzer's
// count and the stream it emits must agree about the same corpus.
//
// The set is pinned in BOTH directions: a deliberate change (a fixture written to be blind, or
// one deliberately recovered) edits this list, or the `blindUnderCurrent` of its oracle or recall
// case, in the same commit; anything else fails with the path-level diff.
/** Record exclusions that mean a test has nothing to compare, which `summary.blindFiles` skips. */
const NOTHING_TO_COMPARE = new Set(['no-callback', 'empty-body'])

const DELIBERATE_BLIND_FILES = new Set([
  // Benchmarks are counted, not compared.
  'declarations/bench-legacy-toplevel.bench.ts',
  'oracle/benchmarks/speed.bench.ts',
  'recall/duplicate-test-bench/perf.bench.ts',
  'recall/duplicate-test-bench/perf.test.ts',
  // A context the analyzer cannot resolve: a title folded from a runtime value, a scoped
  // `vi.doMock`, a body cut short at the depth cap, a guard that `eval` can rebind.
  'declarations/template-literal-title-const-fold.test.ts',
  'duplicates/not-duplicate-each-row-loop-template-title.test.ts',
  'edge/vi-domock-scoped.test.ts',
  'edge/vi-dounmock-scoped.test.ts',
  'mixed/mixed-loop-each-in-non-idempotent-hook-scope.test.ts',
  'oracle/truncated-body/truncated.test.ts',
  'recall/runner-eval-binding-guard/rows-unsafe.test.ts',
  // Tests that read state a hook or another test accumulates.
  'duplicates/not-redundant-hook-accumulator.test.ts',
  'recall/suite-registration-state/a.test.ts',
  // Registrations through imported or local runners whose effects are unproved: the calling
  // scope is excluded.
  'observed/observed-module-scope-helper-declares-tests.test.ts',
  'oracle/binding-stability/namespace-export-const.test.ts',
  'oracle/binding-stability/namespace-export-runtime.test.ts',
  'oracle/binding-stability/namespace-export-shadow.test.ts',
  'oracle/binding-stability/namespace-export-stars.test.ts',
  'oracle/binding-stability/rewritten-namespace-call.test.ts',
  'oracle/binding-stability/rewritten-namespace-export.test.ts',
  'oracle/collection-scope-calls/optional-call.test.ts',
  'oracle/hooks-only-control/control.test.ts',
  'oracle/hooks-only-cycle-head-direct/case.test.ts',
  'oracle/hooks-only-cycle-head-local/case.test.ts',
  'oracle/hooks-only-cycle-head-third/case.test.ts',
  'oracle/wrapper-inlining/wrapped.test.ts',
  'recall/framework-runner-module-values/slugs.test.ts',
  'recall/framework-runner-plain-import-control/slugs.test.ts',
  'recall/helper-in-source-bindings/run.test.ts',
  'recall/helper-initializer-factories/run.test.ts',
  'recall/helper-initializer-test/run.test.ts',
  'recall/local-helper-const-arrow/run.test.ts',
  'recall/local-helper-const-concise/run.test.ts',
  'recall/local-helper-const-later-call/run.test.ts',
  'recall/local-helper-finally-local-break/run.test.ts',
  'recall/local-helper-member-write/run.test.ts',
  'recall/local-helper-overloaded/run.test.ts',
  'recall/local-helper-return-describe/run.test.ts',
  'recall/local-helper-return-tail/run.test.ts',
  'recall/local-helper-return-test/run.test.ts',
  'recall/local-helper-unused-default/run.test.ts',
  'recall/runner-unproved-scope-retention/unsafe.test.ts',
  'recall/suite-user-wrapper-withheld/a.test.ts',
  // Registrations a wrapper call, star re-export or namespace export makes where the file does
  // not spell them.
  'oracle/api-reexports/star-package.test.ts',
  'oracle/api-reexports/star-second.test.ts',
  'oracle/binding-stability/namespace-export-declared.test.ts',
  'oracle/binding-stability/namespace-export-type.test.ts',
  'recall/runner-arguments-receiver/caller.test.ts',
  'recall/runner-eval-binding-guard/unsafe.test.ts',
  // A suite-local helper that registers inside a function its caller decides.
  'recall/concise-suite-hook-retention/run.test.ts',
  // A local helper called with computed arguments.
  'recall/local-helper-computed-arguments/run.test.ts',
])

/**
 * Compare whole-corpus non-comparable test records with the pinned blind-file inventory,
 * appending any failures.
 */
function checkBlindFiles(binary, failures) {
  const { records } = runAnalyzerJsonl(binary, [
    'fixtures',
    '--emit-test-fingerprints',
  ])
  const summary = records.find((record) => record.type === 'summary')
  if (summary === undefined) {
    failures.push(
      'FAIL blind files: the whole-tree run produced no summary record, so blindness over the ' +
        'fixture corpus was not measured at all',
    )
    return
  }
  // Derive the blind set from the per-test stream: a file is blind when it emitted at least one
  // executable-test record and every one of them says not comparable.
  // Derive the blind set from the per-test stream. The accumulator tracks `anyComparable`
  // deliberately: a first version tracked `all: true` and cleared it on every non-comparable
  // record, which computes "ANY record was non-comparable" -- a file with four excluded rows
  // beside three compared tests read as blind. The stream-vs-summary cross-check below caught
  // it on its first run (14 derived vs 13 reported), which is that check earning its place.
  const comparableByPath = new Map()
  for (const record of records) {
    if (record.type !== 'test' || typeof record.path !== 'string') continue
    // A todo or empty-bodied test has nothing to compare; it neither blinds nor clears a file.
    if (NOTHING_TO_COMPARE.has(record.exclusion)) continue
    const entry = comparableByPath.get(record.path) ?? { any: false, anyComparable: false }
    entry.any = true
    if (record.comparable === true) entry.anyComparable = true
    comparableByPath.set(record.path, entry)
  }
  const blind = new Set(
    [...comparableByPath.entries()]
      .filter(([, { any, anyComparable }]) => any && !anyComparable)
      .map(([path]) => path),
  )

  // Free consistency check: the analyzer's own aggregate must equal the set its stream implies.
  const reported = summary.blindFiles
  if (!Number.isInteger(reported) || reported < 0) {
    failures.push(
      `FAIL blind files: summary field blindFiles is ${JSON.stringify(reported)}, not a ` +
        'non-negative integer; the analyzer and this gate disagree about the report schema',
    )
    return
  }
  if (reported !== blind.size) {
    failures.push(
      `FAIL blind files: the summary reports ${reported} but the test stream implies ` +
        `${blind.size}. The aggregate and the records it aggregates disagree -- an analyzer ` +
        'defect, not a corpus change',
    )
  }

  // Oracle and recall cases record their own blind files, so adding a case touches no shared list.
  const recallManifest = JSON.parse(readFileSync('fixtures/recall/manifest.json', 'utf8'))
  const deliberate = new Set([
    ...DELIBERATE_BLIND_FILES,
    ...blindCaseFiles(readCases(process.cwd()).cases),
    ...blindRecallFiles(recallManifest),
  ])
  const missing = [...deliberate].filter((path) => !blind.has(path))
  const extra = [...blind].filter((path) => !deliberate.has(path))
  if (missing.length > 0 || extra.length > 0) {
    const detail = (label, paths) =>
      paths.length > 0 ? `\n  ${label}:\n${paths.sort().map((p) => `    ${p}`).join('\n')}` : ''
    failures.push(
      `FAIL blind files: the deliberate set drifted over fixtures/.${detail('no longer blind (recover the decision or update DELIBERATE_BLIND_FILES or the case\'s blindUnderCurrent)', missing)}${detail('newly blind (a fixture went blind for a reason the per-fixture incomplete expectations cannot see)', extra)}`,
    )
  } else {
    console.log(
      `blind files: ${blind.size} deliberate, set-equal (pinned by path in DELIBERATE_BLIND_FILES and the cases)`,
    )
  }
}

/**
 * Judge each owned fixture target and the blind-file pin, run the isolation gate, and exit nonzero
 * when an assertion fails.
 */
function main() {
  // Recall deltas belong to `check-recall.mjs`, which consumes them.
  const known = JSON.parse(readFileSync(KNOWN_PATH, 'utf8')).cases.filter(({ id }) => !id.startsWith('recall-'))
  const failures = knownProblems(known).map((message) => `FAIL ${message}`)
  const held = []
  const consumed = new Set()
  let total = 0
  for (const target of TARGETS) {
    const count = checkTarget(target, known, { failures, held, consumed })
    // A directory that silently holds no fixtures would pass over nothing.
    if (count === 0) {
      failures.push(
        `FAIL ${target.dir}: the directory holds no test-named fixtures. A scanned directory that ` +
          'contributes no assertions is indistinguishable from a passing one',
      )
    }
    total += count
  }
  for (const entry of known) {
    if (!consumed.has(entry.id)) {
      failures.push(`FAIL known entry "${entry.id}" no longer applies: its delta is gone or changed. Remove it -- ${entry.reason}`)
    }
  }
  // Before the printer, so a blind drift is printed rather than only counted.
  checkBlindFiles(binary, failures)

  let failed = false
  if (failures.length > 0) {
    for (const line of failures) console.error(line)
    console.error(`\n${failures.length} problem(s) across ${total} fixtures.`)
    failed = true
  } else {
    console.log(
      `ok: ${total} fixtures across ${TARGETS.map((t) => `${t.dir}/`).join(', ')} matched their ` +
        `declared pairwise expectations, ${held.length} known delta(s) held.`,
    )
  }
  for (const line of held.sort()) console.log(`  ${line}`)

  // Here rather than its own CI step, so a new fixture cannot satisfy one check and escape the
  // other: were the participants of a `rule=none` expectation ever close enough for the silence to
  // mean anything. Both run even when the first fails; they are independent defects.
  if (reportIsolation(runIsolation(binary)) !== 0) failed = true

  if (failed) process.exit(1)
}

main()

// `fixtures/near/` holds fixtures whose expected verdict today's binary cannot produce: either an
// unimplemented rule, or -- since near-arrow-vs-function-this-twin.test.ts moved there -- a known
// analyzer defect being recorded honestly instead of being pinned as correct behavior. They are
// deliberately outside this gate's finding assertions (asserting them would fail against today's
// binary) but they are NOT outside the isolation gate, which needs no binary and checks them along
// with fixtures/duplicates/. A directory nothing reports on is how work gets forgotten, so count
// them and say so, every run.
const pendingDir = 'fixtures/near'
if (existsSync(pendingDir)) {
  const pending = readdirSync(pendingDir).filter((f) => FIXTURE_NAME.test(f))
  if (pending.length > 0) {
    console.log(
      `note: ${pending.length} fixture(s) in ${pendingDir}/ are pending an unimplemented rule or ` +
        'an analyzer fix, and their findings are not asserted by this gate:',
    )
    for (const name of pending.sort()) console.log(`  ${name}`)
  }
}
