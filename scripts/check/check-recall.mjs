#!/usr/bin/env node
// Recall gate for `fixtures/recall/`.
//
// ---------------------------------------------------------------------------------------------
// Why this exists, and why it is not any of the controls that already did
// ---------------------------------------------------------------------------------------------
//
// This project had two corpus controls and neither one measured recall.
//
//   - `check-duplicates-corpus.mjs` asserts a hand-written expectation per fixture. Excellent at
//     what it does, and every expectation in it was written because someone had already thought of
//     the shape. A rule that never worked for a shape nobody wrote a fixture for is invisible to
//     it, and stays invisible forever.
//   - `run-external-corpus.mjs` asserts *known-positive retention*: previously confirmed true
//     positives must still be reported. That is retention, not recall, and the reason is
//     structural rather than a matter of degree -- **the labels are selected by the detector being
//     evaluated.** A rule can miss every reordered-setup pair, every `.each`-row duplicate, and
//     every `bench` declaration forever while retaining 100% of the exact duplicates it already
//     found. Before this gate, no labelled recall set existed at all.
//
// This gate is that set. `fixtures/recall/manifest.json` enumerates one self-contained case
// directory per rule behaviour, and each case declares three things independently of what the
// binary currently prints:
//
//   `expectedFindings`  behaviour the analyzer supports and must RETAIN.
//   `expectedAbsent`    intentional non-findings that protect precision. Not decoration: a recall
//                       set listing only what must be found cannot catch a rule that starts
//                       over-firing, and over-firing is how a recall number gets improved by
//                       cheating.
//   `knownMisses`       findings believed CORRECT that the analyzer does not produce today. They
//                       count against the recall floor rather than being written down as correct
//                       silence.
//   `knownFalsePositives` findings the analyzer PRODUCES today that are believed WRONG -- the
//                       mirror image of `knownMisses`, for extra-finding defects (a rule firing
//                       where it must stay silent). Listed with a reason, counted against
//                       `knownFalsePositiveBaseline`, asserted PRESENT: the day the finding stops
//                       appearing, the gate fails and demands the entry move to `expectedAbsent`
//                       with the baseline decremented. Without this field a recall case pinning a
//                       known false positive would go red on arrival with nothing to classify it,
//                       and could not be shipped at all.
//   `pending`           a CASE-level marker (reason required) for expectations today's binary
//                       cannot satisfy at all -- an extraction floor of 2 tests over a file the
//                       analyzer cannot yet discover, say. A pending case's assertions become a
//                       one-way ratchet, the same shape as `check-duplicates-corpus.mjs`'s
//                       per-expectation `@vitest-dedup:pending`: while any assertion still fails,
//                       the case holds and is printed; the day every assertion passes, the gate
//                       fails and demands the marker's removal. `expectedTests` must stay a
//                       positive integer even for a pending case -- a case that could never
//                       extract anything would measure nothing, and the floor exists precisely so
//                       that shape is written down instead of worked around.
//
// ---------------------------------------------------------------------------------------------
// The rule that makes it a barrier: expectations are established, never recorded
// ---------------------------------------------------------------------------------------------
//
// Every expectation in the manifest was derived by reading the case source and the rule that
// governs it, then run against the binary. Where the two disagreed and the binary was judged
// wrong, the manifest records the expectation believed correct and marks the case a known miss.
// A manifest generated from current output would be a snapshot, not an expectation, and would
// ratify today's bugs as the contract -- which is the failure mode `knownMissBaseline` exists to
// make impossible to reach quietly. When a disagreement is fixed, it moves from `knownMisses` to
// `expectedFindings` and the baseline ratchets down.
//
// ---------------------------------------------------------------------------------------------
// Floors on work performed (ss5a)
// ---------------------------------------------------------------------------------------------
//
// The repository-wide rule: every gate must assert a floor on work performed, not only that
// nothing failed. Five separate instances of silent success in this repository, each invisible
// because every assertion was of the form "X did not happen". This gate is a recall
// measurement, which makes it the single easiest place in the repository to score well by
// measuring less -- so it fails when:
//
//   - the manifest holds fewer than `minimumCases` cases (zero included);
//   - a case's directory is missing, or holds no analyzable source file;
//   - a case's run produces no summary record;
//   - a directory under `fixtures/recall/` is not referenced by any case (a case dropped in
//     without a manifest entry must fail, never be silently skipped);
//   - a case extracts a number of tests other than its declared `expectedTests`;
//   - any rule in the analyzer's rule set lacks both a positive and a negative case;
//   - any unordered pair of rules lacks a mixed-coverage entry (`mixedRuleCoverage`): a case
//     where both fire, or a recorded reason the pair cannot co-occur;
//   - the number of cases actually run differs from the number the manifest declares.
//
// The last one is the guard against this file itself: a `continue` added to the case loop for any
// reason would otherwise reduce the measurement silently.
//
// ---------------------------------------------------------------------------------------------
// distinct vs not-compared
// ---------------------------------------------------------------------------------------------
//
// Every case declares `expectedIncomplete`, and it is asserted exactly. `distinct` (compared,
// found different) and `not-compared` (never eligible -- unresolved context) both produce no
// finding, and conflating them is how a rule that silently analyzes nothing passes a precision
// gate. A case whose `expectedAbsent` holds only because its tests were never compared must say
// so, or its silence is worthless as evidence.
// This is the same `summary.incomplete` proxy `check-duplicates-corpus.mjs` uses, and it is usable
// here for the same reason: every case is its own directory and its own run.
import assert from 'node:assert/strict'
import { existsSync, readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { ALL_RULES } from './lib/fixture-directives.mjs'
import { validateManifestLabels } from './lib/manifest-schema.mjs'
import { describeMatcher, judgeRecallCase, newTotals, observeRecallCase } from './lib/recall-runner.mjs'
import { defaultBinary } from './lib/run-analyzer.mjs'

const binary = process.argv[2] ?? defaultBinary()
const root = process.argv[3] ?? 'fixtures/recall'

// `ALL_RULES` (lib/fixture-directives.mjs) is derived from the verdict table that rejects any
// `@vitest-dedup:expect` naming a rule no verdict admits, so a tenth rule arriving in `Rule::ALL`
// fails THIS gate until it has a positive and a negative recall case, instead of arriving with
// no recall coverage and nothing to say so.

// Per-case running and judging live in `lib/recall-runner.mjs` (`observeRecallCase`, then
// `judgeRecallCase`), so an evaluator can reuse the observation and classify instead of
// asserting. Everything the gate prints on a per-case basis is that module's output contract;
// the corpus-wide floors and the manifest validation stay here.

/**
 * Rejects a manifest that cannot be measured, before anything is run.
 *
 * Structural validation is separated from the per-case run for one reason: a malformed matcher is
 * indistinguishable, at assertion time, from a finding that did not appear. A matcher with neither
 * `titles` nor `locations` matches the FIRST finding of its rule, so as an `expectedFindings` entry
 * it would pass on the wrong finding and as an `expectedAbsent` entry it would fail on the right
 * one. Either way the case stops measuring what it claims to.
 */
function validateManifest(manifest, failures) {
  assert.equal(manifest.schemaVersion, 1, 'unsupported recall manifest schemaVersion')
  for (const field of [
    'minimumCases',
    'knownMissBaseline',
    'knownFalsePositiveBaseline',
    'minimumRecall',
    'recallDestination',
  ]) {
    if (typeof manifest[field] !== 'number') {
      failures.push(`FAIL manifest: \`${field}\` must be a number`)
    }
  }
  if (!Array.isArray(manifest.cases)) {
    failures.push('FAIL manifest: `cases` must be an array')
    return
  }
  if (manifest.cases.length < manifest.minimumCases) {
    failures.push(
      `FAIL manifest: ${manifest.cases.length} case(s) declared, floor is ${manifest.minimumCases}. ` +
        `A recall gate with nothing to measure reports perfect recall, which is why the floor is ` +
        `here and why lowering it is a reviewed edit rather than a side effect`,
    )
  }

  const names = new Set()
  const paths = new Set()
  const missIds = new Set()
  const roles = new Map()

  for (const testCase of manifest.cases) {
    const where = `case ${testCase.name ?? '<unnamed>'}`
    for (const field of ['name', 'path', 'rule', 'role']) {
      if (typeof testCase[field] !== 'string' || testCase[field] === '') {
        failures.push(`FAIL ${where}: \`${field}\` must be a non-empty string`)
      }
    }
    if (names.has(testCase.name)) failures.push(`FAIL ${where}: duplicate case name`)
    names.add(testCase.name)
    if (!ALL_RULES.has(testCase.rule)) {
      failures.push(`FAIL ${where}: unknown rule "${testCase.rule}"`)
    }
    if (!['positive', 'negative', 'variant'].includes(testCase.role)) {
      failures.push(`FAIL ${where}: role must be positive, negative, or variant`)
    }
    if (!Number.isInteger(testCase.expectedTests) || testCase.expectedTests < 1) {
      failures.push(
        `FAIL ${where}: \`expectedTests\` must be a positive integer -- a case extracting zero ` +
          `tests measures nothing, however many expectations it declares`,
      )
    }
    if (typeof testCase.expectedIncomplete !== 'boolean') {
      failures.push(
        `FAIL ${where}: \`expectedIncomplete\` must be declared. It is what separates "compared ` +
          `and found different" from "never compared"; a case that does not say which one its ` +
          `silence is cannot be read as evidence of either`,
      )
    }
    if (
      testCase.expectedUnresolved !== undefined &&
      (!Number.isInteger(testCase.expectedUnresolved) || testCase.expectedUnresolved < 0)
    ) {
      failures.push(`FAIL ${where}: \`expectedUnresolved\` must be a non-negative integer`)
    }
    if (testCase.arguments && !Array.isArray(testCase.arguments)) {
      failures.push(`FAIL ${where}: \`arguments\` must be an array of CLI tokens`)
    }
    if (
      testCase.pending !== undefined &&
      (typeof testCase.pending !== 'string' || testCase.pending.length < 20)
    ) {
      failures.push(
        `FAIL ${where}: \`pending\`, when set, must be a reason (20+ characters) naming the ` +
          `defect the case waits on -- an unexplained pending case is an unasserted expectation`,
      )
    }
    paths.add(testCase.path)

    const groups = [
      ['expectedFindings', testCase.expectedFindings, true],
      ['expectedAbsent', testCase.expectedAbsent, false],
      ['knownMisses', testCase.knownMisses ?? [], true],
      ['knownFalsePositives', testCase.knownFalsePositives ?? [], true],
    ]
    for (const [field, entries, needsSeverity] of groups) {
      if (!Array.isArray(entries)) {
        failures.push(`FAIL ${where}: \`${field}\` must be an array`)
        continue
      }
      for (const entry of entries) {
        const label = `${where} ${field} ${describeMatcher(entry)}`
        if (!ALL_RULES.has(entry.rule)) failures.push(`FAIL ${label}: unknown rule`)
        if (!entry.titles && !entry.locations) {
          failures.push(
            `FAIL ${label}: a matcher needs \`titles\` or \`locations\`. Rule alone matches the ` +
              `first finding of that rule, which passes on the wrong finding as an expectation ` +
              `and fails on the right one as a non-finding`,
          )
        }
        if (needsSeverity && !entry.severity) {
          failures.push(
            `FAIL ${label}: an asserted finding must declare its \`severity\`, so a rule silently ` +
              `downgraded to off still fails here`,
          )
        }
        if (!needsSeverity && entry.severity) {
          failures.push(
            `FAIL ${label}: \`expectedAbsent\` must not declare a severity -- absence is ` +
              `unconditional, and a severity would let the finding reappear at another level`,
          )
        }
      }
    }
    for (const miss of testCase.knownMisses ?? []) {
      if (!miss.id) failures.push(`FAIL ${where}: every known miss needs an \`id\``)
      if (!miss.reason || miss.reason.length < 40) {
        failures.push(
          `FAIL ${where}: known miss ${miss.id} needs a \`reason\` naming the mechanism. An ` +
            `unexplained known miss is an accepted regression wearing a different label`,
        )
      }
      if (missIds.has(miss.id)) failures.push(`FAIL ${where}: duplicate known-miss id ${miss.id}`)
      missIds.add(miss.id)
    }
    for (const miss of testCase.knownFalsePositives ?? []) {
      if (!miss.id) failures.push(`FAIL ${where}: every known false positive needs an \`id\``)
      if (
        miss.expectCount !== undefined &&
        (!Number.isInteger(miss.expectCount) || miss.expectCount < 1)
      ) {
        failures.push(
          `FAIL ${where}: known false positive ${miss.id} \`expectCount\` must be a positive ` +
            `integer -- the number of active findings the entry is declared to classify`,
        )
      }
      if (!miss.reason || miss.reason.length < 40) {
        failures.push(
          `FAIL ${where}: known false positive ${miss.id} needs a \`reason\` naming the ` +
            `mechanism. An unexplained known false positive is a suppressed finding wearing a ` +
            `different label`,
        )
      }
      if (missIds.has(miss.id)) {
        failures.push(`FAIL ${where}: duplicate known id ${miss.id} (known-miss and known-false-positive ids share one namespace)`)
      }
      missIds.add(miss.id)
    }

    const seen = roles.get(testCase.rule) ?? new Set()
    // A `variant` case that asserts nothing but absence is a negative for coverage purposes: the
    // role field records the case's intent, and this records what it actually establishes.
    if (testCase.role === 'positive' || testCase.expectedFindings?.length) seen.add('positive')
    if (testCase.role === 'negative' || testCase.expectedAbsent?.length) seen.add('negative')
    roles.set(testCase.rule, seen)
  }

  // The owner's requirement, machine-checked: for every rule, a positive and a negative.
  for (const rule of [...ALL_RULES].sort()) {
    const seen = roles.get(rule) ?? new Set()
    for (const kind of ['positive', 'negative']) {
      if (!seen.has(kind)) {
        failures.push(
          `FAIL rule ${rule}: no ${kind} recall case. Every rule needs both -- a precision fix and ` +
            `a deleted rule produce the same silence, and a rule that never fired produces the ` +
            `same success as one that fires correctly`,
        )
      }
    }
  }

  // Label fields (origin, verdict, classifiedAt) and unknown keys: `lib/manifest-schema.mjs`.
  failures.push(...validateManifestLabels(manifest))

  validateMixedCoverage(manifest, failures)

  // An orphan directory is the unclassified-fixture hazard `check-duplicates-corpus.mjs` closes by
  // failing to parse: a case dropped in here without a manifest entry would simply never run.
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue
    if (!paths.has(entry.name)) {
      failures.push(
        `FAIL ${join(root, entry.name)}: directory is not referenced by any manifest case, so ` +
          `nothing runs it. Add a case or delete the directory`,
      )
    }
  }
}

