// The corpus evaluator's measurement, separated from its command line so every classification is
// reachable from a unit test with a fabricated observation.
//
// It reports what one binary does against the labelled recall corpus: per expectation, whether the
// run satisfied it; per rule, the denominators behind the satisfied counts; every finding nobody
// labelled; and, for each silence the corpus claims, whether the pair was eligible for comparison. Nothing
// here judges a run pass or fail except the work floors (`measurementProblems`): a contract
// violation is data, a measurement that did not measure is an error.
//
// Matching reuses `findingMatches`, the recall gate's own matcher, so the evaluator and the gate
// cannot disagree about which finding an expectation addresses.
import { readFileSync, realpathSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'

import { ALL_RULES } from './fixture-directives.mjs'
import { findingMatches } from './recall-runner.mjs'

/** The comparison keys a `test` record carries, in the order a difference is reported. */
export const KEY_FIELDS = Object.freeze(['alpha', 'structural', 'behavior', 'contextKey', 'modifierKey'])

/** The severities a finding record can carry. */
const SEVERITIES = Object.freeze(['off', 'warning', 'error'])

/** Version of the JSON report. */
export const REPORT_SCHEMA_VERSION = 1

/**
 * `numerator / denominator`, or `null` when there is nothing to divide by. A zero denominator is
 * "no data", never 0% and never 100%; the text report prints it as `n/a`.
 *
 * @param {number} numerator Count of the satisfied population.
 * @param {number} denominator Size of the population.
 * @returns {number | null}
 */
export function ratio(numerator, denominator) {
  return denominator === 0 ? null : numerator / denominator
}

/**
 * Problems that make a record unusable as evidence. The evaluator divides by what it reads, so a
 * record that parses but lacks a field would shrink a denominator instead of failing.
 *
 * @param {object[]} records The parsed jsonl records of one case run.
 * @param {string} label The case, for the message.
 * @returns {string[]}
 */
function recordProblems(records, label) {
  const problems = []
  const isTextList = (value) => Array.isArray(value) && value.every((entry) => typeof entry === 'string')
  records.forEach((record, index) => {
    const where = `${label}: record ${index + 1}`
    if (record === null || typeof record !== 'object' || typeof record.type !== 'string') {
      problems.push(`${where} is not a typed object`)
    } else if (record.type === 'finding') {
      if (!ALL_RULES.has(record.rule) || !SEVERITIES.includes(record.severity)) {
        problems.push(`${where}: a finding needs a known \`rule\` and a \`severity\` of ${SEVERITIES.join(', ')}`)
      }
      if (!isTextList(record.titles)) problems.push(`${where}: a finding needs a \`titles\` list`)
      if (record.baselined !== undefined && typeof record.baselined !== 'boolean') {
        problems.push(`${where}: \`baselined\` must be a boolean when present`)
      }
      if (
        !Array.isArray(record.locations) ||
        !record.locations.every((loc) => typeof loc?.path === 'string' && Number.isInteger(loc.line))
      ) {
        problems.push(`${where}: a finding needs \`locations\` with a path and line each`)
      }
    } else if (record.type === 'test' || record.type === 'hook') {
      if (typeof record.path !== 'string' || !Number.isInteger(record.line)) {
        problems.push(`${where}: a test record needs a \`path\` and a \`line\``)
      }
      if (record.type === 'test' ? typeof record.title !== 'string' : typeof record.kind !== 'string') {
        problems.push(`${where}: a ${record.type} record needs a string ${record.type === 'test' ? '`title`' : '`kind`'}`)
      }
      // The keys are what a silence is judged by; a record without them would read as equal keys.
      const keys = record.type === 'test' ? KEY_FIELDS : KEY_FIELDS.slice(0, 3)
      const missing = keys.filter((field) => typeof record[field] !== 'string' || record[field] === '')
      if (missing.length > 0) {
        problems.push(`${where}: a ${record.type} record lacks ${missing.map((field) => `\`${field}\``).join(', ')}`)
      }
      // A hook is always compared against its own kind, so its record carries no `comparable`.
      const comparableOk = record.type === 'hook' ? record.comparable === undefined : typeof record.comparable === 'boolean'
      if (!comparableOk || typeof record.trivial !== 'boolean') {
        problems.push(`${where}: a ${record.type} record needs a boolean \`trivial\`${record.type === 'test' ? ' and \`comparable\`' : ''}`)
      }
    }
  })
  return problems
}

