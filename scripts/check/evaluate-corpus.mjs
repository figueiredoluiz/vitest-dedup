#!/usr/bin/env node
// Reports what one analyzer binary does against the labelled recall corpus.
//
// Usage: node scripts/check/evaluate-corpus.mjs [binary] [--root DIR] [--json FILE]
//                                               [--fail-on-violation]
//
// Unlike `check-recall.mjs`, which stops at the first broken contract, this runs every case, never
// stops on a violation and reports each one with the denominator behind it: expected findings
// satisfied, forbidden findings held, findings nobody labelled, and for each silence the corpus
// claims whether the pair was eligible for comparison. It is a measurement, so it stays out of
// `npm run check`; a violation is data and exits 0 unless `--fail-on-violation` is given. A run
// that did not measure the corpus (a case that cannot run, an empty label set, zero extracted
// tests, a malformed record, a binary that cannot emit test records) fails instead of printing a
// smaller table.
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { buildReport, evaluateCase, renderText } from './lib/corpus-evaluation.mjs'
import { readRecallManifest } from './lib/corpus-labels.mjs'
import { runAsGate } from './lib/gate-main.mjs'
import { observeRecallCase } from './lib/recall-runner.mjs'
import { defaultBinary } from './lib/run-analyzer.mjs'

const here = fileURLToPath(new URL('.', import.meta.url))
const DEFAULT_ROOT = resolve(here, '..', '..', 'fixtures', 'recall')

/**
 * Parses the command line.
 *
 * @param {string[]} argv Arguments after the script name.
 * @returns {{binary: string, root: string, json: string | null, failOnViolation: boolean}}
 * @throws {Error} On an unknown flag, a flag missing its value, or a second positional argument,
 *   because an ignored argument would silently evaluate something other than what was asked.
 */
export function parseArguments(argv) {
  const options = { binary: null, root: DEFAULT_ROOT, json: null, failOnViolation: false }
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index]
    if (argument === '--fail-on-violation') {
      options.failOnViolation = true
    } else if (argument === '--root' || argument === '--json') {
      const value = argv[index + 1]
      if (value === undefined || value.startsWith('--')) throw new Error(`${argument} needs a value`)
      options[argument === '--root' ? 'root' : 'json'] = argument === '--root' ? resolve(value) : value
      index += 1
    } else if (argument.startsWith('--')) {
      throw new Error(`unknown option ${argument}`)
    } else if (options.binary === null) {
      options.binary = argument
    } else {
      throw new Error(`unexpected argument ${argument}`)
    }
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
 * @returns {{status: number, lines: string[]}} The exit status and the lines to print.
 */
export function runCli(argv, { observe = observeRecallCase, write = writeFileSync } = {}) {
  const options = parseArguments(argv)
  const labels = readRecallManifest(options.root)
  const cases = labels.cases.map((caseLabel) => {
    const testCase = { name: caseLabel.name, path: caseLabel.path, arguments: caseLabel.arguments }
    const observation = observe({
      binary: options.binary,
      root: options.root,
      testCase,
      extraArguments: ['--emit-test-fingerprints'],
    })
    const own = labels.expectations.filter((expectation) => expectation.caseName === caseLabel.name)
    return evaluateCase(caseLabel, own, observation)
  })
  const report = buildReport({ binary: options.binary, root: options.root, labels, cases })
  if (options.json !== null) write(options.json, `${JSON.stringify(report, null, 2)}\n`)
  const status = options.failOnViolation && report.violations > 0 ? 1 : 0
  return { status, lines: renderText(report) }
}

runAsGate(import.meta.url, runCli, 'corpus evaluation')
