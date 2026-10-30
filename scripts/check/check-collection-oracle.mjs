#!/usr/bin/env node
// Collection oracle: what real Vitest collects from the owned corpus, against what the analyzer
// extracts from it.
//
// Every expectation in the corpus gates is written by hand from our model of Vitest. This gate asks
// Vitest instead. Each fixture directory and recall case is collected by the pinned Vitest through
// `lib/collection-probe-runner.mjs` (describe callbacks run, test and hook bodies do not) and
// analyzed with `--emit-test-fingerprints`; `lib/collection-oracle.mjs` compares test titles and
// hook declaration sites. A disagreement is something Vitest collected that the analyzer never
// read, or something the analyzer recorded that Vitest never registers.
//
// The same collection also checks the next engine's inventory (`VITEST_DEDUP_ENGINE=next`): every
// test and suite Vitest collected must have a registration site of its kind on the line Vitest
// places it on (`lib/collection-oracle.mjs`'s `compareSites`). Sites that registered nothing Vitest
// collected are counted, not failed: those are what the engine's exclusion reasons are for.
//
// Owned cases under `fixtures/oracle/` run the same comparisons against their own `expected.json`
// (`lib/oracle-cases.mjs`).
//
// Known disagreements are listed in `collection-oracle-known.json`, each with its reason. The gate
// fails on a disagreement that is not listed, and on a listed one that no longer occurs: a change
// that fixes one deletes its entry in the same commit.
//
// Not compared, and counted per bucket: files Vitest cannot collect (corpus code is deliberately
// incomplete; a fixture may use a global its config never declares), files whose collection read
// a stub of a missing module (`lib/collection-oracle-stubs.mjs`), files the analyzer excluded,
// files outside the analysis root, type tests and benchmarks. Agreement does not prove the
// analyzer right in general: Vitest reports one run in one environment.
//
// One assumption is made rather than checked: a missing module a file imports values from
// registers nothing when it loads. A stub cannot reproduce what the real module would do on load,
// so a file that imports one only for that (`import './x'`) is not compared; a file that imports
// named values is, as long as its collection reads none of them. Treating every stub load as a
// dependency instead halved the comparison on the owned corpus (374 files to 205).
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { isDeepStrictEqual } from 'node:util'
import { collectNextEngine, collectSites, collectWithAnalyzer, collectWithVitest, compareDirectory, compareSites, matchKnown, oracleBinary } from './lib/collection-oracle.mjs'
import { judgeCase, readCases } from './lib/oracle-cases.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')
const KNOWN = join(ROOT, 'scripts', 'check', 'collection-oracle-known.json')
// The fixture directories the corpus gates analyze, plus `lookalikes` (Vitest must collect no
// test from them either). Every recall case is added from its manifest.
const FIXTURE_DIRS = [
  'fixtures/duplicates', 'fixtures/observed', 'fixtures/near', 'fixtures/mixed', 'fixtures/hooks',
  'fixtures/each', 'fixtures/modifiers', 'fixtures/declarations', 'fixtures/edge',
  'fixtures/resolution', 'fixtures/fixtures', 'fixtures/discovery', 'fixtures/lookalikes',
]

// Floors on work performed, set to what the corpus measured when the gate landed. A gate that
// compared nothing, because the probe stopped loading or every file fell into a bucket, must fail
// rather than report agreement. Raise them when the corpus grows.
const MIN_FILES_COMPARED = 374
const MIN_TESTS_COMPARED = 842
const MIN_HOOK_SITES_COMPARED = 95
// The same floor for the next engine's inventory, whose registration sites are compared with the
// line Vitest places each collected test and suite on.
const MIN_SITE_FILES_COMPARED = 546
const MIN_SITE_TASKS_COMPARED = 1465