/** Rules whose claim is about titles, so a trivial body does not take a test out of comparison. */
const IGNORES_TRIVIALITY = new Set(['duplicate-test-name'])

/**
 * Rules whose default severity is `off`, so they run only when a case turns them on. Mirrors
 * `Rule::default_severity` in `src/model.rs`; a unit test fails when the two disagree.
 */
export const DEFAULT_OFF_RULES = Object.freeze(['subsumed-test'])

/** The configuration file name the analyzer looks for above the analysis root. */
const CONFIG_FILE_NAME = '.vitest-dedup.json'

/** Whether `path` is a regular file (or a link to one), as the analyzer's discovery asks. */
function isFile(path) {
  try {
    return statSync(path).isFile()
  } catch {
    return false
  }
}

/**
 * The configuration path in a case's CLI arguments: `--config FILE`, `--config=FILE`, `-c FILE`,
 * `-cFILE` or `-c=FILE`. The analyzer resolves a relative path against the analyzed directory.
 *
 * @param {string[]} argumentsList The case's CLI arguments.
 * @returns {string | null} The path as written, or `null` when none is given.
 */
export function explicitConfigPath(argumentsList) {
  let found = null
  argumentsList.forEach((token, index) => {
    if (token === '--config' || token === '-c') found = argumentsList[index + 1] ?? ''
    else if (token.startsWith('--config=')) found = token.slice(9)
    else if (token.startsWith('-c') && !token.startsWith('--')) found = token.slice(2).replace(/^=/, '')
  })
  return found
}

/**
 * The `rules` a case's configuration file sets: the file named by `--config`, else the first
 * `.vitest-dedup.json` found walking up from the case directory, as the analyzer resolves it.
 *
 * @param {string | undefined} caseRoot The analyzed directory; no lookup happens without one.
 * @param {string[]} argumentsList The case's CLI arguments.
 * @returns {Record<string, string>} Rule id to severity; empty when there is no readable file.
 */
export function configuredRules(caseRoot, argumentsList) {
  if (caseRoot === undefined) return {}
  // The analyzer canonicalizes the analysis root before it looks for configuration.
  try {
    caseRoot = realpathSync(caseRoot)
  } catch {
    // A directory that cannot be resolved is judged by the path as given.
  }
  const explicit = explicitConfigPath(argumentsList)
  let path = null
  if (explicit !== null) {
    path = resolve(caseRoot, explicit)
  } else {
    for (let dir = resolve(caseRoot); path === null; dir = dirname(dir)) {
      if (isFile(join(dir, CONFIG_FILE_NAME))) path = join(dir, CONFIG_FILE_NAME)
      if (dirname(dir) === dir) break
    }
  }
  try {
    const rules = JSON.parse(readFileSync(path, 'utf8')).rules
    return rules !== null && typeof rules === 'object' ? rules : {}
  } catch {
    return {}
  }
}

/**
 * The rules a case leaves switched off, by the analyzer's precedence: each rule's default (the
 * default-off ones are off), then the configuration file, then `--rule <rule>=<severity>` (or
 * `--rule=<rule>=<severity>`; the last setting wins). A rule that is off compares nothing, so its
 * silences say nothing.
 *
 * @param {string[]} argumentsList The case's CLI arguments.
 * @param {Record<string, string>} [configured] Severities from the case's configuration file.
 * @returns {Set<string>}
 */
export function disabledRules(argumentsList, configured = {}) {
  const severities = new Map(DEFAULT_OFF_RULES.map((rule) => [rule, 'off']))
  for (const [rule, severity] of Object.entries(configured)) severities.set(rule, severity)
  argumentsList.forEach((token, index) => {
    const value = token === '--rule' ? argumentsList[index + 1] : token.startsWith('--rule=') ? token.slice(7) : undefined
    const [rule, severity] = (value ?? '').split('=')
    if (severity !== undefined) severities.set(rule, severity)
  })
  return new Set([...severities].filter(([, severity]) => severity === 'off').map(([rule]) => rule))
}

