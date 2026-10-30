#!/usr/bin/env node
// Shared subprocess invocation for the corpus gates.
//
// This is the single place that knows the analyzer's exit-code contract: 0 (clean run, no
// findings) and 1 (findings exist, analysis still completed normally) are both legitimate outcomes
// and return normally; anything else -- 2 (operational failure: bad args, unreadable input, a
// panic or no tests analyzed), a signal, a missing binary -- is a hard error that throws rather than being silently
// treated as "the analyzer found nothing" or folded into "the analyzer found something".
//
// That distinction was implemented correctly in `check-duplicates-corpus.mjs`, then implemented
// WRONG in `run-external-corpus.mjs` (a bare try/catch that accepted *any* non-zero exit as long as
// stdout was non-empty), fixed there, and only then discovered to have regressed back in the first
// file by a later review -- because each script invoked the binary itself and was reviewed in
// isolation. For a duplicate-detection tool, shipping the identical defect in two copies of
// near-identical code is about as on-the-nose as a bug can get. This function exists so there is
// now exactly one copy of the contract to review, or to get wrong.
import { spawnSync } from 'node:child_process'
import { join } from 'node:path'

/**
 * The release binary `cargo build --release` writes, spelled for the platform: `vitest-dedup.exe`
 * on Windows, bare everywhere else. The six gates that default to it share this one spelling so a
 * Windows checkout does not fail every gate's existence check right after a successful build
 * (CodeRabbit, PR #40).
 *
 * @param {object} [options]
 * @param {string} [options.platform] - `process.platform` unless a test says otherwise.
 * @param {string} [options.root] - checkout root for an absolute path; omitted, the path is the
 *   cwd-relative `./target/release/...` the gates have always printed.
 * @returns {string} the binary path.
 */
export function defaultBinary({ platform = process.platform, root } = {}) {
  const name = `vitest-dedup${platform === 'win32' ? '.exe' : ''}`
  return root ? join(root, 'target', 'release', name) : `./target/release/${name}`
}

/** Whether an analyzer process completed with a quality result that corpus gates can inspect. */
export function isCompletedAnalysisExit(status) {
  return status === 0 || status === 1
}

/**
 * Runs the analyzer binary with `--reporters jsonl` and returns its parsed output.
 *
 * @param {string} binary - path to the vitest-dedup binary.
 * @param {string[]} args - CLI arguments *excluding* `--reporters jsonl`, which this function
 *   always appends as the last flags.
 * @param {Record<string, string>} [env] - variables set for the run, over the inherited ones.
 * @returns {{records: object[], stderr: string}} `records` is every non-blank stdout line parsed
 *   as JSON, in order. `stderr` is the raw stderr text, for callers that want to surface
 *   diagnostics (e.g. the unresolved-context warning) even on a successful run.
 * @throws {Error} if the process could not be spawned (e.g. binary missing), was killed by a
 *   signal, or exited with a code other than 0 or 1. The message names the actual exit code and
 *   includes stderr, so a crash is never confused with "zero findings" or "findings exist".
 */
export function runAnalyzerJsonl(binary, args, env = {}) {
  const result = spawnSync(binary, [...args, '--reporters', 'jsonl'], {
    encoding: 'utf8',
    env: { ...process.env, ...env },
    maxBuffer: 256 << 20,
  })

  if (result.error) throw result.error

  if (!isCompletedAnalysisExit(result.status)) {
    const signalNote = result.signal ? ` (signal ${result.signal})` : ''
    const stderrNote = result.stderr && result.stderr.trim() ? `\n${result.stderr.trim()}` : ''
    const error = new Error(
      `${binary} exited ${result.status}${signalNote} -- an operational failure, not "zero ` +
        `findings" or "findings exist". Only exit codes 0 and 1 are accepted.${stderrNote}`,
    )
    // What the run printed before failing, for a caller that accepts one specific exit 2.
    error.stdout = result.stdout ?? ''
    throw error
  }

  return { records: parseJsonl(result.stdout), stderr: result.stderr ?? '' }
}

/** The records of a JSONL report. */
export function parseJsonl(stdout) {
  return stdout
    .split('\n')
    .filter((line) => line.trim() !== '')
    .map((line) => JSON.parse(line))
}
