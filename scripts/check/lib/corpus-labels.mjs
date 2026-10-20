// The label adapter over the recall manifest: one flat list of cases and one of expectations, each
// with a stable id, so a consumer (the corpus evaluator) reads labels without walking the manifest's
// shape or re-deriving what `check-recall.mjs` already decided.
//
// Pure except `readRecallManifest`. `buildRecallLabels` never touches the filesystem and never runs
// the analyzer; it describes what the manifest claims, not whether the binary agrees.
//
// An expectation id is `<case name>#<n>`, `n` counting from 1 across the case's `expectedFindings`,
// `expectedAbsent`, `knownMisses` and `knownFalsePositives` in that order. The index is positional:
// reordering a case's lists, or moving an entry between them, renames ids. Consumers that keep ids
// across runs must keep them across those edits too.
//
// The adapter throws on anything it cannot label rather than skipping it: a skipped entry shrinks
// the population a consumer divides by, and a smaller denominator reads as a better score. It
// reuses `validateCaseLabels` for the label fields and checks only the structure it needs to
// produce a label; the recall gate stays the authority on everything the run can contradict.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { ALL_RULES } from './fixture-directives.mjs'
import { validateCaseLabels, validateRootKeys } from './manifest-schema.mjs'
import { describeMatcher } from './recall-runner.mjs'

/**
 * Matcher groups in id order: manifest field, label kind, the outcome the label claims, whether the
 * field is mandatory on a case, and whether its matchers must (`true`) or must not (`false`) carry a
 * severity.
 */
export const EXPECTATION_GROUPS = Object.freeze([
  { field: 'expectedFindings', kind: 'expected-finding', outcome: 'finding', required: true, severity: true },
  { field: 'expectedAbsent', kind: 'expected-absent', outcome: 'silence', required: true, severity: false },
  { field: 'knownMisses', kind: 'known-miss', outcome: 'finding', required: false, severity: true, known: true },
  { field: 'knownFalsePositives', kind: 'known-false-positive', outcome: 'silence', required: false, severity: true, known: true },
])

/** What a matcher addresses, ignoring severity: the identity two matchers must not share. */
function matcherKey(matcher) {
  const sorted = (values) => (Array.isArray(values) ? [...values].sort() : null)
  return JSON.stringify([matcher.rule, sorted(matcher.titles), sorted(matcher.locations)])
}

/** True for a string with at least one character. */
function isNonEmptyString(value) {
  return typeof value === 'string' && value !== ''
}

/** True for a non-empty array whose members are all non-empty strings. */
function isStringList(value) {
  return Array.isArray(value) && value.length > 0 && value.every(isNonEmptyString)
}

/** Structural problems that stop a case from being labelled. */
function caseShapeProblems(testCase, where) {
  const problems = []
  for (const field of ['name', 'path', 'rule', 'role']) {
    if (!isNonEmptyString(testCase[field])) problems.push(`${where}: \`${field}\` must be a non-empty string`)
  }
  if (!ALL_RULES.has(testCase.rule)) problems.push(`${where}: unknown rule "${testCase.rule}"`)
  if (!['positive', 'negative', 'variant'].includes(testCase.role)) {
    problems.push(`${where}: role must be positive, negative, or variant`)
  }
  if (!Number.isInteger(testCase.expectedTests) || testCase.expectedTests < 1) {
    problems.push(`${where}: \`expectedTests\` must be a positive integer`)
  }
  if (typeof testCase.expectedIncomplete !== 'boolean') {
    problems.push(`${where}: \`expectedIncomplete\` must be a boolean`)
  }
  if (
    testCase.expectedUnresolved !== undefined &&
    (!Number.isInteger(testCase.expectedUnresolved) || testCase.expectedUnresolved < 0)
  ) {
    problems.push(`${where}: \`expectedUnresolved\` must be a non-negative integer`)
  }
  if (testCase.pending !== undefined && !isNonEmptyString(testCase.pending)) {
    problems.push(`${where}: \`pending\` must be a non-empty reason string`)
  }
  if (testCase.arguments !== undefined && !(Array.isArray(testCase.arguments) && testCase.arguments.every(isNonEmptyString))) {
    problems.push(`${where}: \`arguments\` must be an array of non-empty CLI tokens`)
  }
  return problems
}