/** The name a `test` or `hook` record answers to in a matcher's `titles`. */
function recordName(record) {
  return record.title ?? record.kind
}

/**
 * The record types a rule's participants appear as. Suites are not emitted at all, and a hook is
 * not a test, so a claim resolved against the wrong type would read as comparison evidence.
 *
 * @param {string} rule The rule the claim forbids.
 * @returns {string[]} Record types; empty when the rule's participants have no record.
 */
function participantTypes(rule) {
  if (rule === 'duplicate-suite') return []
  return rule === 'redundant-hook' ? ['hook'] : ['test']
}

/**
 * Resolves the tests or hooks a silence claim names, by location (`path:line`) when the matcher has
 * locations and by title otherwise. A claim is resolved only when every named participant maps to
 * a record and no name is ambiguous: a title repeated `k` times in the matcher must match exactly
 * `k` records. A partial map would read as a smaller, easier population.
 *
 * @param {{titles: string[] | null, locations: string[] | null}} expectation The silence claim.
 * @param {object[]} candidates The case's `test` and `hook` records.
 * @returns {object[] | null} The participant records, or `null` when any is missing or ambiguous.
 */
function resolveParticipants(expectation, candidates) {
  const types = participantTypes(expectation.rule)
  candidates = candidates.filter((record) => types.includes(record.type))
  const byLocation = expectation.locations !== null
  const wanted = byLocation ? expectation.locations : expectation.titles
  const participants = []
  for (const name of new Set(wanted)) {
    // Titles, when the matcher has them too, say which of several tests at one location it means.
    const hits = byLocation
      ? candidates.filter(
          (record) =>
            `${record.path}:${record.line}` === name &&
            (expectation.titles === null || expectation.titles.includes(recordName(record))),
        )
      : candidates.filter((record) => recordName(record) === name)
    if (hits.length !== wanted.filter((entry) => entry === name).length) return null
    participants.push(...hits)
  }
  // Two names answered by one record would shrink the pair to a single test.
  if (new Set(participants).size !== participants.length) return null
  // Locations decided the hits, so the titles have to account for exactly the records found.
  if (byLocation && expectation.titles !== null) {
    const found = participants.map(recordName).sort()
    const named = [...expectation.titles].sort()
    if (found.length !== named.length || found.some((title, index) => title !== named[index])) return null
  }
  return participants
}

/**
 * The record fields a rule's pairing turns on. `duplicate-test-name` groups by full title alone, so
 * the body fingerprints say nothing about why such a pair is silent; `duplicate-each-row` keys rows
 * by full title as well as body; hooks pair by kind first.
 *
 * @param {string} rule The rule the claim forbids.
 * @param {object[]} participants The resolved records.
 * @returns {string[]}
 */
function comparisonFields(rule, participants) {
  if (rule === 'duplicate-test-name') return ['title']
  if (rule === 'duplicate-each-row') return ['title', ...KEY_FIELDS]
  return participants.every((record) => record.type === 'hook') ? ['kind', ...KEY_FIELDS] : KEY_FIELDS
}

/**
 * Whether the pair a silence claim names could have been compared, and where its comparison keys
 * differ. The record stream shows only what the extractor resolved, so `eligible` is necessary, not
 * sufficient: it cannot see a configuration that switches the rule off, whether the rows of an
 * `.each` table belong to one declaration, or which hooks share a chain.
 *
 * - `eligible`: every participant resolved and comparable, and not trivial unless the rule's claim
 *   is about titles alone. `differs` lists the key
 *   fields that disagree; an empty list means the keys are equal yet no finding was emitted, which
 *   some other mechanism explains (the analyzer keeps one representative per equality group, per
 *   file for `cross-file-duplicate` and per declaration for `.each` rows). Such pairs are counted
 *   apart as `eligible ... with equal keys`, not as distinct.
 * - `not-compared`: a participant was set aside as not comparable (or trivial, where that excludes
 *   it), so the silence says nothing about the pair.
 * - `unresolved`: the claim's participants could not be mapped onto test or hook records (a suite
 *   claim names no such record).
 *
 * @param {object} expectation An `expected-absent` label.
 * @param {object[]} testRecords The `test` and `hook` records of the case run.
 * @param {Set<string>} [disabled] Rules the case switches off, which compare nothing.
 * @returns {{eligibility: string, differs: string[]}}
 */
