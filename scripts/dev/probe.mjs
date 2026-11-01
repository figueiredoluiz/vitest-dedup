#!/usr/bin/env node
// Vitest and both engines, side by side, on throwaway inputs.
//
// Each argument is a test file, or a directory holding one and its helpers. It is copied to a
// temporary project that can resolve the checkout's Vitest, so nothing is written to the repository.
import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, mkdtempSync, realpathSync, rmSync, statSync, symlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { collectNextEngine, collectWithVitest } from '../check/lib/collection-oracle.mjs'
import { parseJsonl } from '../check/lib/run-analyzer.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')

/** Which claim a finding makes: its rule and where. */
const identity = ({ rule, locations = [] }) =>
  `${rule} ${locations.map(({ path, line }) => `${path}:${line}`).sort().join(',')}`

/**
 * What one analyzer run reported.
 * @param {object[]} records The run's JSONL records.
 * @param {number|null} exit Its exit code, `null` when a signal ended it.
 * @param {string} [stderr] What it printed, kept for a run that failed.
 * @returns {{exit: number|null, failure: string|null, findings: string[], rules: string[], tests: number, unresolved: number, incomplete: boolean, reasons: string[]}} The summary.
 */
export function summarize(records, exit, stderr = '') {
  const summary = records.find((record) => record?.type === 'summary')
  const findings = records.filter((record) => record?.type === 'finding')
  // 0 is clean and 1 is findings; anything else, or no summary, is a failure, never agreement.
  const ran = (exit === 0 || exit === 1) && summary !== undefined
  return {
    exit,
    failure: ran ? null : stderr.trim().split('\n').slice(-2).join(' ').slice(0, 200) || 'no summary record',
    // By place, not fingerprint: the two engines fingerprint the same finding differently.
    findings: findings.map(identity).sort(),
    rules: [...new Set(findings.map((record) => record.rule))].sort(),
    tests: summary?.testsAnalyzed ?? 0,
    unresolved: summary?.unresolvedTests ?? 0,
    incomplete: summary?.incomplete === true,
    reasons: [...new Set(records.filter((record) => record?.type === 'excluded-file').map((record) => record.reason))].sort(),
  }
}

/**
 * Where a case's three readings part: each engine's count against what Vitest collected, and the
 * findings of one engine against the other's.
 * @param {{vitest: string[]|null, next: object, current: object}} row One case.
 * @returns {string[]} Disagreements; empty when all three agree.
 */
export function disagreements({ vitest, next, current }) {
  const notes = []
  if (next.failure !== null) notes.push('next-failed')
  if (current.failure !== null) notes.push('default-failed')
  if (vitest === null) notes.push('vitest-unread')
  else {
    if (next.tests !== vitest.length) notes.push('next-count-differs')
    if (current.tests !== vitest.length) notes.push('default-count-differs')
  }
  if (next.findings.join() !== current.findings.join()) notes.push('findings-differ')
  if (next.incomplete !== current.incomplete) notes.push('completeness-differs')
  if (next.unresolved !== current.unresolved || next.reasons.join() !== current.reasons.join()) notes.push('diagnostics-differ')
  return notes
}

/**
 * Render one case for a terminal.
 * @param {{name: string, vitest: string[]|null, error?: string, sites: string[], next: object, current: object}} row One case.
 * @returns {string} Its lines.
 */
export function render(row) {
  const engine = (label, run) =>
    `  ${label}: exit ${run.exit}, ${run.tests} analyzed, ${run.unresolved} unresolved` +
    `${run.incomplete ? ', incomplete' : ''}; rules [${run.rules.join(', ')}]` +
    `${run.reasons.length ? `; excluded [${run.reasons.join(', ')}]` : ''}` +
    `${run.failure === null ? '' : `; failed: ${run.failure}`}`
  const notes = disagreements(row)
  return [
    `${notes.length ? 'DIFF' : 'SAME'} ${row.name}${notes.length ? ` (${notes.join(', ')})` : ''}`,
    `  vitest: ${row.vitest === null ? `unread: ${row.error ?? ''}` : JSON.stringify(row.vitest)}`,
    `  sites: ${row.sites.join(' ') || 'none'}`,
    engine('next', row.next),
    engine('default', row.current),
  ].join('\n')
}