/**
 * Problems with a `knownMisses` or `knownFalsePositives` entry, mirroring the recall gate: an id
 * (one namespace across both lists), a mechanism-naming reason of 40+ characters, and a positive
 * integer `expectCount` when present. Records the id in `knownIds`.
 */
function knownEntryProblems(entry, label, knownIds) {
  const problems = []
  if (!isNonEmptyString(entry.id)) {
    problems.push(`${label}: every known entry needs an \`id\``)
  } else {
    if (knownIds.has(entry.id)) problems.push(`${label}: duplicate known id ${entry.id}`)
    knownIds.add(entry.id)
  }
  if (typeof entry.reason !== 'string' || entry.reason.length < 40) {
    problems.push(`${label}: a known entry needs a \`reason\` of 40+ characters naming the mechanism`)
  }
  if (entry.expectCount !== undefined && (!Number.isInteger(entry.expectCount) || entry.expectCount < 1)) {
    problems.push(`${label}: \`expectCount\` must be a positive integer`)
  }
  return problems
}

/** Structural problems in one matcher entry. */
function matcherProblems(entry, label) {
  if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
    return [`${label}: a matcher must be an object`]
  }
  const problems = []
  if (!ALL_RULES.has(entry.rule)) problems.push(`${label}: unknown rule "${entry.rule}"`)
  if (entry.titles === undefined && entry.locations === undefined) {
    problems.push(`${label}: a matcher needs \`titles\` or \`locations\``)
  }
  for (const field of ['titles', 'locations']) {
    if (entry[field] !== undefined && !isStringList(entry[field])) {
      problems.push(`${label}: \`${field}\` must be a non-empty array of non-empty strings`)
    }
  }
  return problems
}

/**
 * Problems with one `mixedRuleCoverage` entry, mirroring the recall gate: a pair of two distinct
 * rules the analyzer implements, recorded once whichever way round it is written, and exactly one
 * of `case` (a declared case in which BOTH rules are expected to fire) or `cannotCoOccur` (a reason
 * of 20+ characters). Records the pair in `seenPairs`.
 */
function mixedControlProblems(entry, where, casesByName, seenPairs) {
  if (!Array.isArray(entry?.pair) || entry.pair.length !== 2) {
    return [`${where}: \`pair\` must name exactly two rules`]
  }
  const [first, second] = entry.pair
  if (!ALL_RULES.has(first) || !ALL_RULES.has(second) || first === second) {
    return [`${where}: both members must be distinct rules the analyzer implements`]
  }
  const key = [first, second].sort().join('\n')
  if (seenPairs.has(key)) return [`${where}: this pair is recorded more than once`]
  seenPairs.add(key)
  // Presence decides which field the entry claims; only then is its value judged, so an entry
  // that also carries a malformed second field is refused instead of returned with both.
  const declaresCase = entry.case !== undefined
  if (declaresCase === (entry.cannotCoOccur !== undefined)) {
    return [
      `${where}: an entry names exactly one of \`case\` (the case covering the pair) or ` +
        '`cannotCoOccur` (a reason of 20+ characters naming the structural impossibility)',
    ]
  }
  if (!declaresCase) {
    return typeof entry.cannotCoOccur === 'string' && entry.cannotCoOccur.length >= 20
      ? []
      : [`${where}: \`cannotCoOccur\` must be a reason of 20+ characters`]
  }
  if (!isNonEmptyString(entry.case)) return [`${where}: \`case\` must name a declared case`]
  const covering = casesByName.get(entry.case)
  if (!covering) return [`${where}: names case "${entry.case}", which the manifest does not declare`]
  const findings = Array.isArray(covering.expectedFindings) ? covering.expectedFindings : []
  const firing = new Set(findings.map((finding) => finding?.rule))
  return [first, second]
    .filter((rule) => !firing.has(rule))
    .map(
      (rule) =>
        `${where}: names case "${entry.case}", whose \`expectedFindings\` include no \`${rule}\` ` +
        'finding. Coverage means both rules fire in the case',
    )
}