export function classifyEligibility(expectation, testRecords, disabled = new Set()) {
  if (disabled.has(expectation.rule)) return { eligibility: 'not-compared', differs: [] }
  const participants = resolveParticipants(expectation, testRecords)
  // One participant is not a pair, and a pair claim cannot be judged from it.
  if (participants === null || participants.length < 2) return { eligibility: 'unresolved', differs: [] }
  // Every rule but `cross-file-duplicate` groups by registering file, so a pair spanning two files
  // never reaches one comparison. Hook records name no registering file and a helper's hooks can
  // join another file's chain, so such a hook pair cannot be placed either way.
  const files = new Set(participants.map((record) => record.registrationFile ?? record.path))
  if (files.size > 1 && expectation.rule !== 'cross-file-duplicate') {
    return { eligibility: expectation.rule === 'redundant-hook' ? 'unresolved' : 'not-compared', differs: [] }
  }
  const trivialExcludes = !IGNORES_TRIVIALITY.has(expectation.rule)
  if (participants.some((record) => record.comparable === false || (trivialExcludes && record.trivial))) {
    return { eligibility: 'not-compared', differs: [] }
  }
  // A hook is only examined under a test that is itself comparable, and the stream does not say
  // which tests a hook governs (an imported runner's hooks belong to its callers), so any set-aside
  // test in the case leaves the claim unresolved. A definite exclusion above outranks this.
  const unjudgedHooks =
    expectation.rule === 'redundant-hook' &&
    testRecords.some((record) => record.type === 'test' && record.comparable === false)
  if (unjudgedHooks) return { eligibility: 'unresolved', differs: [] }
  // `redundant-hook` only pairs hooks of one kind, so a differing kind is why such a pair is silent.
  const fields = comparisonFields(expectation.rule, participants)
  const differs = fields.filter(
    (field) => new Set(participants.map((record) => record[field])).size > 1,
  )
  return { eligibility: 'eligible', differs }
}

/**
 * Evaluates one expectation against the active findings of its case.
 *
 * @param {object} expectation A label from `buildRecallLabels`.
 * @param {object[]} active The case's active (non-baselined) findings.
 * @param {Set<object>} claimed Findings a known false positive of the case owns.
 * @param {object[]} testRecords The case's `test` and `hook` records.
 * @param {Set<string>} disabled Rules the case switches off.
 * @returns {object} The expectation's id, kind, status and the findings or eligibility behind it.
 */
function evaluateExpectation(expectation, active, claimed, testRecords, disabled) {
  const result = { id: expectation.id, kind: expectation.kind, rule: expectation.rule }
  const matches = active.filter((finding) => findingMatches(finding, expectation))
  switch (expectation.kind) {
    case 'expected-finding':
      return { ...result, matches: matches.length, status: matches.length === 1 ? 'satisfied' : matches.length === 0 ? 'missing' : 'ambiguous' }
    case 'expected-absent': {
      const unclaimed = matches.filter((finding) => !claimed.has(finding))
      return {
        ...result,
        matches: unclaimed.length,
        status: unclaimed.length === 0 ? 'held' : 'violated',
        ...classifyEligibility(expectation, testRecords, disabled),
      }
    }
    case 'known-miss':
      return { ...result, matches: matches.length, status: matches.length === 0 ? 'still-missed' : 'now-reported' }
    case 'known-false-positive': {
      const wanted = expectation.expectCount ?? 1
      const status = matches.length === 0 ? 'no-longer-reported' : matches.length === wanted ? 'still-reported' : 'count-mismatch'
      return { ...result, matches: matches.length, expectCount: wanted, status }
    }
    default:
      throw new Error(`unknown expectation kind "${expectation.kind}" on ${expectation.id}`)
  }
}

