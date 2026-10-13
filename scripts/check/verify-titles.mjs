#!/usr/bin/env node
// The differential-title harness (V4): runs ONE fixture through real Vitest and through this
// repository's analyzer, and asserts the rendered `.each` titles byte-match.
//
// ---------------------------------------------------------------------------------------------
// Why this exists
// ---------------------------------------------------------------------------------------------
//
// The strongest verification in the `.each` work was procedural, not committed: a probe ran one
// file through both the analyzer and real Vitest and diffed the titles. It produced every
// measurement in `docs/vitest-surface.md` §4 and caught two defects in the T6 mirror -- and then
// it was gone, because it lived in a review session instead of in the tree. This file is the
// permanent form of that probe. When either side changes -- a Vitest upgrade, a renderer edit --
// the diff re-runs here instead of from memory.
//
// What a mismatch means: the analyzer's rendered title and the runtime string have diverged for
// the value that titles the mismatched row. Titles reach `duplicate-test-name` and
// `duplicate-each-row` as comparison keys, so a divergence is a real recall or precision claim,
// not a cosmetic one. Fix the renderer (or, for a genuinely unmeasured mechanism, measure first
// and record the boundary in §4) -- never edit this fixture to make a mismatch disappear.
//
// Floors on work performed: the harness fails when Vitest is not installed or is not the PINNED
// version (an npm-ci-less or stale checkout must say so, not report a pass against the wrong
// oracle), when Vitest ran fewer tests than the fixture is written with, when the two sides
// disagree on the count, and when the analyzer extracts nothing. A harness that compared two
// empty lists would pass while measuring nothing.
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { spawnSync } from 'node:child_process'
import { defaultBinary, runAnalyzerJsonl } from './lib/run-analyzer.mjs'

const FIXTURE = 'scripts/vitest-harness/title-values.test.ts'
const FIXTURE_DIR = 'scripts/vitest-harness'
const IMPORTED_FIXTURE = 'scripts/vitest-harness/imported/imported.test.ts'
const binary = process.argv[2] ?? defaultBinary()

// Vitest is spawned as a module under this Node, not through the `.bin` shim: the shim is a
// POSIX shell script, so on Windows or without its executable bit `spawnSync` would fail with a
// bare spawn error and empty output.
const vitestEntry = join('node_modules', 'vitest', 'vitest.mjs')
const vitestPackage = join('node_modules', 'vitest', 'package.json')
if (!existsSync(vitestEntry) || !existsSync(vitestPackage)) {
  console.error(
    `FAIL: ${vitestEntry} not found. This harness diffs this analyzer's rendered titles against\n` +
      `real Vitest, so Vitest must be installed: npm ci (vitest is a pinned devDependency).\n` +
      'A missing Vitest is a failure, not a skip: a diff that ran measures nothing.',
  )
  process.exit(2)
}

// The oracle must be the pinned Vitest. The JSON report carries no version, so the installed
// package is read directly; a stale node_modules with another Vitest would otherwise become the
// runtime silently, and every §4 claim says "measured on 5.0.3".
const pinned = JSON.parse(readFileSync('package.json', 'utf8')).devDependencies?.vitest
const installed = JSON.parse(readFileSync(vitestPackage, 'utf8')).version
if (typeof pinned !== 'string' || !/^\d+\.\d+\.\d+$/.test(pinned)) {
  console.error(
    `FAIL: package.json must pin vitest to an exact version (found ${JSON.stringify(pinned)}); ` +
      'the harness asserts the installed oracle against that pin.',
  )
  process.exit(2)
}
if (installed !== pinned) {
  console.error(
    `FAIL: installed Vitest is ${installed}, package.json pins ${pinned}. Run npm ci: a diff ` +
      'against an unpinned runtime measures a version nobody recorded.',
  )
  process.exit(2)
}
if (!existsSync(binary)) {
  console.error(`FAIL: analyzer binary ${binary} not found. Build it: cargo build --release --locked`)
  process.exit(2)
}

// The work-performed floor, stated as a floor on what VITEST ran (the runtime oracle): a
// gutted fixture would otherwise let both sides agree on nothing. The source-derived row count
// this replaced counted regex shapes -- brittle against any fixture edit, and it validated the
// prose rather than the measurement.
const MINIMUM_ROWS = 20

// 1. The runtime truth: real Vitest, JSON reporter, written to a temp file so the reporter's
// stdout cannot interleave with anything else.
const outDir = mkdtempSync(join(tmpdir(), 'vitest-dedup-titles-'))
const vitestJson = join(outDir, 'vitest.json')
const vitestRun = spawnSync(
  process.execPath,
  // Cargo's shared target cache can contain an unpacked package with copies of these fixtures.
  // Vitest treats the file arguments as filters, so exclude that generated tree at discovery.
  [vitestEntry, 'run', FIXTURE, IMPORTED_FIXTURE, '--exclude', '**/target/**',
    '--reporter=json', `--outputFile=${vitestJson}`],
  { encoding: 'utf8' },
)
if (vitestRun.error || vitestRun.status !== 0 || !existsSync(vitestJson)) {
  console.error(
    `FAIL: vitest run did not complete successfully` +
      (vitestRun.error ? ` (spawn error: ${vitestRun.error.message})` : ` (exit ${vitestRun.status ?? vitestRun.signal})`) +
      '. stderr/stdout:',
  )
  console.error(vitestRun.stderr ?? '')
  console.error(vitestRun.stdout ?? '')
  rmSync(outDir, { recursive: true, force: true })
  process.exit(2)
}
const vitestReport = JSON.parse(readFileSync(vitestJson, 'utf8'))
const normalizePath = (path) => path.replaceAll('\\', '/')
const [titleFile, importedFile] = [FIXTURE, IMPORTED_FIXTURE].map((path) => normalizePath(resolve(path)))
const expectedFiles = [titleFile, importedFile].sort()
const reportedFiles = vitestReport.testResults.map((result) => normalizePath(result.name)).sort()
if (JSON.stringify(reportedFiles) !== JSON.stringify(expectedFiles)) {
  console.error(`FAIL: Vitest reported unexpected fixture files: ${JSON.stringify(reportedFiles)}`)
  rmSync(outDir, { recursive: true, force: true })
  process.exit(2)
}
const vitestTitles = vitestReport.testResults
  .filter((result) => normalizePath(result.name) === titleFile)
  .flatMap((result) => result.assertionResults)
  .map((assertion) => assertion.title)
