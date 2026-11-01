// The case-level label schema of `fixtures/recall/manifest.json`.
//
// `check-recall.mjs` validates what the run can contradict: counts, expectations, severities and
// the rule inventory. This module validates the labels it cannot -- where a case came from, whether
// its expectations were read against the source, when it was classified -- and rejects any key of a
// case, a matcher or the manifest root that no schema entry names, so a misspelled field fails
// instead of being ignored. These labels
// are format-checked only: nothing here (or in the run) can confirm that `origin` is true or that
// a `reviewed` verdict had a reviewer.
//
// A field joins this schema together with a case that uses it. `axes` and `variantOf` are
// deliberately absent: no case carries either, and a field nobody exercises cannot be shown to
// reject its own malformed values.

export const ORIGINS = Object.freeze(['synthetic', 'real', 'regression'])
export const VERDICTS = Object.freeze(['reviewed', 'generated'])

/** The label fields every case must carry. */
export const LABEL_FIELDS = Object.freeze(['origin', 'verdict', 'classifiedAt'])

/** Every key a case may carry: the gate's own fields plus the label fields. */
export const CASE_KEYS = new Set([
  'name',
  'path',
  'rule',
  'role',
  'arguments',
  'expectedTests',
  'expectedIncomplete',
  'expectedUnresolved',
  'blindUnderCurrent',
  'expectedFindings',
  'expectedAbsent',
  'knownMisses',
  'knownFalsePositives',
  'pending',
  ...LABEL_FIELDS,
])

/** Every key a matcher entry (`expectedFindings`, `expectedAbsent`, `knownMisses`, ...) may carry. */
export const MATCHER_KEYS = new Set(['rule', 'severity', 'titles', 'locations', 'id', 'reason', 'expectCount'])

/** Every key the manifest root may carry. */
export const ROOT_KEYS = new Set([
  'schemaVersion',
  'minimumCases',
  'knownMissBaseline',
  'knownFalsePositiveBaseline',
  'minimumRecall',
  'recallDestination',
  'cases',
  'mixedRuleCoverage',
])

const MATCHER_FIELDS = ['expectedFindings', 'expectedAbsent', 'knownMisses', 'knownFalsePositives']

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/

/** True for a real calendar date written `YYYY-MM-DD` (rejects `2026-02-30`, which `Date` rolls over). */
export function isCalendarDate(value) {
  if (typeof value !== 'string' || !DATE_RE.test(value)) return false
  const parsed = new Date(`${value}T00:00:00Z`)
  return !Number.isNaN(parsed.getTime()) && parsed.toISOString().slice(0, 10) === value
}

/**
 * Format-checks one case's label fields and rejects unknown keys.
 *
 * @param {unknown} testCase A manifest case.
 * @param {number} index Its position, used when the case has no usable name.
 * @returns {string[]} One `FAIL case ...` line per violation; empty when the labels are valid.
 */
export function validateCaseLabels(testCase, index) {
  if (testCase === null || typeof testCase !== 'object' || Array.isArray(testCase)) {
    return [`FAIL case #${index}: a case must be an object`]
  }
  const where = `case ${typeof testCase.name === 'string' && testCase.name !== '' ? testCase.name : `#${index}`}`
  const failures = []
  for (const key of Object.keys(testCase)) {
    if (!CASE_KEYS.has(key)) {
      failures.push(
        `FAIL ${where}: unknown key \`${key}\`. A field no schema entry names is ignored by every ` +
          `gate, so a misspelled one silently drops its meaning`,
      )
    }
  }
  for (const field of MATCHER_FIELDS) {
    if (!Array.isArray(testCase[field])) continue
    testCase[field].forEach((entry, position) => {
      if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) return
      for (const key of Object.keys(entry)) {
        if (!MATCHER_KEYS.has(key)) {
          failures.push(
            `FAIL ${where} ${field}[${position}]: unknown key \`${key}\`. A misspelled matcher ` +
              `field is ignored and the matcher asserts less than it appears to`,
          )
        }
      }
    })
  }
  if (!ORIGINS.includes(testCase.origin)) {
    failures.push(`FAIL ${where}: \`origin\` must be one of ${ORIGINS.join(', ')}`)
  }
  if (!VERDICTS.includes(testCase.verdict)) {
    failures.push(`FAIL ${where}: \`verdict\` must be one of ${VERDICTS.join(', ')}`)
  }
  if (!isCalendarDate(testCase.classifiedAt)) {
    failures.push(`FAIL ${where}: \`classifiedAt\` must be a calendar date written YYYY-MM-DD`)
  }
  return failures
}

/**
 * Format-checks the labels of every case in a manifest. A manifest whose `cases` is not an array
 * yields no failures here: `check-recall.mjs` already reports that, and this module adds nothing
 * to a manifest it cannot read. Zero cases is likewise the gate's `minimumCases` floor to fail;
 * `buildRecallLabels` refuses an empty manifest on its own account.
 *
 * @param {{cases?: unknown}} manifest The parsed manifest.
 * @returns {string[]} Failure lines, empty when every case's labels are valid.
 */
export function validateManifestLabels(manifest) {
  if (!Array.isArray(manifest?.cases)) return []
  return [
    ...validateRootKeys(manifest),
    ...manifest.cases.flatMap((testCase, index) => validateCaseLabels(testCase, index)),
  ]
}

/**
 * Rejects manifest root keys no schema entry names.
 *
 * @param {object} manifest The parsed manifest.
 * @returns {string[]} One `FAIL manifest: ...` line per unknown key.
 */
export function validateRootKeys(manifest) {
  return Object.keys(manifest)
    .filter((key) => !ROOT_KEYS.has(key))
    .map(
      (key) =>
        `FAIL manifest: unknown key \`${key}\`. A misspelled floor or baseline is ignored, and the ` +
        `gate then compares against nothing`,
    )
}