/**
 * Evaluates one observed case.
 *
 * @param {object} caseLabel The case's label from `buildRecallLabels`.
 * @param {object[]} expectations The labels of this case's expectations.
 * @param {object} observation The `observeRecallCase` result for the case.
 * @param {object} [options]
 * @param {boolean} [options.requireTestRecords] Whether the run must carry one `test` record per
 *   analyzed test; false for a baseline binary that may predate `--emit-test-fingerprints`.
 * @param {boolean} [options.allowPendingZeroTests] Whether a pending case may analyze zero tests; false
 *   for a baseline, which has no pending claim to wait on and must have measured something.
 * @returns {object} Extraction facts, one outcome per expectation, and the unclaimed findings.
 * @throws {Error} When the case did not produce usable evidence (missing directory, no source, no
 *   summary, malformed records, a non-integer `testsAnalyzed`), because none of those can be
 *   scored as a miss.
 */
export function evaluateCase(
  caseLabel,
  expectations,
  observation,
  { requireTestRecords = true, allowPendingZeroTests = true } = {},
) {
  const { label, directoryPresent, sourceFiles, records, summary } = observation
  if (!directoryPresent) throw new Error(`${label}: case directory is missing`)
  if (sourceFiles.length === 0) throw new Error(`${label}: case directory holds no analyzable source file`)
  if (!summary) throw new Error(`${label}: run produced no summary record`)
  if (!Number.isInteger(summary.testsAnalyzed) || summary.testsAnalyzed < 0) {
    throw new Error(`${label}: the summary has no non-negative integer \`testsAnalyzed\``)
  }
  // A pending case waits on an extraction defect, so zero tests is what it may be holding.
  if (summary.testsAnalyzed === 0 && !(allowPendingZeroTests && caseLabel.pending !== null)) {
    throw new Error(`${label}: the run analyzed zero tests; a case labelled with tests measured nothing`)
  }
  if (typeof summary.incomplete !== 'boolean') {
    throw new Error(`${label}: the summary has no boolean \`incomplete\``)
  }
  if (summary.unresolvedTests !== undefined && !(Number.isInteger(summary.unresolvedTests) && summary.unresolvedTests >= 0)) {
    throw new Error(`${label}: the summary's \`unresolvedTests\` is not a non-negative integer`)
  }
  const bad = recordProblems(records, label)
  if (bad.length > 0) throw new Error(bad.join('\n'))

  const testRecords = records.filter((record) => record.type === 'test')
  const candidates = records.filter((record) => record.type === 'test' || record.type === 'hook')
  const findingRecords = records.filter((record) => record.type === 'finding')
  if (summary.recordCount !== findingRecords.length) {
    throw new Error(`${label}: the summary counts ${summary.recordCount} finding(s) but ${findingRecords.length} \`finding\` record(s) arrived; the stream is partial`)
  }
  if (requireTestRecords && testRecords.length !== summary.testsAnalyzed) {
    throw new Error(`${label}: ${summary.testsAnalyzed} test(s) analyzed but ${testRecords.length} \`test\` record(s) emitted; the stream is partial or the binary lacks --emit-test-fingerprints`)
  }
  const active = records.filter((record) => record.type === 'finding' && !record.baselined)

  // Findings a known false positive owns are classified, not news.
  const claimed = new Set()
  for (const known of expectations.filter((e) => e.kind === 'known-false-positive')) {
    const matches = active.filter((finding) => findingMatches(finding, known))
    if (matches.length === (known.expectCount ?? 1)) matches.forEach((finding) => claimed.add(finding))
  }
  const disabled = disabledRules(caseLabel.arguments ?? [], configuredRules(observation.caseRoot, caseLabel.arguments ?? []))
  const outcomes = expectations.map((e) => evaluateExpectation(e, active, claimed, candidates, disabled))

  // One finding satisfying two expected findings leaves a declared finding unaccounted for while the
  // counts still balance; the recall gate reports it as "N active against M declared".
  const expectedLabels = expectations.filter((e) => e.kind === 'expected-finding')
  for (const finding of active) {
    const sharing = expectedLabels.filter((e) => findingMatches(finding, e))
    // A known false positive that owns the finding is a second claim on it.
    if (sharing.length + (claimed.has(finding) ? 1 : 0) < 2) continue
    for (const outcome of outcomes.filter((o) => sharing.some((e) => e.id === o.id) && o.status === 'satisfied')) {
      outcome.status = 'shared'
    }
  }

  // A finding that a label addresses is already accounted for by that label: reported once, as a known
  // miss no longer holding or as a forbidden finding present, and not again as unlabelled.
  const owners = expectations.filter((e) => ['expected-finding', 'known-miss', 'expected-absent'].includes(e.kind))
  const unclaimed = active
    .filter((finding) => !claimed.has(finding) && !owners.some((e) => findingMatches(finding, e)))
    .map((finding) => ({
      rule: finding.rule,
      severity: finding.severity,
      titles: finding.titles,
      locations: finding.locations.map((loc) => `${loc.path}:${loc.line}`),
    }))

  const extraction = {
    expectedTests: caseLabel.expectedTests,
    tests: summary.testsAnalyzed,
    expectedIncomplete: caseLabel.expectedIncomplete,
    incomplete: summary.incomplete,
    expectedUnresolved: caseLabel.expectedUnresolved,
    unresolved: summary.unresolvedTests ?? null,
  }
  extraction.matches =
    extraction.tests === extraction.expectedTests &&
    extraction.incomplete === extraction.expectedIncomplete &&
    (extraction.expectedUnresolved === null || extraction.unresolved === extraction.expectedUnresolved)

  return {
    name: caseLabel.name,
    rule: caseLabel.rule,
    role: caseLabel.role,
    origin: caseLabel.origin,
    pending: caseLabel.pending,
    extraction,
    testRecords: testRecords.length,
    activeFindings: active.length,
    expectations: outcomes,
    unclaimed,
  }
}