/**
 * Copy `source` into a project under `work`, beside a link to the checkout's dependencies.
 * @param {string} source A test file or a directory.
 * @param {string} work The temporary root.
 * @returns {string} The project directory.
 */
export function project(source, work) {
  // Its own directory, since two inputs can share a basename.
  const directory = mkdtempSync(join(work, 'case-'))
  if (statSync(source).isDirectory()) cpSync(source, directory, { recursive: true })
  else cpSync(source, join(directory, basename(source)))
  return directory
}

/**
 * One engine's run over `directory`. A run that cannot start, times out or prints something that is
 * not JSONL is that row's failure, so it never costs the other inputs their rows.
 * @param {string} binary The analyzer.
 * @param {string} directory The project.
 * @param {string|null} engine The engine to select, or null for the default.
 * @returns {object} The run's summary.
 */
export function analyze(binary, directory, engine) {
  const env = { ...process.env }
  delete env.VITEST_DEDUP_ENGINE
  if (engine) env.VITEST_DEDUP_ENGINE = engine
  const run = spawnSync(binary, [directory, '--reporters', 'jsonl'], { encoding: 'utf8', env, maxBuffer: 256 << 20, timeout: 120_000 })
  try {
    if (run.error) throw run.error
    return summarize(parseJsonl(run.stdout), run.status, run.stderr)
  } catch (error) {
    return summarize([], null, String(error.message))
  }
}

/** The next engine's sites in `directory`; a run that cannot report any is shown by `analyze`. */
function sitesOf(binary, directory) {
  try {
    return [...collectNextEngine(binary, ROOT, directory).sites.values()].flat()
  } catch {
    return []
  }
}

/** Vitest's reading of `directory`, or why it has none. */
function collect(directory) {
  try {
    const files = [...collectWithVitest(ROOT, directory).values()]
    // A read of a stubbed missing module changes what registers, so that collection is not evidence.
    const errors = files.flatMap((file) => [...(file.collectErrors ?? []), ...(file.stubReads ?? [])])
    if (errors.length) return { vitest: null, error: JSON.stringify(errors).slice(0, 200) }
    return { vitest: files.flatMap((file) => file.tests) }
  } catch (error) {
    return { vitest: null, error: String(error.message).slice(0, 200) }
  }
}

/**
 * Probe every source.
 * @param {string[]} sources Files or directories.
 * @param {{binary: string}} options The analyzer to run.
 * @returns {object[]} One row per source.
 */
export function probe(sources, { binary }) {
  const work = realpathSync(mkdtempSync(join(tmpdir(), 'vitest-dedup-probe-')))
  try {
    symlinkSync(join(ROOT, 'node_modules'), join(work, 'node_modules'), process.platform === 'win32' ? 'junction' : 'dir')
    return sources.map((source) => {
      const directory = project(source, work)
      return {
        name: source,
        ...collect(directory),
        sites: sitesOf(binary, directory).map((site) => `${site.kind}@${site.line}`),
        next: analyze(binary, directory, 'next'),
        current: analyze(binary, directory, null),
      }
    })
  } finally {
    rmSync(work, { recursive: true, force: true })
  }
}

/**
 * Read the command line.
 * @param {string[]} args Arguments after the script.
 * @returns {{binary: string, sources: string[]}} The analyzer and the inputs.
 * @throws {Error} When no input is given, or the value of `--binary` is missing.
 */
export function parseArguments(args) {
  const options = { binary: join(ROOT, 'target/release/vitest-dedup'), sources: [] }
  for (let index = 0; index < args.length; index++) {
    if (args[index] !== '--binary') options.sources.push(args[index])
    else if (args[++index]) options.binary = resolve(args[index])
    else throw new Error('--binary needs a path')
  }
  if (options.sources.length === 0) throw new Error('usage: probe.mjs [--binary path] <test file or directory>...')
  return options
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  try {
    const { binary, sources } = parseArguments(process.argv.slice(2))
    for (const row of probe(sources, { binary })) console.log(render(row))
  } catch (error) {
    console.error(error.message)
    process.exitCode = 1
  }
}