// The mixed-rule inventory: one entry per unordered pair of distinct rules, each naming the case
// that covers it (or recording that the pair cannot co-occur, with the structural reason).
//
// Why a per-pair inventory at all: no single-rule case can observe interference, because every
// `-positive` holds exactly one defect. The manifest therefore declares, for every pair of rules,
// which case carries BOTH defects -- and the gate derives the pair set from `ALL_RULES` the same
// way it derives the positive/negative requirement, so a tenth rule arrives with 9 uncovered
// pairs that fail here until they are covered or recorded as impossible.
//
// Coverage means BOTH rules fire in the named case (`expectedFindings` of each). A finding of one
// rule plus an `expectedAbsent` of the other is not coverage of a pair that can co-occur: the
// interesting fact about co-occurring rules is that neither eats the other's finding, and a
// suppression-shaped interaction is pinned INSIDE a covering case by its own `expectedAbsent`
// entries (see `mixed-suite-copy-with-internal-duplicates`), not by pointing the inventory at a
// case where one of the two never fires.
/**
 * Check the manifest inventory for each unordered rule pair and append missing or invalid
 * mixed-rule coverage diagnostics.
 */
function validateMixedCoverage(manifest, failures) {
  if (!Array.isArray(manifest.mixedRuleCoverage)) {
    failures.push(
      `FAIL manifest: \`mixedRuleCoverage\` must be an array with one entry per unordered pair of ` +
        `distinct rules. Mixed-rule interference is invisible to single-rule cases, so every pair ` +
        `is either covered by a case or recorded as unable to co-occur`,
    )
    return
  }
  const byName = new Map(manifest.cases.map((testCase) => [testCase.name, testCase]))
  const pairKey = (a, b) => [a, b].sort().join('\n')

  const expectedPairs = new Set()
  const rules = [...ALL_RULES]
  for (let i = 0; i < rules.length; i += 1) {
    for (let j = i + 1; j < rules.length; j += 1) {
      expectedPairs.add(pairKey(rules[i], rules[j]))
    }
  }
  const seen = new Set()
  for (const entry of manifest.mixedRuleCoverage) {
    const where = `mixedRuleCoverage ${JSON.stringify(entry?.pair ?? entry)}`
    if (!Array.isArray(entry?.pair) || entry.pair.length !== 2) {
      failures.push(`FAIL ${where}: \`pair\` must name exactly two rules`)
      continue
    }
    const [a, b] = entry.pair
    if (!ALL_RULES.has(a) || !ALL_RULES.has(b) || a === b) {
      failures.push(
        `FAIL ${where}: both members must be distinct rules the analyzer implements`,
      )
      continue
    }
    if (seen.has(pairKey(a, b))) {
      failures.push(`FAIL ${where}: this pair is recorded more than once`)
      continue
    }
    seen.add(pairKey(a, b))

    const hasCase = typeof entry.case === 'string' && entry.case !== ''
    const hasCannot = typeof entry.cannotCoOccur === 'string' && entry.cannotCoOccur.length >= 20
    if (hasCase === hasCannot) {
      failures.push(
        `FAIL ${where}: an entry names exactly one of \`case\` (the case covering the pair) or ` +
          `\`cannotCoOccur\` (a reason of 20+ characters naming the structural impossibility)`,
      )
      continue
    }
    if (!hasCase) continue
    const testCase = byName.get(entry.case)
    if (!testCase) {
      failures.push(`FAIL ${where}: names case "${entry.case}", which the manifest does not declare`)
      continue
    }
    const firing = new Set((testCase.expectedFindings ?? []).map((finding) => finding.rule))
    for (const rule of [a, b]) {
      if (!firing.has(rule)) {
        failures.push(
          `FAIL ${where}: names case "${entry.case}", whose \`expectedFindings\` include no ` +
            `\`${rule}\` finding. Coverage means BOTH rules fire in the case -- one firing and ` +
            `one \`expectedAbsent\` does not observe the interference this inventory exists for`,
        )
      }
    }
  }
  for (const missing of expectedPairs) {
    if (!seen.has(missing)) {
      const [a, b] = missing.split('\n')
      failures.push(
        `FAIL mixedRuleCoverage: the pair [${a}, ${b}] is not recorded. Every pair of rules is ` +
          `either covered by a mixed case or recorded with \`cannotCoOccur\` naming why no single ` +
          `run can hold both defects`,
      )
    }
  }
}