/** The statuses that mean an expectation held, by kind; every other status is a violation. */
const HELD = Object.freeze({
  'expected-finding': 'satisfied',
  'expected-absent': 'held',
  'known-miss': 'still-missed',
  'known-false-positive': 'still-reported',
})

/**
 * Adds `amount` to `counts[key]`, creating it. Keeps the tallies below one-liners.
 *
 * @param {Record<string, number>} counts The tally.
 * @param {string} key The bucket.
 * @param {number} [amount] The increment.
 */
function bump(counts, key, amount = 1) {
  counts[key] = (counts[key] ?? 0) + amount
}

/**
 * Builds the per-rule contract table from the evaluated cases. Pending cases are listed apart and
 * excluded from every denominator, as the recall gate excludes them from its recall figure.
 *
 * @param {object[]} cases Output of {@link evaluateCase}, one per case.
 * @returns {object[]} One row per rule that any case labels or any finding names, sorted by rule.
 */
export function buildRuleTable(cases) {
  const rows = new Map()
  const row = (rule) => {
    if (!rows.has(rule)) {
      rows.set(rule, {
        rule,
        cases: 0,
        expectedFindings: { satisfied: 0, total: 0 },
        expectedAbsent: { held: 0, total: 0, eligible: 0, eligibleEqualKeys: 0, notCompared: 0, unresolved: 0 },
        knownMisses: { stillMissed: 0, total: 0 },
        knownFalsePositives: { stillReported: 0, total: 0 },
        unclaimedEmitted: 0,
        extractionMismatches: 0,
        violations: 0,
      })
    }
    return rows.get(rule)
  }
  for (const evaluated of cases.filter((c) => c.pending === null)) {
    const own = row(evaluated.rule)
    own.cases += 1
    if (!evaluated.extraction.matches) {
      own.extractionMismatches += 1
      own.violations += 1
    }
    for (const outcome of evaluated.expectations) {
      const target = row(outcome.rule)
      const held = outcome.status === HELD[outcome.kind]
      if (!held) target.violations += 1
      if (outcome.kind === 'expected-finding') {
        target.expectedFindings.total += 1
        if (held) target.expectedFindings.satisfied += 1
      } else if (outcome.kind === 'expected-absent') {
        target.expectedAbsent.total += 1
        if (held) {
          target.expectedAbsent.held += 1
          const field = { eligible: 'eligible', 'not-compared': 'notCompared', unresolved: 'unresolved' }[outcome.eligibility]
          target.expectedAbsent[field] += 1
          if (outcome.eligibility === 'eligible' && outcome.differs.length === 0) target.expectedAbsent.eligibleEqualKeys += 1
        }
      } else if (outcome.kind === 'known-miss') {
        target.knownMisses.total += 1
        if (held) target.knownMisses.stillMissed += 1
      } else {
        target.knownFalsePositives.total += 1
        if (held) target.knownFalsePositives.stillReported += 1
      }
    }
    for (const finding of evaluated.unclaimed) {
      const target = row(finding.rule)
      target.unclaimedEmitted += 1
      target.violations += 1
    }
  }
  return [...rows.values()].sort((a, b) => a.rule.localeCompare(b.rule))
}

