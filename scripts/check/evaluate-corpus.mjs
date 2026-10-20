#!/usr/bin/env node
// Reports what one analyzer binary does against the labelled recall corpus.
//
// Usage: node scripts/check/evaluate-corpus.mjs [binary] [--root DIR] [--json FILE]
//                                               [--fail-on-violation]
//                                               [--baseline-binary FILE [--baseline-ref REF]
//                                                [--fail-on-regression]]
//
// Unlike `check-recall.mjs`, which stops at the first broken contract, this runs every case, never
// stops on a violation and reports each one with the denominator behind it: expected findings
// satisfied, forbidden findings held, findings nobody labelled, and for each silence the corpus
// claims whether the pair was eligible for comparison. It is a measurement, so it stays out of
// `npm run check`; a violation is data and exits 0 unless `--fail-on-violation` is given. A run
// that did not measure the corpus (a case that cannot run, an empty label set, zero extracted
// tests, a malformed record, a binary that cannot emit test records) fails instead of printing a
// smaller table.
//
// With `--baseline-binary` the same corpus is also run through an older binary and each expectation
// is classed as retained, newly satisfied, corrected, regressed, a new forbidden finding or still
// unmet, with findings nobody labelled compared the same way. A case the baseline cannot be run or
// read on (a crash, an old report schema, no extracted tests) is `unavailable`, never zero findings.
// `--baseline-ref` names the commit the baseline was built from, so cases whose labels or files
// differ from it are totalled apart from detector changes. `--fail-on-regression` exits 1 on a
// regression or an unavailable case (one new or changed since the baseline stays data).
import { readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import {
  binaryVersion,
  buildRunRecord,
  compareCorpus,
  readLabelChanges,
  renderComparison,
  runGit,
} from './lib/corpus-comparison.mjs'
import { buildReport, evaluateCase, renderText } from './lib/corpus-evaluation.mjs'
import { readRecallManifest } from './lib/corpus-labels.mjs'
import { runAsGate } from './lib/gate-main.mjs'
import { observeRecallCase } from './lib/recall-runner.mjs'
import { defaultBinary } from './lib/run-analyzer.mjs'

const here = fileURLToPath(new URL('.', import.meta.url))
const DEFAULT_ROOT = resolve(here, '..', '..', 'fixtures', 'recall')

/** Options that take a value, and the option each sets. */
const VALUE_OPTIONS = Object.freeze({
  '--root': 'root',
  '--json': 'json',
  '--baseline-binary': 'baseline',
  '--baseline-ref': 'baselineRef',
})

/**
 * Parses the command line.
 *
 * @param {string[]} argv Arguments after the script name.
 * @returns {{binary: string, root: string, json: string | null, failOnViolation: boolean,
 *   baseline: string | null, baselineRef: string | null, failOnRegression: boolean}}
 * @throws {Error} On an unknown flag, a flag missing its value, or a second positional argument,
 *   because an ignored argument would silently evaluate something other than what was asked.
 */
export function parseArguments(argv) {
  const options = {
    binary: null,
    root: DEFAULT_ROOT,
    json: null,
    failOnViolation: false,
    baseline: null,
    baselineRef: null,
    failOnRegression: false,
  }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--fail-on-violation') {
      options.failOnViolation = true
    } else if (argument === '--fail-on-regression') {
      options.failOnRegression = true
    } else if (VALUE_OPTIONS[argument] !== undefined) {
      const value = argv[index + 1]
      if (value === undefined || value === '' || value.startsWith('--')) throw new Error(`${argument} needs a value`)
      options[VALUE_OPTIONS[argument]] = argument === '--root' ? resolve(value) : value
      index += 1
    } else if (argument.startsWith('--')) {
      throw new Error(`unknown option ${argument}`)
    } else if (options.binary === null) {
      options.binary = argument
    } else {
      throw new Error(`unexpected argument ${argument}`)
    }
  }
  if (options.baselineRef?.startsWith('-')) throw new Error('--baseline-ref must be a revision, not an option')
  if (options.baseline === null && (options.baselineRef !== null || options.failOnRegression)) {
    throw new Error('--baseline-ref and --fail-on-regression need --baseline-binary')
  }
  options.binary ??= defaultBinary()
  return options
}

/**
 * Runs the evaluation.
 *
 * @param {string[]} argv Arguments after the script name.
 * @param {object} [deps]
 * @param {typeof observeRecallCase} [deps.observe] Case runner, replaceable by a test.
 * @param {typeof writeFileSync} [deps.write] File writer for `--json`.
 * @param {(args: string[]) => string} [deps.git] Runs git in the checkout.
 * @param {typeof binaryVersion} [deps.version] Reports a binary's version line.
 * @param {typeof readLabelChanges} [deps.readChanges] Finds the cases that differ from the baseline ref.
 * @returns {{status: number, lines: string[]}} The exit status and the lines to print.
 */
export function runCli(
  argv,
  {
    observe = observeRecallCase,
    write = writeFileSync,
    git = (args) => runGit(args, here),
    version = binaryVersion,
    readChanges = readLabelChanges,
  } = {},
) {
  const options = parseArguments(argv)
  const labels = readRecallManifest(options.root)
  /** Runs one case through `binary` and evaluates it; the same description serves both binaries. */
  const evaluateWith = (binary, caseLabel, extraArguments, evaluation) => {
    const testCase = { name: caseLabel.name, path: caseLabel.path, arguments: caseLabel.arguments }
    const own = labels.expectations.filter((expectation) => expectation.caseName === caseLabel.name)
    return evaluateCase(caseLabel, own, observe({ binary, root: options.root, testCase, extraArguments }), evaluation)
  }
  const cases = labels.cases.map((caseLabel) => evaluateWith(options.binary, caseLabel, ['--emit-test-fingerprints']))
  const report = buildReport({ binary: options.binary, root: options.root, labels, cases })
  const lines = renderText(report)
  let regressed = false
  if (options.baseline !== null) {
    const baselines = labels.cases.map((caseLabel) => {
      try {
        return { evaluated: evaluateWith(options.baseline, caseLabel, [], { requireTestRecords: false, allowPendingZeroTests: false }) }
      } catch (error) {
        // The binary's own path says nothing the reader does not already know.
        return { unavailable: String(error.message).split('\n')[0].split(options.baseline).join('the baseline binary') }
      }
    })
    const manifestCases = JSON.parse(readFileSync(resolve(options.root, 'manifest.json'), 'utf8')).cases
    const labelChanges =
      options.baselineRef === null
        ? new Map()
        : readChanges({ root: options.root, ref: options.baselineRef, manifestCases, git })
    report.comparison = compareCorpus(cases, baselines, labelChanges)
    report.comparison.runRecord = buildRunRecord({
      candidate: options.binary,
      baseline: options.baseline,
      root: options.root,
      ref: options.baselineRef,
      cases: labels.cases,
      git,
      version,
    })
    lines.push(...renderComparison(report.comparison))
    if (options.baselineRef === null) lines.push('  no --baseline-ref: cases whose labels changed since the baseline are not separated')
    regressed = options.failOnRegression && (report.comparison.regressions > 0 || report.comparison.unavailableCases > 0)
  }
  if (options.json !== null) write(options.json, `${JSON.stringify(report, null, 2)}\n`)
  const violated = options.failOnViolation && report.violations > 0
  return { status: violated || regressed ? 1 : 0, lines }
}

runAsGate(import.meta.url, runCli, 'corpus evaluation')
