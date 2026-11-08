#!/usr/bin/env node
// Generated table titles: every title the analyzer proves for a generated `.each`/`.for` case must
// be the one real Vitest collects. Refusing to prove a title is allowed; proving a different one,
// or a different number of rows, is a disagreement.
//
// Cases come from a fixed seed (override with `--seed`) plus seeded shapes that have diverged
// before. Each case is its own file, so a case Vitest refuses to collect loses only itself. A
// disagreement is shrunk, one Vitest run per round, to a case no single step smaller still
// disagrees, and printed as a test file to add to `scripts/vitest-harness/title-values.test.ts`.
//
// Floors on work performed: the run fails when fewer cases were compared, or fewer titles proved,
// than the floors below; refusing everything must not read as agreement.
import { mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { analyzerRecordsOf, collectWithVitest, normalizePath, oracleBinary } from './lib/collection-oracle.mjs'
import { generate, holdsLongValue, random, shrinkCandidates, spellCase, verdict } from './lib/title-cases.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const SEED = 0x7e57
const CASES = 1000
// Measured on the default seed: 1,005 compared, 697 proved, 168 long-value cases compared.
const MIN_COMPARED = 950
const MIN_PROVED = 600
const MIN_LONG_COMPARED = 150
const SHRINK_ROUNDS = 12
/** A display cut past every generated value, so Vitest renders long values whole. */
const LONG_CUT = 100
const DISAGREEMENTS = new Set(['count', 'title', 'phantom'])

/** Shapes that diverged from Vitest before, always included. */
const SEEDED = [
  { api: 'for', pattern: ['value ', '%s'], rows: [{ kind: 'scalar', cells: [[null, 'null']] }] },
  { api: 'each', pattern: ['$01'], rows: [{ kind: 'array', cells: [[null, "'p'"], [null, "'q'"]] }] },
  { api: 'each', pattern: ['$', '%#'], rows: [{ kind: 'array', cells: [[null, "'p'"]] }] },
  { api: 'each', pattern: ['$1'], rows: [{ kind: 'object', cells: [['1', "'p'"]] }] },
  { api: 'each', pattern: ['%i'], rows: [{ kind: 'array', cells: [[null, "'1.99999999999999999999'"]] }] },
  { api: 'each', pattern: ['$name'], rows: [{ kind: 'object', cells: [['name', "'n'"]] }] },
  { api: 'each', pattern: ['$labelに', 'a'], rows: [{ kind: 'object', cells: [["'labelに'", "'v'"]] }] },
  { api: 'each', pattern: ['$a', '→', '$b'], rows: [{ kind: 'object', cells: [['a', "'F'"], ['b', "'T'"]] }] },
]

/**
 * Reads `--seed`, `--cases` and `--binary`.
 * @param {string[]} args The command line after the script.
 * @returns {{seed: number, cases: number, binary: string | undefined}} The options.
 */
function options(args) {
  const parsed = { seed: SEED, cases: CASES, binary: undefined }
  for (let i = 0; i < args.length; i++) {
    const value = args[i + 1]
    if (args[i] === '--seed') parsed.seed = Number(value)
    else if (args[i] === '--cases') parsed.cases = Number(value)
    else if (args[i] === '--binary') parsed.binary = value
    else throw new Error(`unknown argument ${args[i]}`)
    i++
  }
  if (!Number.isInteger(parsed.seed) || !Number.isInteger(parsed.cases) || parsed.cases < 1) {
    throw new Error('--seed and --cases take integers, and --cases at least 1')
  }
  return parsed
}

/**
 * Collects `cases` with Vitest and the analyzer in one throwaway project.
 * @param {object[]} cases The cases.
 * @param {string} binary The analyzer.
 * @param {number | null} truncate The project's `taskTitleValueFormatTruncate`, or `null` for the default.
 * @returns {object[]} Each case's verdict, in order.
 */
function judge(cases, binary, truncate) {
  const work = realpathSync(mkdtempSync(join(tmpdir(), 'vitest-dedup-titles-')))
  try {
    symlinkSync(join(ROOT, 'node_modules'), join(work, 'node_modules'), process.platform === 'win32' ? 'junction' : 'dir')
    if (truncate !== null) {
      writeFileSync(join(work, 'vitest.config.mjs'), `export default { test: { taskTitleValueFormatTruncate: ${truncate} } }\n`)
    }
    const names = cases.map((_, i) => `case-${String(i).padStart(5, '0')}.test.ts`)
    cases.forEach((testCase, i) => writeFileSync(join(work, names[i]), spellCase(testCase)))
    const collected = collectWithVitest(ROOT, work)
    const records = new Map()
    for (const record of analyzerRecordsOf(binary, [work, '--emit-test-fingerprints'])) {
      if (record.type !== 'test') continue
      const path = normalizePath(record.path)
      if (!records.has(path)) records.set(path, [])
      records.get(path).push(record)
    }
    return names.map((name) => verdict(collected.get(name), records.get(name) ?? []))
  } finally {
    rmSync(work, { recursive: true, force: true })
  }
}

/**
 * Shrinks each failing case to one no single step smaller still fails the same way, one Vitest
 * run per round across all of them.
 * @param {Array<{testCase: object, result: object}>} failing The failing cases and verdicts.
 * @param {string} binary The analyzer.
 * @param {number | null} truncate The display cut they failed under.
 * @returns {Array<{testCase: object, result: object}>} The shrunk cases.
 */
function shrink(failing, binary, truncate) {
  let current = failing
  for (let round = 0; round < SHRINK_ROUNDS; round++) {
    const candidates = current.map(({ testCase }) => shrinkCandidates(testCase))
    const results = judge(candidates.flat(), binary, truncate)
    let offset = 0
    let progressed = false
    current = current.map((entry, i) => {
      const own = results.slice(offset, offset + candidates[i].length)
      offset += candidates[i].length
      const smaller = own.findIndex((result) => result.status === entry.result.status)
      if (smaller === -1) return entry
      progressed = true
      return { testCase: candidates[i][smaller], result: own[smaller] }
    })
    if (!progressed) break
  }
  return current
}

/**
 * Judges `cases` under one display cut, prints its tally and shrunk disagreements.
 * @param {string} label The pass's name.
 * @param {object[]} cases The cases.
 * @param {string} binary The analyzer.
 * @param {number | null} truncate The display cut.
 * @returns {{compared: number, proved: number, disagreements: number}} What it measured.
 */
function pass(label, cases, binary, truncate) {
  const results = judge(cases, binary, truncate)
  const tally = Object.groupBy(results, (result) => result.status)
  const compared = results.length - (tally['vitest-error']?.length ?? 0)
  const proved = results.reduce((sum, result) => sum + result.proved, 0)
  const rows = results.reduce((sum, result) => sum + result.rows, 0)
  console.log(
    `${label}: ${cases.length} cases, ${compared} compared (${rows} rows, ${proved} proved titles); ` +
      Object.entries(tally).map(([status, list]) => `${status} ${list.length}`).join(', '),
  )
  const failing = cases.flatMap((testCase, i) => (DISAGREEMENTS.has(results[i].status) ? [{ testCase, result: results[i] }] : []))
  const shrunk = failing.length > 0 ? shrink(failing, binary, truncate) : []
  const distinct = new Map(shrunk.map((entry) => [spellCase(entry.testCase), entry]))
  for (const [source, { result }] of distinct) {
    console.error(`\n${result.status.toUpperCase()} disagreement (${label}):\n${source}${JSON.stringify(result.mismatches)}`)
  }
  return { compared, proved, disagreements: distinct.size }
}

const { seed, cases, binary: binaryArgument } = options(process.argv.slice(2))
const binary = oracleBinary(ROOT, binaryArgument)
if (binary === null) process.exit(2)
const next = random(seed)
const all = [...SEEDED, ...Array.from({ length: cases }, () => generate(next))]
const standard = pass(`seed ${seed}, default display cut`, all, binary, null)
// A project can lengthen the cut; only long values render differently, so only they rerun.
const long = pass(`seed ${seed}, taskTitleValueFormatTruncate ${LONG_CUT}`, all.filter(holdsLongValue), binary, LONG_CUT)
const failures = []
const floor = (measured, minimum, what) => {
  if (cases >= CASES && measured < minimum) failures.push(`${measured} ${what}, below the ${minimum} floor`)
}
floor(standard.compared, MIN_COMPARED, 'cases compared')
floor(standard.proved, MIN_PROVED, 'proved titles (refusing everything is not agreement)')
floor(long.compared, MIN_LONG_COMPARED, 'long-value cases compared')
const disagreements = standard.disagreements + long.disagreements
if (disagreements > 0) failures.push(`${disagreements} distinct disagreement(s) after shrinking`)
for (const failure of failures) console.error(`FAIL: ${failure}`)
process.exit(failures.length > 0 ? 1 : 0)