/**
 * Counts cases and expectations along the axes the labels vary on.
 *
 * @param {{cases: object[], expectations: object[]}} labels Output of `buildRecallLabels`.
 * @returns {Record<string, Record<string, number>>} `byRule`, `byRole`, `byOrigin`, `byVerdict`,
 *   `byKind` (expectations) and `pending`.
 */
export function buildDistribution(labels) {
  const out = { byRule: {}, byRole: {}, byOrigin: {}, byVerdict: {}, byKind: {}, pending: {} }
  for (const c of labels.cases) {
    bump(out.byRule, c.rule)
    bump(out.byRole, c.role)
    bump(out.byOrigin, c.origin)
    bump(out.byVerdict, c.verdict)
    bump(out.pending, c.pending === null ? 'active' : 'pending')
  }
  for (const e of labels.expectations) bump(out.byKind, e.kind)
  return out
}

/**
 * The reasons a run did not measure what it claims to. Empty means the evaluation covered the
 * whole labelled corpus; anything else must make the command fail, never print a smaller table.
 *
 * @param {{cases: object[], expectations: object[]}} labels The labels the run was meant to cover.
 * @param {object[]} cases The evaluated cases.
 * @returns {string[]}
 */
export function measurementProblems(labels, cases) {
  const problems = []
  if (labels.cases.length === 0 || labels.expectations.length === 0) {
    problems.push('the label set is empty; nothing was measured')
  }
  for (const [baseline, kind, name] of [
    [labels.knownMissBaseline, 'known-miss', 'known misses'],
    [labels.knownFalsePositiveBaseline, 'known-false-positive', 'known false positives'],
  ]) {
    const actual = labels.expectations.filter((e) => e.kind === kind).length
    if (Number.isInteger(baseline) && actual !== baseline) {
      problems.push(`${actual} ${name} labelled against a declared baseline of ${baseline}`)
    }
  }
  if (Number.isInteger(labels.minimumCases) && labels.cases.length < labels.minimumCases) {
    problems.push(`${labels.cases.length} labelled case(s) against a declared floor of ${labels.minimumCases}`)
  }
  if (cases.length !== labels.cases.length) {
    problems.push(`${cases.length} of ${labels.cases.length} labelled case(s) were evaluated`)
  }
  const evaluated = cases.reduce((sum, c) => sum + c.expectations.length, 0)
  if (evaluated !== labels.expectations.length) {
    problems.push(`${evaluated} of ${labels.expectations.length} labelled expectation(s) were evaluated`)
  }
  if (cases.reduce((sum, c) => sum + c.extraction.tests, 0) === 0) {
    problems.push('the corpus extracted zero tests; nothing was measured')
  }
  if (cases.reduce((sum, c) => sum + c.testRecords, 0) === 0) {
    problems.push('no `test` record was emitted for any case; eligibility cannot be judged')
  }
  return problems
}

/**
 * Assembles the report.
 *
 * @param {object} input
 * @param {string} input.binary The binary evaluated.
 * @param {string} input.root The corpus root.
 * @param {{cases: object[], expectations: object[]}} input.labels The corpus labels.
 * @param {object[]} input.cases The evaluated cases.
 * @returns {object} The JSON report.
 * @throws {Error} When {@link measurementProblems} is not empty.
 */