const importedTitles = vitestReport.testResults
  .filter((result) => normalizePath(result.name) === importedFile)
  .flatMap((result) => result.assertionResults)
  .map((assertion) => [...assertion.ancestorTitles, assertion.title].join(' > '))
rmSync(outDir, { recursive: true, force: true })

// 2. The analyzer's rendering, over the same file.
// `--emit-test-fingerprints` is what puts per-test records on the jsonl stream; without it the
// run emits only the summary and the harness would compare against nothing.
const { records, stderr } = runAnalyzerJsonl(binary, [FIXTURE_DIR, '--emit-test-fingerprints'])
// Keep the primary title fixture separate from the imported registration oracle below.
const analyzerTitles = records
  .filter((record) => record.type === 'test' && record.path === 'title-values.test.ts')
  .map((record) => record.title)
if (analyzerTitles.length === 0) {
  console.error(
    'FAIL: the analyzer extracted no per-test records. Without --emit-test-fingerprints the ' +
      'jsonl stream carries only a summary, and a diff against nothing is not a diff.',
  )
  process.exit(1)
}

// 3. The diff: a multiset comparison, so row order (a presentation detail) cannot matter and a
// title Vitest renders twice must be rendered twice here too.
const countTitles = (titles) => {
  const counts = new Map()
  for (const title of titles) counts.set(title, (counts.get(title) ?? 0) + 1)
  return counts
}
const expectedCounts = countTitles(vitestTitles)
const actualCounts = countTitles(analyzerTitles)
const missing = []
const unexpected = []
for (const [title, count] of expectedCounts) {
  for (let i = actualCounts.get(title) ?? 0; i < count; i++) missing.push(title)
}
for (const [title, count] of actualCounts) {
  for (let i = expectedCounts.get(title) ?? 0; i < count; i++) unexpected.push(title)
}
const reportDiff = () => {
  console.error(`FAIL: ${missing.length} title(s) Vitest renders that the analyzer does not:`)
  for (const title of missing.slice(0, 20)) console.error(`  - ${JSON.stringify(title)}`)
  console.error(`FAIL: ${unexpected.length} title(s) the analyzer renders that Vitest does not:`)
  for (const title of unexpected.slice(0, 20)) console.error(`  + ${JSON.stringify(title)}`)
  console.error('(stderr from the analyzer run, for context):')
  console.error(stderr)
}

console.log(`vitest tests: ${vitestTitles.length}, analyzer tests: ${analyzerTitles.length}`)
if (vitestTitles.length < MINIMUM_ROWS) {
  console.error(
    `FAIL: Vitest ran ${vitestTitles.length} test(s), below the ${MINIMUM_ROWS} floor. A harness ` +
      `measuring fewer cases than it was written with is measuring nothing.`,
  )
  process.exit(1)
}
if (analyzerTitles.length !== vitestTitles.length) {
  // A count mismatch is a finding in itself -- an unproven value refuses its row rather than
  // rendering it -- and the diff below is what names the refused row.
  console.error(
    `FAIL: the analyzer extracted ${analyzerTitles.length} test(s) but Vitest ran ` +
      `${vitestTitles.length}.`,
  )
  reportDiff()
  process.exit(1)
}
if (missing.length > 0 || unexpected.length > 0) {
  reportDiff()
  process.exit(1)
}
console.log(`ok: ${analyzerTitles.length} rendered title(s) byte-match real Vitest ${installed}`)

// The same owned input is included by the frontend regression. Repeated registrations must
// remain repeated here; a set would hide the original imported-extraction defect.
const importedRun = runAnalyzerJsonl(binary, ['scripts/vitest-harness/imported', '--emit-test-fingerprints'])
const importedTests = importedRun.records.filter((record) => record.type === 'test')
const importedSummary = importedRun.records.find((record) => record.type === 'summary')
if (importedTitles.length !== 7 || importedTests.length !== 7 || importedSummary?.incomplete !== false) {
  throw new Error(`Imported runner oracle requires 7 runtime/analyzer tests and a complete analysis; got ${importedTitles.length}/${importedTests.length}, incomplete=${importedSummary?.incomplete}`)
}
/** Compares title-frequency multisets independently of runtime registration order. */
const sortedCounts = (titles) => [...countTitles(titles)].sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
if (JSON.stringify(sortedCounts(importedTitles)) !== JSON.stringify(sortedCounts(importedTests.map((record) => record.title)))) {
  throw new Error('Imported runner titles differ from real Vitest registrations')
}
const helperTests = importedTests.filter((record) => record.path === 'runner.ts')
if (helperTests.length !== 6 || helperTests.some((record) => record.registrationFile !== 'imported.test.ts' || !record.comparable)) {
  throw new Error('Imported declaration source/caller identity or comparison eligibility was lost')
}
console.log(`ok: 7 imported-runner registrations byte-match real Vitest ${installed} on ${process.version}`)
