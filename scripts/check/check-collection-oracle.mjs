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
// Known disagreements are listed in `collection-oracle-known.json`, each with its reason. The gate
// fails on a disagreement that is not listed, and on a listed one that no longer occurs: a change
// that fixes one deletes its entry in the same commit.
//
// Not compared, and counted per bucket: files Vitest cannot collect (corpus code is deliberately
// incomplete; a fixture may use a global its config never declares), files whose collection read
// a stub of a missing module (`lib/collection-oracle-stubs.mjs`), files the analyzer excluded,
// files outside the analysis root, type tests and benchmarks. Agreement does not prove the
// analyzer right in general: Vitest reports one run in one environment.
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { isDeepStrictEqual } from 'node:util'
import { collectWithAnalyzer, collectWithVitest, compareDirectory, matchKnown, vitestInstallError } from './lib/collection-oracle.mjs'
import { defaultBinary } from './lib/run-analyzer.mjs'

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

// What the probe must report for the self-check directories, checked before anything is compared:
// hooks that do not register (`if (false)`, after `return`), one registered per loop iteration,
// `test.beforeEach`, a helper's hook at the helper's own line, and every way a file can read a stub
// (by name, whole, through a re-export of `then`, or by sharing one load under `isolate: false`).
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
    'stub-then.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#then'] },
    'stub-then-reexport.test.ts': { tests: ['always'], hooks: [], stubReads: ['./missing-module#then'] },
  },
  'scripts/check/collection-oracle-self-check-shared': {
    'first.test.ts': { tests: ['first'], hooks: [], stubReads: ['./missing-module#*'] },
    'second.test.ts': { tests: ['second'], hooks: [], stubReads: ['./missing-module#*'] },
  },
}

/** The probe's report on the self-check directories, as failure messages (none when it matches). */
function selfCheck() {
  return Object.entries(SELF_CHECK).flatMap(([dir, want]) => {
    const actual = Object.fromEntries(
      [...collectWithVitest(ROOT, dir)].map(([path, file]) => [path, { tests: file.tests, hooks: file.hooks, stubReads: file.stubReads }]),
    )
    return isDeepStrictEqual(actual, want)
      ? []
      : [`probe self-check ${dir}: expected ${JSON.stringify(want)}\n    got ${JSON.stringify(actual)}`]
  })
}

function main() {
  const binary = resolve(ROOT, process.argv[2] ?? defaultBinary({ root: ROOT }))
  const installError = vitestInstallError(ROOT)
  if (installError) {
    console.error(`FAIL: ${installError}. The oracle is the pinned Vitest; a missing one measures nothing.`)
    return 2
  }
  if (!existsSync(binary)) {
    console.error(`FAIL: analyzer binary ${binary} not found. Build it: cargo build --release --locked`)
    return 2
  }
  const manifest = JSON.parse(readFileSync(join(ROOT, 'fixtures', 'recall', 'manifest.json'), 'utf8'))
  const dirs = [...FIXTURE_DIRS, ...manifest.cases.map((c) => `fixtures/recall/${c.path}`)]
  const known = JSON.parse(readFileSync(KNOWN, 'utf8')).disagreements

  const totals = { files: 0, tests: 0, hooks: 0 }
  const buckets = new Map()
  const probeErrors = []
  const found = []
  for (const dir of dirs) {
    const result = compareDirectory(collectWithVitest(ROOT, dir), collectWithAnalyzer(binary, ROOT, dir))
    totals.files += result.compared.length
    totals.tests += result.testsCompared
    totals.hooks += result.hooksCompared
    for (const [bucket, entries] of result.skipped) {
      buckets.set(bucket, (buckets.get(bucket) ?? 0) + entries.length)
      if (bucket === 'probe-error') probeErrors.push(...entries.map((e) => `${dir}/${e.path}`))
    }
    found.push(...result.disagreements.map((d) => ({ dir, ...d })))
  }

  const selfCheckFailures = selfCheck()
  const { unlisted, stale } = matchKnown(found, known)

  console.log(
    `collection oracle: ${dirs.length} directories, ${totals.files} files compared, ` +
      `${totals.tests} tests, ${totals.hooks} hook sites; ${found.length} disagreements ` +
      `(${found.length - unlisted.length} known)`,
  )
  console.log(`  not compared: ${[...buckets].map(([b, n]) => `${b} ${n}`).join(', ')}`)

  const failures = [...selfCheckFailures]
  if (probeErrors.length > 0) failures.push(`probe errors (the probe and the collected suites disagree):\n    ${probeErrors.join('\n    ')}`)
  for (const d of unlisted) failures.push(`new disagreement: ${d.dir}/${d.path}: ${d.kind} ${JSON.stringify(d.item)}`)
  for (const d of stale) {
    failures.push(`listed disagreement no longer occurs (delete it from collection-oracle-known.json): ${d.dir}/${d.path}: ${d.kind} ${JSON.stringify(d.item)}`)
  }
  if (totals.files < MIN_FILES_COMPARED) failures.push(`compared ${totals.files} files, floor is ${MIN_FILES_COMPARED}`)
  if (totals.tests < MIN_TESTS_COMPARED) failures.push(`compared ${totals.tests} tests, floor is ${MIN_TESTS_COMPARED}`)
  if (totals.hooks < MIN_HOOK_SITES_COMPARED) failures.push(`compared ${totals.hooks} hook sites, floor is ${MIN_HOOK_SITES_COMPARED}`)
  for (const failure of failures) console.error(`FAIL: ${failure}`)
  return failures.length === 0 ? 0 : 1
}

process.exitCode = main()