/**
 * Builds the case and expectation labels from a parsed recall manifest.
 *
 * @param {object} manifest The parsed `manifest.json`.
 * @returns {{cases: object[], expectations: object[], controls: object[]}} `cases` holds one label
 *   per case (identity, role, origin, verdict, classifiedAt, `pending` reason or `null`, extraction
 *   and completeness expectations, CLI arguments); `expectations` one per matcher (`id`, `caseName`,
 *   `index`, `kind`, `outcome`, `rule`, `severity`, `titles`, `locations`, the case's labels,
 *   `pending`, and for known entries `knownId`, `reason`, `expectCount`); `controls` one per
 *   `mixedRuleCoverage` entry, its case reference resolved.
 * @throws {Error} Listing every problem, when the manifest has no cases, a case or matcher is
 *   malformed, two cases share a name, a matcher is repeated, a finding is both expected and
 *   expected absent, or a coverage entry names a case the manifest does not declare.
 */
export function buildRecallLabels(manifest) {
  const problems = []
  if (!Array.isArray(manifest?.cases) || manifest.cases.length === 0) {
    throw new Error('recall manifest has no cases; there is nothing to label')
  }

  const cases = []
  const expectations = []
  const names = new Set()
  const knownIds = new Set()
  problems.push(...validateRootKeys(manifest).map((line) => line.replace(/^FAIL /, '')))
  manifest.cases.forEach((testCase, caseIndex) => {
    const labelProblems = validateCaseLabels(testCase, caseIndex)
    if (testCase === null || typeof testCase !== 'object' || Array.isArray(testCase)) {
      problems.push(...labelProblems)
      return
    }
    const where = `case ${isNonEmptyString(testCase.name) ? testCase.name : `#${caseIndex}`}`
    problems.push(...labelProblems.map((line) => line.replace(/^FAIL /, '')))
    const shape = caseShapeProblems(testCase, where)
    problems.push(...shape)
    if (isNonEmptyString(testCase.name)) {
      if (names.has(testCase.name)) problems.push(`${where}: duplicate case name`)
      names.add(testCase.name)
    }

    const caseLabel = {
      name: testCase.name,
      path: testCase.path,
      rule: testCase.rule,
      role: testCase.role,
      origin: testCase.origin,
      verdict: testCase.verdict,
      classifiedAt: testCase.classifiedAt,
      pending: testCase.pending ?? null,
      expectedTests: testCase.expectedTests,
      expectedIncomplete: testCase.expectedIncomplete,
      expectedUnresolved: testCase.expectedUnresolved ?? null,
      arguments: testCase.arguments ?? [],
    }
    cases.push(caseLabel)

    let index = 0
    const keysByKind = new Map()
    for (const group of EXPECTATION_GROUPS) {
      const entries = testCase[group.field] ?? (group.required ? undefined : [])
      if (!Array.isArray(entries)) {
        problems.push(`${where}: \`${group.field}\` must be an array`)
        continue
      }
      const keys = new Map()
      keysByKind.set(group.kind, keys)
      for (const entry of entries) {
        index += 1
        const id = `${testCase.name}#${index}`
        const entryProblems = matcherProblems(entry, `${id} ${group.field}`)
        problems.push(...entryProblems)
        if (entryProblems.length > 0) continue
        if (group.severity && !isNonEmptyString(entry.severity)) {
          problems.push(`${id} ${group.field}: an asserted finding must declare its \`severity\``)
        }
        if (!group.severity && entry.severity) {
          problems.push(`${id} ${group.field}: \`expectedAbsent\` must not declare a severity`)
        }
        if (group.known) problems.push(...knownEntryProblems(entry, `${id} ${group.field}`, knownIds))
        const key = matcherKey(entry)
        if (keys.has(key)) {
          problems.push(`${id} ${group.field}: repeats ${keys.get(key)} (${describeMatcher(entry)})`)
        } else {
          keys.set(key, id)
        }
        expectations.push({
          id,
          caseName: testCase.name,
          index,
          kind: group.kind,
          outcome: group.outcome,
          rule: entry.rule,
          severity: entry.severity ?? null,
          titles: entry.titles ?? null,
          locations: entry.locations ?? null,
          role: caseLabel.role,
          origin: caseLabel.origin,
          verdict: caseLabel.verdict,
          classifiedAt: caseLabel.classifiedAt,
          pending: caseLabel.pending,
          knownId: entry.id ?? null,
          reason: entry.reason ?? null,
          expectCount: entry.expectCount ?? null,
        })
      }
    }
    // A finding both expected and expected absent is a contradiction no run can satisfy.
    const expected = keysByKind.get('expected-finding')
    for (const [key, id] of keysByKind.get('expected-absent') ?? []) {
      if (expected?.has(key)) problems.push(`${id}: also expected as a finding by ${expected.get(key)}`)
    }
  })

  const controls = []
  const byName = new Map(
    manifest.cases.filter((c) => c !== null && typeof c === 'object').map((c) => [c.name, c]),
  )
  const seenPairs = new Set()
  const coverage = manifest.mixedRuleCoverage === undefined ? [] : manifest.mixedRuleCoverage
  if (!Array.isArray(coverage)) {
    problems.push('mixedRuleCoverage: must be an array with one entry per unordered pair of rules')
  }
  for (const entry of Array.isArray(coverage) ? coverage : []) {
    const where = `mixedRuleCoverage ${JSON.stringify(entry?.pair ?? entry)}`
    const controlProblems = mixedControlProblems(entry, where, byName, seenPairs)
    problems.push(...controlProblems)
    if (controlProblems.length > 0) continue
    controls.push({
      pair: [...entry.pair],
      caseName: typeof entry.case === 'string' ? entry.case : null,
      cannotCoOccur: typeof entry.cannotCoOccur === 'string' ? entry.cannotCoOccur : null,
    })
  }

  if (problems.length > 0) {
    throw new Error(`${problems.length} recall label problem(s):\n${problems.map((line) => `  ${line}`).join('\n')}`)
  }
  return { cases, expectations, controls }
}

/**
 * Counts what a label set holds, for a consumer's work-performed floor.
 *
 * @param {{cases: object[], expectations: object[]}} labels Output of {@link buildRecallLabels}.
 * @returns {{cases: number, expectations: number, pendingCases: number, byKind: Record<string, number>}}
 */
export function summarizeLabels(labels) {
  const byKind = Object.fromEntries(EXPECTATION_GROUPS.map((group) => [group.kind, 0]))
  for (const expectation of labels.expectations) byKind[expectation.kind] += 1
  return {
    cases: labels.cases.length,
    expectations: labels.expectations.length,
    pendingCases: labels.cases.filter((label) => label.pending !== null).length,
    byKind,
  }
}

/**
 * Reads and labels `<root>/manifest.json`; the only function here that touches the filesystem.
 *
 * @param {string} root The recall corpus root.
 * @returns {ReturnType<typeof buildRecallLabels>}
 */
export function readRecallManifest(root) {
  const manifest = JSON.parse(readFileSync(join(root, 'manifest.json'), 'utf8'))
  if (manifest.schemaVersion !== 1) throw new Error('unsupported recall manifest schemaVersion')
  return buildRecallLabels(manifest)
}
