// The recall corpus runner, split so the per-case work can be reused. Observing a case (checking
// its directory, running the binary, reading the jsonl records) touches the filesystem and judges
// nothing; judging it (every per-case assertion the gate makes) reads only the observation and
// routes failures exactly as the gate's inline loop did, so the split cannot change a message, an
// ordering or an exit code.

import { existsSync, readdirSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { runAnalyzerJsonl } from './run-analyzer.mjs'

// Extensions discovery can reach. Only used to assert a case directory is not empty of analyzable
// source -- the analyzer's own `include`/`exclude` decision is the real authority, and a case whose
// files it declines is caught by the `expectedTests` assertion, not here.
const SOURCE_RE = /\.(?:[cm]?[jt]sx?)$/

/** `rule + sorted titles + sorted locations` is how a matcher addresses one finding. */
export function findingMatches(finding, expected) {
  if (finding.rule !== expected.rule) return false
  if (expected.severity && finding.severity !== expected.severity) return false
  if (expected.titles) {
    const actual = [...finding.titles].sort()
    const wanted = [...expected.titles].sort()
    if (actual.length !== wanted.length) return false
    if (!actual.every((title, index) => title === wanted[index])) return false
  }
  if (expected.locations) {
    // Sorted, because participant order is a presentation detail that differs by rule: a
    // `redundant-hook` pair over `afterEach`/`afterAll` reports innermost-first, since
    // `Context::hooks` is the chain and after-hooks run inward-out. A matcher that depended on
    // that order would encode the reporting convention rather than the finding's identity.
    const actual = finding.locations.map((loc) => `${loc.path}:${loc.line}`).sort()
    const wanted = [...expected.locations].sort()
    if (actual.length !== wanted.length) return false
    if (!actual.every((loc, index) => loc === wanted[index])) return false
  }
  return true
}

/**
 * Render a rule matcher with any severity, title and location constraints for failure
 * diagnostics.
 */
export function describeMatcher(matcher) {
  const parts = [matcher.rule]
  if (matcher.severity) parts.push(`[${matcher.severity}]`)
  if (matcher.titles) parts.push(`{${matcher.titles.join(' | ')}}`)
  if (matcher.locations) parts.push(`@ ${matcher.locations.join(' + ')}`)
  return parts.join(' ')
}

/**
 * The expectations a finding satisfies. Exactly one is the contract: zero means unclassified, and
 * more than one means overlapping expectations -- one finding satisfying two matchers while a
 * second goes unaccounted is precisely the shape the per-case totals cannot see, because the
 * counts still balance.
 *
 * @param {object} finding An active finding record from the jsonl stream.
 * @param {object[]} expectedFindings The case's `expectedFindings` matchers.
 * @returns {object[]}
 */
export function findingOwners(finding, expectedFindings) {
  return expectedFindings.filter((expected) => findingMatches(finding, expected))
}

/**
 * Fresh corpus-wide counters for {@link judgeRecallCase}. One constructor, so a counter the judge
 * adds cannot be missing from a caller: an absent field makes `+= 1` produce `NaN`, and the
 * gate's `=== 0` floor on `testsAnalyzed` is false for `NaN`.
 *
 * @returns {{detected: number, expectedFindings: number, knownMisses: number,
 *   knownFalsePositivesTotal: number, ran: number, testsAnalyzed: number}}
 */
export function newTotals() {
  return {
    detected: 0,
    expectedFindings: 0,
    knownMisses: 0,
    knownFalsePositivesTotal: 0,
    ran: 0,
    testsAnalyzed: 0,
  }
}

/**
 * Runs one recall case and returns what it produced. Nothing is asserted: a case whose directory
 * is missing or holds no analyzable source is returned as a fact for the judge to fail on, and an
 * operational run failure (a missing binary, a crash exit) propagates as the throw
 * `runAnalyzerJsonl` always raised -- observation must not be able to decide that a case measured
 * anything.
 *
 * @param {object} options
 * @param {string} options.binary Path to the analyzer binary.
 * @param {string} options.root Recall corpus root (the directory holding `manifest.json`).
 * @param {object} options.testCase Recall manifest case.
 * @returns {{label: string, caseRoot: string, directoryPresent: boolean, sourceFiles: string[],
 *   records: object[] | null, summary: object | null}} The case's run label and resolved root;
 *   whether its directory exists; the analyzable source files found under it; the parsed jsonl
 *   records and the summary record (all `null` when the case never ran -- the run is skipped for
 *   a missing or sourceless directory exactly as the gate's inline loop skipped it, so the judge
 *   sees the same facts it always did); `summary` is `null` when the run wrote no summary record.
 */
export function observeRecallCase({ binary, root, testCase }) {
  const label = `${testCase.name} (${testCase.path})`
  const caseRoot = resolve(root, testCase.path)
  const directoryPresent = existsSync(caseRoot) && statSync(caseRoot).isDirectory()
  if (!directoryPresent) {
    return { label, caseRoot, directoryPresent, sourceFiles: [], records: null, summary: null }
  }
  // Recursive, because a case may hold SUBDIRECTORIES -- the `cross-file-duplicate` shapes need
  // two same-named helper files that cannot share one directory (the plan's D4). Only used to
  // assert a case is not empty of analyzable source; the analyzer's own include/exclude
  // decision remains the real authority, caught by `expectedTests`.
  const sourceFiles = readdirSync(caseRoot, { recursive: true }).filter((name) =>
    SOURCE_RE.test(String(name).split(/[\\/]/).pop() ?? name),
  )
  if (sourceFiles.length === 0) {
    return { label, caseRoot, directoryPresent, sourceFiles, records: null, summary: null }
  }
  const records = runAnalyzerJsonl(binary, [caseRoot, ...(testCase.arguments ?? [])]).records
  return {
    label,
    caseRoot,
    directoryPresent,
    sourceFiles,
    records,
    summary: records.find((record) => record.type === 'summary') ?? null,
  }
}

/**
 * Judges one observation against its case's expectations, appending failure lines and pending
 * notes and accumulating the corpus-wide counters exactly as the gate's inline loop did: the
 * messages, their routing (operational failures and both ratchets straight to `failures`,
 * expectation problems held for the `pending` decision) and their order are the gate's output
 * contract and move verbatim.
 *
 * @param {object} testCase Recall manifest case.
 * @param {object} observation Result of {@link observeRecallCase}.
 * @param {object} context
 * @param {string[]} context.failures Corpus-wide failure lines, appended in the gate's order.
 * @param {string[]} context.pendingHeld Held pending-case notes, printed on success.
 * @param {{detected: number, expectedFindings: number, knownMisses: number,
 *   knownFalsePositivesTotal: number, ran: number, testsAnalyzed: number}} context.totals
 *   Corpus-wide counts, updated as each expectation is judged.
 */
export function judgeRecallCase(testCase, observation, { failures, pendingHeld, totals }) {
  const { label, directoryPresent, sourceFiles, records, summary } = observation
  // Assertions land here first and are promoted to failures only for a case that is not
  // `pending`; see the marker's description in the gate's header for the ratchet.
  const problems = []

  if (!directoryPresent) {
    failures.push(
      `FAIL ${label}: case directory is missing. A vanished case cannot fail its own ` +
        `expectations, so it has to fail here`,
    )
    return
  }
  if (sourceFiles.length === 0) {
    failures.push(`FAIL ${label}: case directory holds no analyzable source file`)
    return
  }
  if (!summary) {
    failures.push(`FAIL ${label}: run produced no summary record`)
    return
  }
  // Type before arithmetic, for the reason `.github/workflows/ci.yml`'s smoke test now states:
  // a renamed field makes this accumulator `NaN`, and `NaN === 0` is false, so the corpus-wide
  // "nothing was measured" floor below would pass over a corpus that measured nothing.
  if (!Number.isInteger(summary.testsAnalyzed) || summary.testsAnalyzed < 0) {
    failures.push(
      `FAIL ${label}: the summary record carries no non-negative integer \`testsAnalyzed\` ` +
        `(got ${JSON.stringify(summary.testsAnalyzed)}); the analyzer and this gate disagree ` +
        `about the report schema`,
    )
    return
  }
  totals.ran += 1
  totals.testsAnalyzed += summary.testsAnalyzed

  if (summary.testsAnalyzed !== testCase.expectedTests) {
    problems.push(
      `FAIL ${label}: expected ${testCase.expectedTests} extracted test(s), got ` +
        `${summary.testsAnalyzed}`,
    )
  }
  if (summary.incomplete !== testCase.expectedIncomplete) {
    problems.push(
      `FAIL ${label}: expected incomplete=${testCase.expectedIncomplete}, got ` +
        `${summary.incomplete}. ${
          testCase.expectedIncomplete
            ? 'This case\'s silence is only meaningful as `not-compared`; a complete run means ' +
              'the pair WAS compared and the absent finding is now a real miss'
            : 'This case\'s silence is claimed as `distinct`; an incomplete run means something ' +
              'was excluded from comparison and the silence proves nothing'
        }`,
    )
  }
  if (
    testCase.expectedUnresolved !== undefined &&
    summary.unresolvedTests !== testCase.expectedUnresolved
  ) {
    problems.push(
      `FAIL ${label}: expected ${testCase.expectedUnresolved} unresolved test(s), got ` +
        `${summary.unresolvedTests ?? '<missing>'}`,
    )
  }

  const active = records.filter((record) => record.type === 'finding' && !record.baselined)

  // Known false positives first: every finding an entry matches is claimed by it, excused from
  // `expectedAbsent`, and admitted by the unclassified-finding sweep below. An entry that no
  // longer matches anything is the ratchet firing: the defect it named is fixed, and the entry
  // must move to `expectedAbsent` with the baseline decremented.
  const fpClaimed = new Set()
  for (const falsePositive of testCase.knownFalsePositives ?? []) {
    totals.knownFalsePositivesTotal += 1
    const expectedCount = falsePositive.expectCount ?? 1
    const matches = active.filter((finding) => findingMatches(finding, falsePositive))
    if (matches.length === 0) {
      // A ratchet firing, not an unsatisfied expectation: the defect this entry named is FIXED.
      // Routed straight to failures so a pending case (whose other assertions still hold it
      // open) can never swallow the one signal that says the manifest is now stale.
      failures.push(
        `FAIL ${label}: known false positive ${falsePositive.id} is no longer reported -- the ` +
          `fix it waited on has landed. Delete the entry and decrement ` +
          `\`knownFalsePositiveBaseline\`, leaving \`expectedAbsent\` as the assertion`,
      )
    } else if (matches.length !== expectedCount) {
      problems.push(
        `FAIL ${label}: known false positive ${falsePositive.id} matched ${matches.length} ` +
          `active finding(s), expected ${expectedCount}` +
          (expectedCount === 1 ? '' : ' as declared by \`expectCount\`') +
          `. Tighten the matcher (titles, locations) or correct \`expectCount\` -- an entry ` +
          `that silently absorbs extra findings of its shape is a suppressed finding wearing ` +
          `a classified label`,
      )
    } else {
      for (const finding of matches) fpClaimed.add(finding)
    }
  }

  if (!testCase.pending) totals.expectedFindings += testCase.expectedFindings.length
  for (const expected of testCase.expectedFindings) {
    const matches = active.filter((finding) => findingMatches(finding, expected))
    if (matches.length !== 1) {
      const sameRule = active
        .filter((finding) => finding.rule === expected.rule)
        .map((finding) => `${finding.severity} {${finding.titles.join(' | ')}}`)
      problems.push(
        `FAIL ${label}: expected exactly 1 ${describeMatcher(expected)}, got ${matches.length}. ` +
        `Findings of that rule in this case: ${sameRule.length ? sameRule.join('; ') : 'none'}`,
      )
    } else if (!testCase.pending) {
      totals.detected += 1
    }
  }

  for (const expected of testCase.expectedAbsent) {
    // A finding claimed by a known false positive of this same case is already classified; it
    // is exactly the finding `expectedAbsent` names as the destination. Everything else must
    // genuinely be absent.
    const matches = active.filter(
      (finding) => findingMatches(finding, expected) && !fpClaimed.has(finding),
    )
    if (matches.length !== 0) {
      problems.push(
        `FAIL ${label}: ${describeMatcher(expected)} must NOT be reported, got ` +
        `${matches.length}. This is a precision regression: the case declares this pair is ` +
        `not what the rule claims`,
      )
    }
  }

  for (const miss of testCase.knownMisses ?? []) {
    totals.knownMisses += 1
    // Two assertions, because a known miss must fail loudly the moment it is FIXED -- otherwise
    // the baseline drifts out of step with reality and the recall number understates the tool.
    const matches = active.filter((finding) => findingMatches(finding, miss))
    if (matches.length !== 0) {
      failures.push(
        `FAIL ${label}: known miss ${miss.id} is now reported. Move it to \`expectedFindings\` ` +
        `and decrement \`knownMissBaseline\``,
      )
    }
    // Stronger form, applied only where the case expects nothing else from that rule: the
    // matcher's `titles` are a considered guess at the shape of a finding that does not exist
    // yet, so "no finding of this rule at all in this case" is the claim that does not depend on
    // the guess being right.
    const ruleAlsoExpected = testCase.expectedFindings.some((entry) => entry.rule === miss.rule)
    if (!ruleAlsoExpected) {
      // A finding already claimed by this case's own known false positive is classified, not
      // news: the pair above owns it. Only unclaimed findings of the rule contradict the
      // miss's "reports nothing here" claim.
      const anyOfRule = active.filter(
        (finding) => finding.rule === miss.rule && !fpClaimed.has(finding),
      )
      if (anyOfRule.length !== 0) {
        failures.push(
          `FAIL ${label}: known miss ${miss.id} claims ${miss.rule} reports nothing here, but ` +
          `${anyOfRule.length} ${miss.rule} finding(s) appeared. Either the miss is fixed ` +
          `(move it to \`expectedFindings\`) or the rule is firing on the wrong participants`,
        )
      }
    }
  }

  // Unclassified-finding sweep. Without it a case could satisfy every declared expectation while
  // the analyzer also reported three things nobody asked about -- which is how a recall number
  // survives a precision collapse. Findings claimed by a known false positive are classified by
  // definition; everything else must be accounted for.
  if (active.length !== testCase.expectedFindings.length + fpClaimed.size) {
    const unclaimed = active
      .filter(
        (finding) =>
          !fpClaimed.has(finding) &&
          findingOwners(finding, testCase.expectedFindings).length === 0,
      )
      .map((finding) => `${finding.rule} [${finding.severity}] {${finding.titles.join(' | ')}}`)
    problems.push(
      `FAIL ${label}: ${active.length} active finding(s) against ` +
      `${testCase.expectedFindings.length} declared and ${fpClaimed.size} classified as known ` +
      `false positive(s). Unclassified: ${
        unclaimed.length ? unclaimed.join('; ') : '(none -- a declared matcher matched twice)'
      }`,
    )
  }

  // The pending ratchet, last: a pending case holds while ANY assertion still fails, and fails
  // the moment every one of them passes, so the marker can never outlive the defect it waits
  // on. Operational failures above (missing directory, unreadable summary) are never excused:
  // a pending case the gate cannot even measure is a broken case, not a held one.
  if (testCase.pending) {
    if (problems.length === 0) {
      failures.push(
        `FAIL ${label}: pending case now satisfies every declared expectation -- remove the ` +
          `\`pending\` field so the expectations become hard assertions`,
      )
    } else {
      pendingHeld.push(`${label}: ${problems.length} assertion(s) still failing -- ${testCase.pending}`)
    }
  } else {
    failures.push(...problems)
  }
}