/**
 * The gate body: structurally validate the manifest (aborting before any run if it cannot be
 * measured), execute every case against the binary, and enforce the recall, precision and
 * work-performed floors over what actually ran. Exits 1 on any failure, printing each one.
 *
 * The binary and corpus root come from argv so the gate is runnable both directly
 * (`node scripts/check/check-recall.mjs [binary] [root]`) and from CI's working directory
 * without arguments.
 */
function main() {
  const failures = []
  const manifestPath = join(root, 'manifest.json')
  assert.ok(existsSync(manifestPath), `missing recall manifest at ${manifestPath}`)
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))
  validateManifest(manifest, failures)
  // Any structural failure aborts, not only a missing `cases` array. The case loop below
  // dereferences `testCase.expectedFindings` and `testCase.expectedAbsent` unguarded, so a case
  // missing either one used to die with a `TypeError` *before* the diagnosis validateManifest had
  // already collected was ever printed -- the operator got a stack trace naming a line number
  // instead of the list naming the field.
  if (failures.length > 0) {
    for (const failure of failures) console.error(failure)
    console.error(`\n${failures.length} recall manifest failure(s). The corpus was not run.`)
    process.exit(1)
  }

  const pendingHeld = []
  const totals = newTotals()

  for (const testCase of manifest.cases) {
    judgeRecallCase(
      testCase,
      observeRecallCase({ binary, root, testCase }),
      { failures, pendingHeld, totals },
    )
  }

  if (totals.ran !== manifest.cases.length) {
    failures.push(
      `FAIL: ${totals.ran} of ${manifest.cases.length} manifest case(s) actually ran. A recall ` +
        `figure computed over a subset of the corpus is not the corpus's recall`,
    )
  }
  if (totals.testsAnalyzed === 0) {
    failures.push('FAIL: the whole corpus extracted zero tests; nothing was measured')
  }
  if (totals.knownMisses !== manifest.knownMissBaseline) {
    failures.push(
      `FAIL: ${totals.knownMisses} known miss(es) against a baseline of ${manifest.knownMissBaseline}. ` +
        `The baseline ratchets in one direction: a fix moves an entry to \`expectedFindings\` and ` +
        `decrements it. Raising it to make this gate pass converts a recall gap into the new normal`,
    )
  }
  if (totals.knownFalsePositivesTotal !== manifest.knownFalsePositiveBaseline) {
    failures.push(
      `FAIL: ${totals.knownFalsePositivesTotal} known false positive(s) against a baseline of ` +
        `${manifest.knownFalsePositiveBaseline}. The baseline ratchets in one direction: a fix ` +
        `deletes the entry, leaves the \`expectedAbsent\` matcher behind, and decrements it. ` +
        `Raising it to make this gate pass converts a precision gap into the new normal`,
    )
  }

  const desired = totals.expectedFindings + totals.knownMisses
  const recall = desired === 0 ? 0 : totals.detected / desired
  if (desired === 0) {
    failures.push('FAIL: no desired findings declared anywhere; recall is undefined, not perfect')
  } else if (recall < manifest.minimumRecall) {
    failures.push(
      `FAIL: recall ${(recall * 100).toFixed(1)}% is below the floor of ` +
        `${(manifest.minimumRecall * 100).toFixed(1)}%`,
    )
  }

  if (failures.length > 0) {
    for (const failure of failures) console.error(failure)
    console.error(`\n${failures.length} recall failure(s).`)
    process.exit(1)
  }

  if (pendingHeld.length > 0) {
    console.log(
      `note: ${pendingHeld.length} pending case(s) held -- the defect each waits on is still ` +
        'present, and the gate fails the moment one starts satisfying its expectations:',
    )
    for (const line of pendingHeld) console.log(`  ${line}`)
  }
  console.log(
    `Recall corpus: ${totals.ran} case(s) across ${ALL_RULES.size} rule(s), ${totals.testsAnalyzed} test(s) ` +
      `extracted, ${totals.detected}/${totals.expectedFindings} expected finding(s) matched, ${totals.knownMisses} ` +
      `known miss(es), ${totals.knownFalsePositivesTotal} known false positive(s) (${pendingHeld.length} ` +
      `pending case(s) excluded from the recall figure).`,
  )
  console.log(
    `Recall ${(recall * 100).toFixed(1)}% (floor ${(manifest.minimumRecall * 100).toFixed(1)}%, ` +
      `destination ${(manifest.recallDestination * 100).toFixed(1)}%).`,
  )
}

main()
