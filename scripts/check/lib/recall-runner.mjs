// Recall-case observation and finding matchers. Observing a case (checking its directory, running
// the binary, reading the jsonl records) touches the filesystem and judges nothing; the matchers
// address one finding by rule, titles and locations for the gates and the corpus evaluator.

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
 * @param {string[]} [options.extraArguments] CLI tokens appended after the case's own, for a
 *   consumer that needs more records (the evaluator's `--emit-test-fingerprints`).
 * @param {number} [options.timeoutMs] Stops the run after this long (see `runAnalyzerJsonl`).
 * @returns {{label: string, caseRoot: string, directoryPresent: boolean, sourceFiles: string[],
 *   records: object[] | null, summary: object | null}} The case's run label and resolved root;
 *   whether its directory exists; the analyzable source files found under it; the parsed jsonl
 *   records and the summary record (all `null` when the case never ran -- the run is skipped for
 *   a missing or sourceless directory exactly as the gate's inline loop skipped it, so the judge
 *   sees the same facts it always did); `summary` is `null` when the run wrote no summary record.
 */
export function observeRecallCase({ binary, root, testCase, extraArguments = [], timeoutMs }) {
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
  const records = runAnalyzerJsonl(
    binary,
    [caseRoot, ...(testCase.arguments ?? []), ...extraArguments],
    {},
    { timeoutMs },
  ).records
  return {
    label,
    caseRoot,
    directoryPresent,
    sourceFiles,
    records,
    summary: records.find((record) => record.type === 'summary') ?? null,
  }
}
