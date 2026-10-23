// What a run leaves out of comparison, as numbers a baseline can pin.
//
// Findings alone cannot show a change that narrows what the analyzer looks at: a pattern the
// analyzer stops reading removes tests from the comparison, and the finding count moves little or
// not at all. The summary's `exclusions` states what was left out and why; this module reads it,
// checks it is internally consistent, and compares it with a recorded baseline in both
// directions, because a rise is a regression and a fall is a gain that must be recorded or the
// ceiling becomes slack that a later regression hides inside.

/**
 * @typedef {object} ExcludedFigures
 * @property {number} tests Tests left out of comparison.
 * @property {number} files Files left out of comparison in whole or in part.
 * @property {Record<string, {files: number, tests: number}>} reasons The same, by primary reason.
 * @property {number} testsAnalyzed Tests the run extracted.
 * @property {number} filesAnalyzed Files the run read.
 */

/**
 * @param {unknown} value A candidate count.
 * @returns {boolean} Whether it is a non-negative integer.
 */
const count = (value) => Number.isInteger(value) && value >= 0

/**
 * Reads the exclusion figures of an analysis summary.
 *
 * Refuses a summary that does not state them or states them inconsistently, so a gate cannot pass
 * on a binary that stopped reporting the figures, and a run that analyzed nothing is not a run
 * with nothing excluded.
 *
 * @param {object} summary The run's summary record.
 * @returns {ExcludedFigures}
 * @throws {Error} When the figures are absent, malformed, or inconsistent with the run.
 */
export function excludedFigures(summary) {
  const exclusions = summary?.exclusions
  if (exclusions === null || typeof exclusions !== 'object') {
    throw new Error('the summary has no `exclusions`; this binary does not report what it leaves out of comparison')
  }
  const { tests, files, reasons } = exclusions
  if (!count(tests) || !count(files) || !Array.isArray(reasons)) throw new Error('the summary\'s `exclusions` is malformed')
  if (!count(summary.testsAnalyzed) || !count(summary.filesAnalyzed)) throw new Error('the summary does not state how many tests and files were analyzed')
  if (summary.testsAnalyzed === 0 || summary.filesAnalyzed === 0) {
    throw new Error('the run analyzed no tests or no files; an exclusion figure over nothing is not a result')
  }
  if (tests > summary.testsAnalyzed) throw new Error(`${tests} test(s) excluded out of ${summary.testsAnalyzed} analyzed`)
  const byReason = {}
  for (const entry of reasons) {
    if (typeof entry?.reason !== 'string' || !count(entry.files) || !count(entry.tests) || entry.reason in byReason) {
      throw new Error('an exclusion reason is malformed or repeated')
    }
    byReason[entry.reason] = { files: entry.files, tests: entry.tests }
  }
  // Every excluded file has one primary reason and the tally covers all of them (`omitted` counts
  // file entries left out of the detail list, not reasons), so the reasons add up to the totals.
  const sum = (key) => Object.values(byReason).reduce((total, entry) => total + entry[key], 0)
  if (sum('tests') !== tests || sum('files') !== files) throw new Error('the exclusion reasons do not add up to the totals')
  return { tests, files, reasons: byReason, testsAnalyzed: summary.testsAnalyzed, filesAnalyzed: summary.filesAnalyzed }
}

/**
 * The share of analyzed tests that were left out, as a percentage with one decimal.
 *
 * @param {ExcludedFigures} figures
 * @returns {string}
 */
export function shareText(figures) {
  return `${((100 * figures.tests) / figures.testsAnalyzed).toFixed(1)}%`
}

/**
 * The part of the figures a baseline records.
 *
 * @param {ExcludedFigures} figures
 * @returns {{tests: number, files: number, reasons: Record<string, {files: number, tests: number}>}}
 */
export function recordable(figures) {
  return { tests: figures.tests, files: figures.files, reasons: figures.reasons }
}

/**
 * Whether a comparison is a change either way: a rise fails as a regression, a fall fails until it
 * is recorded, so the ceiling cannot go slack.
 *
 * @param {{rose: string[], fell: string[]}} change The result of {@link compareExcluded}.
 * @returns {boolean}
 */
export function exclusionsDrifted(change) {
  return change.rose.length > 0 || change.fell.length > 0
}

/**
 * Compares a run's exclusions with the recorded ones.
 *
 * @param {{tests: number, files: number, reasons: Record<string, {files: number, tests: number}>} | undefined} base
 *   The recorded figures, or nothing when none were recorded yet.
 * @param {ExcludedFigures} now The run's figures.
 * @returns {{rose: string[], fell: string[], unrecorded: boolean}} What got worse and what got better, in words.
 * @throws {Error} When the recorded figures are present but malformed.
 */
export function compareExcluded(base, now) {
  if (base === undefined || base === null) return { rose: [], fell: [], unrecorded: true }
  // A hand-edited or damaged baseline must not read as "nothing changed": every comparison below
  // is false for a figure that is not a number.
  if (!count(base.tests) || !count(base.files) || base.reasons === null || typeof base.reasons !== 'object' || Array.isArray(base.reasons)) {
    throw new Error("the recorded exclusion baseline is malformed; delete this project's `excluded` entry and re-run with --update after reviewing the figures")
  }
  for (const [reason, entry] of Object.entries(base.reasons)) {
    if (!count(entry?.files) || !count(entry?.tests)) throw new Error(`the recorded exclusion baseline for \`${reason}\` is malformed`)
  }
  const recorded = Object.values(base.reasons)
  if (recorded.reduce((total, entry) => total + entry.tests, 0) !== base.tests || recorded.reduce((total, entry) => total + entry.files, 0) !== base.files) {
    throw new Error('the recorded exclusion baseline does not add up by reason')
  }
  const rose = []
  const fell = []
  const note = (label, was, is) => {
    if (is > was) rose.push(`${label} ${was} -> ${is}`)
    else if (is < was) fell.push(`${label} ${was} -> ${is}`)
  }
  note('tests excluded', base.tests, now.tests)
  note('files excluded', base.files, now.files)
  const reasons = new Set([...Object.keys(base.reasons), ...Object.keys(now.reasons)])
  for (const reason of [...reasons].sort()) {
    const was = base.reasons[reason] ?? { files: 0, tests: 0 }
    const is = now.reasons[reason] ?? { files: 0, tests: 0 }
    note(`${reason} tests`, was.tests, is.tests)
    note(`${reason} files`, was.files, is.files)
  }
  return { rose, fell, unrecorded: false }
}