// What the probe must report for the self-check directories, checked before anything is compared:
// hooks that do not register (`if (false)`, after `return`), one registered per loop iteration,
// `test.beforeEach`, a helper's hook at the helper's own line, and every way a file can read a stub
// (by name, whole, for its side effects, through a re-export of `then` in a module or a fixture's own
// package, or by sharing one load under `isolate: false`).
const SELF_CHECK = {
  'scripts/check/collection-oracle-self-check': {
    'hooks.test.ts': {
      tests: ['conditional > a', 'loop > b', 'member call > c', 'after return > d', 'helper > e'],
      hooks: ['afterEach@hooks.test.ts:11', 'afterEach@hooks.test.ts:11', 'beforeEach@hooks.test.ts:16', 'beforeEach@hook-helper.ts:4'],
      stubReads: [],
    },
    'stub-body.test.ts': { tests: ['adds'], hooks: [], stubReads: [] },
    'stub-namespace.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#*'] },
    'stub-namespace-comment.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#*'] },
    'stub-read.test.ts': { tests: ['reads a stub while collecting > always'], hooks: [], stubReads: ['./missing-module#enabled'] },
    'stub-side-effect.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#*'] },
    'stub-then.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#then'] },
    'stub-then-package.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#then'] },
    'stub-then-reexport.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#then'] },
  },
}
// Without isolation a later file inherits every stub read made earlier in its worker, so which reads
// a file lists depends on collection order; what must hold is that every file counts as reading one.
const SHARED_SELF_CHECK = {
  'scripts/check/collection-oracle-self-check-shared': {
    'first.test.ts': { tests: ['first'], hooks: [], readsStub: true },
    'second.test.ts': { tests: ['second'], hooks: [], readsStub: true },
  },
  'scripts/check/collection-oracle-self-check-shared-named': {
    'fourth.test.ts': { tests: ['fourth'], hooks: [], readsStub: true },
    'third.test.ts': { tests: ['third'], hooks: [], readsStub: true },
  },
}

/** The probe's report on the self-check directories, as failure messages (none when it matches). */
function selfCheck() {
  const checks = [
    ...Object.entries(SELF_CHECK).map(([dir, want]) => [dir, want, (file) => ({ tests: file.tests, hooks: file.hooks, stubReads: file.stubReads })]),
    ...Object.entries(SHARED_SELF_CHECK).map(([dir, want]) => [dir, want, (file) => ({ tests: file.tests, hooks: file.hooks, readsStub: file.stubReads.length > 0 })]),
  ]
  return checks.flatMap(([dir, want, view]) => {
    const actual = Object.fromEntries([...collectWithVitest(ROOT, dir)].map(([path, file]) => [path, view(file)]))
    return isDeepStrictEqual(actual, want)
      ? []
      : [`probe self-check ${dir}: expected ${JSON.stringify(want)}\n    got ${JSON.stringify(actual)}`]
  })
}

/**
 * Runs the owned cases under `fixtures/oracle/`, each against its own `expected.json`.
 *
 * @returns {{failures: string[], held: string[], dirs: number, files: number}}
 */
function runCases(binary) {
  const { cases, problems } = readCases(ROOT)
  const totals = { failures: [...problems], held: [], dirs: cases.length, files: 0 }
  for (const oracleCase of cases) {
    const { dir } = oracleCase
    const vitest = collectWithVitest(ROOT, dir)
    const next = collectNextEngine(binary, ROOT, dir)
    const judged = judgeCase(oracleCase, {
      vitest,
      located: compareSites(vitest, next.sites),
      titles: compareDirectory(vitest, collectWithAnalyzer(binary, ROOT, dir)),
      ...next,
    })
    totals.failures.push(...judged.failures)
    totals.held.push(...judged.held)
    totals.files += judged.files
  }
  if (totals.files === 0) totals.failures.push(`no case file was compared under fixtures/oracle/; a gate that measured nothing fails`)
  return totals
}