export function buildReport({ binary, root, labels, cases }) {
  const problems = measurementProblems(labels, cases)
  if (problems.length > 0) throw new Error(`the evaluation did not measure the corpus:\n  ${problems.join('\n  ')}`)
  const rules = buildRuleTable(cases)
  const live = cases.filter((c) => c.pending === null)
  const violations = rules.reduce((sum, r) => sum + r.violations, 0)
  return {
    schemaVersion: REPORT_SCHEMA_VERSION,
    binary,
    root,
    work: {
      cases: cases.length,
      pendingCases: cases.length - live.length,
      expectations: labels.expectations.length,
      testsExtracted: cases.reduce((sum, c) => sum + c.extraction.tests, 0),
      testRecords: cases.reduce((sum, c) => sum + c.testRecords, 0),
      activeFindings: cases.reduce((sum, c) => sum + c.activeFindings, 0),
    },
    violations,
    rules,
    distribution: buildDistribution(labels),
    pending: cases
      .filter((c) => c.pending !== null)
      .map((c) => ({
        name: c.name,
        reason: c.pending,
        held: c.expectations.filter((o) => o.status !== HELD[o.kind]).length + c.unclaimed.length + (c.extraction.matches ? 0 : 1),
      })),
    cases,
  }
}

/** Formats a satisfied/total pair as `n/d (p%)`, or `n/a` with no population. */
function cell(satisfied, total) {
  const value = ratio(satisfied, total)
  return value === null ? 'n/a' : `${satisfied}/${total} (${(value * 100).toFixed(1)}%)`
}

/**
 * Renders the report for a terminal.
 *
 * @param {object} report Output of {@link buildReport}.
 * @returns {string[]} The lines to print.
 */
export function renderText(report) {
  const lines = [
    `Corpus evaluation (${report.work.cases} case(s), ${report.work.pendingCases} pending, ` +
      `${report.work.expectations} expectation(s), ${report.work.testsExtracted} test(s) extracted)`,
    `binary: ${report.binary}`,
    `contract violations: ${report.violations}`,
    '',
    'per rule (pending cases excluded)',
  ]
  for (const r of report.rules) {
    lines.push(
      `  ${r.rule}: ${r.cases} case(s)`,
      `    expected findings   ${cell(r.expectedFindings.satisfied, r.expectedFindings.total)}`,
      `    forbidden held      ${cell(r.expectedAbsent.held, r.expectedAbsent.total)}` +
        `  [eligible ${r.expectedAbsent.eligible} (${r.expectedAbsent.eligibleEqualKeys} with equal keys), not compared ${r.expectedAbsent.notCompared}, unresolved ${r.expectedAbsent.unresolved}]`,
      `    known misses kept   ${cell(r.knownMisses.stillMissed, r.knownMisses.total)}`,
      `    known FPs kept      ${cell(r.knownFalsePositives.stillReported, r.knownFalsePositives.total)}`,
      `    unclaimed emitted   ${r.unclaimedEmitted}`,
      `    extraction mismatches ${r.extractionMismatches}`,
    )
  }
  const flagged = report.cases.filter(
    (c) => c.pending === null && (!c.extraction.matches || c.unclaimed.length > 0 || c.expectations.some((o) => o.status !== HELD[o.kind])),
  )
  if (flagged.length > 0) {
    lines.push('', 'cases with violations')
    for (const c of flagged) {
      const bad = c.expectations.filter((o) => o.status !== HELD[o.kind]).map((o) => `${o.id} ${o.status}`)
      if (!c.extraction.matches) bad.push('extraction differs from the label')
      for (const finding of c.unclaimed) bad.push(`unclaimed ${finding.rule} @ ${finding.locations.join(' + ')}`)
      lines.push(`  ${c.name}: ${bad.join('; ')}`)
    }
  }
  for (const p of report.pending) lines.push(`pending: ${p.name} (${p.held} assertion(s) still failing) -- ${p.reason}`)
  lines.push('', 'distribution')
  for (const [axis, counts] of Object.entries(report.distribution)) {
    lines.push(`  ${axis}: ${Object.entries(counts).sort(([a], [b]) => a.localeCompare(b)).map(([k, v]) => `${k} ${v}`).join(', ')}`)
  }
  return lines
}