/** Collects every directory with Vitest and both engines, compares them, and returns the exit code. */
function main() {
  const binary = oracleBinary(ROOT, process.argv[2])
  if (binary === null) return 2
  const manifest = JSON.parse(readFileSync(join(ROOT, 'fixtures', 'recall', 'manifest.json'), 'utf8'))
  const dirs = [...FIXTURE_DIRS, ...manifest.cases.map((c) => `fixtures/recall/${c.path}`)]
  const known = JSON.parse(readFileSync(KNOWN, 'utf8')).disagreements

  const totals = { files: 0, tests: 0, hooks: 0, siteFiles: 0, tasks: 0, sites: 0, unexecuted: 0 }
  const knownSites = JSON.parse(readFileSync(KNOWN, 'utf8')).siteDisagreements ?? []
  const foundSites = []
  // Every file not compared, per bucket, with what kept it out (Vitest's first collection error,
  // the first stub read): listed when a floor fails, because a smaller comparison is otherwise
  // reported only as a lower count.
  const buckets = new Map()
  const probeErrors = []
  const found = []
  for (const dir of dirs) {
    const vitest = collectWithVitest(ROOT, dir)
    const located = compareSites(vitest, collectSites(binary, ROOT, dir))
    totals.siteFiles += located.compared.length
    totals.tasks += located.tasksCompared
    totals.sites += located.sites
    totals.unexecuted += located.unexecuted
    foundSites.push(...located.disagreements.map((d) => ({ dir, ...d })))
    const result = compareDirectory(vitest, collectWithAnalyzer(binary, ROOT, dir))
    totals.files += result.compared.length
    totals.tests += result.testsCompared
    totals.hooks += result.hooksCompared
    for (const [bucket, entries] of result.skipped) {
      if (!buckets.has(bucket)) buckets.set(bucket, [])
      buckets.get(bucket).push(...entries.map((entry) => ({ dir, ...entry })))
      if (bucket === 'probe-error') probeErrors.push(...entries.map((e) => `${dir}/${e.path}`))
    }
    found.push(...result.disagreements.map((d) => ({ dir, ...d })))
  }

  const selfCheckFailures = selfCheck()
  const cases = runCases(binary)
  const { unlisted, stale } = matchKnown(found, known)
  const sitesMatched = matchKnown(foundSites, knownSites)

  console.log(
    `collection oracle: ${dirs.length} directories, ${totals.files} files compared, ` +
      `${totals.tests} tests, ${totals.hooks} hook sites; ${found.length} disagreements ` +
      `(${found.length - unlisted.length} known)`,
  )
  console.log(`  not compared: ${[...buckets].map(([bucket, entries]) => `${bucket} ${entries.length}`).join(', ')}`)
  console.log(
    `  next-engine sites: ${totals.siteFiles} files, ${totals.tasks} collected tests and suites, ` +
      `${foundSites.length} without a site (${foundSites.length - sitesMatched.unlisted.length} known); ` +
      `${totals.unexecuted} of ${totals.sites} sites registered nothing Vitest collected`,
  )

  console.log(`  oracle cases: ${cases.dirs} directories, ${cases.files} files compared, ${cases.held.length} held`)
  for (const line of cases.held) console.log(`    held: ${line}`)

  const failures = [...selfCheckFailures, ...cases.failures]
  if (probeErrors.length > 0) failures.push(`probe errors (the probe and the collected suites disagree):\n    ${probeErrors.join('\n    ')}`)
  for (const d of unlisted) failures.push(`new disagreement: ${d.dir}/${d.path}: ${d.kind} ${JSON.stringify(d.item)}`)
  for (const d of stale) {
    failures.push(`listed disagreement no longer occurs (delete it from collection-oracle-known.json): ${d.dir}/${d.path}: ${d.kind} ${JSON.stringify(d.item)}`)
  }
  for (const d of sitesMatched.unlisted) failures.push(`new site disagreement: ${d.dir}/${d.path}: ${d.kind} ${d.item}`)
  for (const d of sitesMatched.stale) {
    failures.push(`listed site disagreement no longer occurs (delete it from collection-oracle-known.json): ${d.dir}/${d.path}: ${d.kind} ${d.item}`)
  }
  if (totals.siteFiles < MIN_SITE_FILES_COMPARED) failures.push(`compared sites in ${totals.siteFiles} files, floor is ${MIN_SITE_FILES_COMPARED}`)
  if (totals.tasks < MIN_SITE_TASKS_COMPARED) failures.push(`compared ${totals.tasks} collected tests and suites with sites, floor is ${MIN_SITE_TASKS_COMPARED}`)
  if (totals.files < MIN_FILES_COMPARED) failures.push(`compared ${totals.files} files, floor is ${MIN_FILES_COMPARED}`)
  if (totals.tests < MIN_TESTS_COMPARED) failures.push(`compared ${totals.tests} tests, floor is ${MIN_TESTS_COMPARED}`)
  if (totals.hooks < MIN_HOOK_SITES_COMPARED) failures.push(`compared ${totals.hooks} hook sites, floor is ${MIN_HOOK_SITES_COMPARED}`)
  for (const failure of failures) console.error(`FAIL: ${failure}`)
  const belowFloor = totals.files < MIN_FILES_COMPARED || totals.tests < MIN_TESTS_COMPARED || totals.hooks < MIN_HOOK_SITES_COMPARED
  if (belowFloor) {
    console.error('Files not compared, by bucket:')
    for (const [bucket, entries] of buckets) {
      for (const { dir, path, detail } of entries) console.error(`  ${bucket}: ${dir}/${path}${detail ? ` -- ${detail}` : ''}`)
    }
  }
  return failures.length === 0 ? 0 : 1
}

process.exitCode = main()
