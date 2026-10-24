// What real Vitest collects versus what the analyzer extracts, for one directory.
//
// Vitest's side comes from `collection-probe-runner.mjs` under `vitest list --staticParse=false`;
// the analyzer's from `--emit-test-fingerprints`. Two things are compared:
//
//   - Test titles, per collected test file, as a multiset. A test an imported runner declares is
//     keyed by the analyzer's `registrationFile`, the test file that called the runner, which is
//     where Vitest collects it.
//   - Hook declaration sites (`kind@path:line`), per directory, as a set. Both sides name the site
//     that declared the hook, so a helper module's hook is its own line on both. A set, not a
//     multiset: the analyzer emits one record per declaration while Vitest registers once per
//     execution (a loop body twice, a `describe.each` callback once per row).
//
// A test or site on one side only is a disagreement: something collected that the analyzer never
// read, or something the analyzer recorded that Vitest never registered.
import { spawnSync } from 'node:child_process'
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { runAnalyzerJsonl } from './run-analyzer.mjs'

const LIB = dirname(fileURLToPath(import.meta.url))
const PROBE_RUNNER = join(LIB, 'collection-probe-runner.mjs')
const STUBS = join(LIB, 'collection-oracle-stubs.mjs')
const TEST_FILES = '**/*.{test,spec}.?(c|m)[jt]s?(x)'
// Benchmarks are collected by `vitest bench`, never by `vitest list`.
const BENCH_FILE = /\.bench\.[cm]?[jt]sx?$/
// Type tests are checked by `tsc` under `--typecheck`; no runner collects them.
const TYPE_TEST_FILE = /\.test-d\.[cm]?[jt]sx?$/

/** `\\` to `/`, so paths from both sides and every platform compare as strings. */
export const normalizePath = (path) => path.replaceAll('\\', '/')

/**
 * `kind@path:line`, the identity a hook site is compared by. Not the column: for
 * `test.beforeEach(...)` V8 reports the property's column and the analyzer the call's. Two hooks of
 * one kind on one line therefore count as one site.
 */
export const hookSite = ({ kind, path, line }) => `${kind}@${path}:${line}`

// The config files Vitest looks for, in its order (`configFiles` in Vitest's `constants.ts`).
const CONFIG_FILES = ['vitest.config', 'vite.config'].flatMap((name) =>
  ['.ts', '.mts', '.cts', '.js', '.mjs', '.cjs'].map((extension) => name + extension),
)

/** The config file Vitest would load from `absoluteDir`, or `null` when it has none. */
export function ownConfig(absoluteDir) {
  const name = CONFIG_FILES.find((file) => existsSync(join(absoluteDir, file)))
  return name ? join(absoluteDir, name) : null
}

/**
 * The config Vitest is run with. Without a config of its own, the directory gets Vitest's default
 * include. With one, that config is loaded unchanged and only the probe runner and the stub plugin
 * are added, to the root and to every inline `test.projects` entry: projects do not inherit the
 * root's settings, and a project left without the runner is caught by the completeness check in
 * `collectWithVitest`.
 */
export function configSource(absoluteDir, cacheDir) {
  const stubs = `import { missingModuleStubs } from ${JSON.stringify(pathToFileURL(STUBS).href)}\n`
  // No cache survives a run: a stub transformed while a module was missing must not be reused once
  // the module exists (`fsModuleCache`), and nothing is written into a shared `node_modules`.
  const probe =
    `{ cacheDir: ${JSON.stringify(cacheDir)}, plugins: [missingModuleStubs(${JSON.stringify(absoluteDir)})], ` +
    `test: { runner: ${JSON.stringify(PROBE_RUNNER)}, fsModuleCache: false } }`
  const own = ownConfig(absoluteDir)
  if (own === null) {
    const test = { root: absoluteDir, dir: absoluteDir, include: [TEST_FILES], exclude: ['**/node_modules/**', '**/target/**'] }
    return `${stubs}const probe = ${probe}\nexport default { ...probe, test: { ...probe.test, ...${JSON.stringify(test)} } }\n`
  }
  const vitestConfig = pathToFileURL(join(dirname(PROBE_RUNNER), '..', '..', '..', 'node_modules', 'vitest', 'dist', 'config.js')).href
  return `${stubs}import { mergeConfig } from ${JSON.stringify(vitestConfig)}
import own from ${JSON.stringify(pathToFileURL(own).href)}
const probe = ${probe}
const base = await (typeof own === 'function' ? own({ command: 'serve', mode: 'test' }) : own)
const withProbe = (config) => mergeConfig(config, probe)
const merged = withProbe(base)
const projects = base.test?.projects?.map((project) => (project && typeof project === 'object' && !('then' in project) ? withProbe(project) : project))
export default projects ? { ...merged, test: { ...merged.test, projects } } : merged
`
}

/**
 * The per-file view of what the probe wrote (`records`) and of what `vitest list --json` listed
 * (`listedFiles`), keyed by path relative to `absoluteDir`. A listed file with no probe record is
 * `unprobed`; a file with records from more than one project is `overlapping`.
 */
export function vitestView(records, listedFiles, absoluteDir) {
  const files = new Map()
  const local = (path) => normalizePath(relative(absoluteDir, path))
  for (const record of records) {
    const located = record.registrations.filter((r) => r.line !== null)
    const path = local(record.file)
    if (files.has(path)) {
      files.get(path).overlapping = true
      continue
    }
    files.set(path, {
      tests: record.tests.map((t) => t.title),
      hooks: located.map((r) => hookSite({ kind: r.kind, path: local(r.file), line: r.line })),
      suiteHookCount: Object.values(record.suiteHooks).reduce((sum, n) => sum + n, 0),
      registrationCount: record.registrations.length,
      unlocatedRegistrations: record.registrations.length - located.length,
      stubReads: record.stubReads,
      collectErrors: record.collectErrors,
      unprobed: false,
      overlapping: false,
    })
  }
  for (const path of new Set(listedFiles.map(local))) {
    if (!files.has(path)) {
      files.set(path, { tests: [], hooks: [], suiteHookCount: 0, registrationCount: 0, unlocatedRegistrations: 0, stubReads: [], collectErrors: [], unprobed: true, overlapping: false })
    }
  }
  return files
}

/**
 * Collects `dir` with real Vitest through the probe runner.
 *
 * @param {string} root - checkout root; `node_modules/vitest` is resolved from it.
 * @param {string} dir - directory to collect, relative to `root`.
 * @returns {Map<string, object>} per collected file, keyed by path relative to `dir`: `tests`
 *   (titles), `hooks` (`kind@path:line` sites), `suiteHookCount`, `registrationCount`,
 *   `unlocatedRegistrations`, `stubReads`, `collectErrors`, and `unprobed` for a file Vitest
 *   listed that the probe runner never saw.
 * @throws {Error} when Vitest exits non-zero having collected nothing.
 */
export function collectWithVitest(root, dir) {
  const work = mkdtempSync(join(tmpdir(), 'vitest-dedup-oracle-'))
  try {
    const out = join(work, 'collected.jsonl')
    const listed = join(work, 'listed.json')
    const config = join(work, 'vitest.config.mjs')
    const absoluteDir = resolve(root, dir)
    writeFileSync(config, configSource(absoluteDir, join(work, 'cache')))
    writeFileSync(out, '')
    const result = spawnSync(
      process.execPath,
      [join(root, 'node_modules', 'vitest', 'vitest.mjs'), 'list', '--staticParse=false',
        // `--allowOnly`: on CI Vitest refuses a file with `.only` instead of collecting it.
        '--config', config, '--root', absoluteDir, `--json=${listed}`, '--fsModuleCache=false', '--allowOnly'],
      {
        cwd: absoluteDir,
        encoding: 'utf8',
        env: { ...process.env, VITEST_DEDUP_ORACLE_OUT: out },
        maxBuffer: 256 << 20,
        timeout: 300_000,
      },
    )
    if (result.error) throw result.error
    const lines = readFileSync(out, 'utf8').split('\n').filter((line) => line.trim() !== '')
    const listedFiles = existsSync(listed) ? JSON.parse(readFileSync(listed, 'utf8')).map((t) => t.file) : []
    // Collecting nothing is a result (a config whose projects hold no test file); failing to is not.
    if (lines.length === 0 && listedFiles.length === 0 && result.status !== 0) {
      throw new Error(
        `vitest list collected nothing in ${dir} (exit ${result.status ?? result.signal}):\n` +
          `${(result.stderr ?? '').trim()}\n${(result.stdout ?? '').trim()}`.slice(-4000),
      )
    }
    return vitestView(lines.map((line) => JSON.parse(line)), listedFiles, absoluteDir)
  } finally {
    rmSync(work, { recursive: true, force: true })
  }
}

/**
 * The analyzer's records as tests and hook sites per declaring file, and its excluded files. Paths
 * are relative to `absoluteDir`, as Vitest's side writes them: the analyzer reports a file outside
 * its root by its absolute path.
 */
export function analyzerView(records, absoluteDir) {
  const local = (path) => normalizePath(absoluteDir && isAbsolute(path) ? relative(absoluteDir, path) : path)
  const tests = new Map()
  const hooks = new Map()
  const excluded = new Set()
  const push = (map, key, value) => {
    if (!map.has(key)) map.set(key, [])
    map.get(key).push(value)
  }
  for (const record of records) {
    const path = record.path === undefined ? undefined : local(record.path)
    // A test an imported runner declares belongs to the test file that registered it.
    if (record.type === 'test') push(tests, local(record.registrationFile ?? record.path), record.title)
    else if (record.type === 'hook') push(hooks, path, hookSite({ ...record, path }))
    else if (record.type === 'excluded-file') excluded.add(path)
  }
  return { tests, hooks, excluded }
}

/**
 * Extracts `dir` with the analyzer.
 *
 * @returns {{tests: Map<string, string[]>, hooks: Map<string, string[]>, excluded: Set<string>}}
 *   tests keyed by the file that registered them and hook sites by the file that declared them,
 *   both relative to `dir`; `excluded` holds
 *   every file with an `excluded-file` record, whose tests the analyzer makes no claim about.
 */
export function collectWithAnalyzer(binary, root, dir) {
  let records
  try {
    records = runAnalyzerJsonl(binary, [resolve(root, dir), '--emit-test-fingerprints']).records
  } catch (error) {
    // Exit 2 with this notice is the analyzer's answer for a directory of lookalikes (Jest, Mocha,
    // `node:test` files): it found test files and no Vitest test in them. Every other exit 2 stays
    // an operational failure.
    if (!/extracted no tests/.test(String(error.message))) throw error
    records = []
  }
  return analyzerView(records, resolve(root, dir))
}

/** Items of `a` not matched one-for-one in `b`, as a multiset difference. */
export function minus(a, b) {
  const counts = new Map()
  for (const item of b) counts.set(item, (counts.get(item) ?? 0) + 1)
  const left = []
  for (const item of a) {
    const n = counts.get(item) ?? 0
    if (n > 0) counts.set(item, n - 1)
    else left.push(item)
  }
  return left
}

/** Why a file Vitest collected is not compared, or `null` when it is. */
function skipReason(path, vitest, analyzerExcluded) {
  // The analyzer does not read outside its root and reports the run incomplete instead.
  if (path.startsWith('../')) return 'outside-root'
  // Collected by more than one project: one record per file cannot hold both collections.
  if (vitest.overlapping) return 'overlapping-projects'
  if (vitest.unprobed && TYPE_TEST_FILE.test(path)) return 'type-test'
  if (vitest.collectErrors.length > 0) return 'uncollectable'
  if (vitest.stubReads.length > 0) return 'stubbed'
  if (analyzerExcluded) return 'excluded'
  if (vitest.unprobed || vitest.registrationCount !== vitest.suiteHookCount || vitest.unlocatedRegistrations > 0) return 'probe-error'
  return null
}

/**
 * Compares one directory. Files Vitest could not collect, files whose collection read a stub of a
 * missing module, and files the analyzer excluded are counted, not compared: the first two have no
 * runtime truth, the last makes no claim. A probe that saw a different number of hook
 * registrations than the collected suites hold is a `probe-error`, which the gate fails on.
 *
 * @returns {{compared: string[], testsCompared: number, hooksCompared: number,
 *   skipped: Map<string, object[]>, disagreements: {path: string, kind: string, item: string}[]}}
 */
export function compareDirectory(vitestFiles, analyzer) {
  const skipped = new Map(
    ['uncollectable', 'stubbed', 'excluded', 'outside-root', 'overlapping-projects', 'type-test', 'bench', 'undetermined-hook', 'probe-error']
      .map((k) => [k, []]),
  )
  const compared = []
  // Files whose collection is no evidence of what registers: a stub decided it, it stopped at an
  // error, or the probe could not account for it. A file the analyzer excluded was collected cleanly.
  let untrusted = 0
  for (const [path, vitest] of [...vitestFiles].sort(([a], [b]) => a.localeCompare(b))) {
    const reason = skipReason(path, vitest, analyzer.excluded.has(path))
    if (reason === null) compared.push(path)
    else skipped.get(reason).push({ path, detail: vitest.collectErrors[0]?.split('\n')[0] ?? vitest.stubReads[0] })
    if (['uncollectable', 'stubbed', 'overlapping-projects', 'probe-error'].includes(reason)) untrusted += 1
  }
  const comparedSet = new Set(compared)
  // A module Vitest did not collect: a helper (whose hooks count) or a test file Vitest leaves out.
  const isModule = (path) => !vitestFiles.has(path)
  const disagreements = []

  let testsCompared = 0
  for (const path of compared) {
    const vitest = vitestFiles.get(path)
    const extracted = analyzer.tests.get(path) ?? []
    testsCompared += vitest.tests.length
    for (const item of minus(vitest.tests, extracted)) disagreements.push({ path, kind: 'missing-test', item })
    for (const item of minus(extracted, vitest.tests)) disagreements.push({ path, kind: 'extra-test', item })
  }
  // Tests registered by a file Vitest never collected: a test file outside Vitest's include, or one
  // a config leaves out. Benchmarks and type tests are collected by other commands.
  for (const [path, titles] of analyzer.tests) {
    if (!isModule(path)) continue
    if (BENCH_FILE.test(path)) skipped.get('bench').push({ path, detail: `${titles.length} benchmarks` })
    else if (TYPE_TEST_FILE.test(path)) skipped.get('type-test').push({ path, detail: `${titles.length} type tests` })
    else for (const item of titles) disagreements.push({ path, kind: 'extra-test', item })
  }

  const registered = new Set(compared.flatMap((path) => vitestFiles.get(path).hooks))
  // A helper's hook that only skipped files register is compared on neither side, like their tests.
  const registeredBySkipped = new Set(
    [...vitestFiles].filter(([path]) => !comparedSet.has(path)).flatMap(([, vitest]) => vitest.hooks),
  )
  // A helper's hook no compared file registers is an extra hook only when every file that could
  // reach the helper is trusted: which helpers an untrusted file reaches is not known, and whether it
  // registered one is no evidence either way.
  const helperSite = (path, site) => {
    if (registered.has(site) || registeredBySkipped.has(site)) return registered.has(site)
    if (untrusted === 0) return true
    skipped.get('undetermined-hook').push({ path, detail: site })
    return false
  }
  const recorded = new Set(
    [...analyzer.hooks]
      .filter(([path]) => comparedSet.has(path) || (isModule(path) && !BENCH_FILE.test(path) && !TYPE_TEST_FILE.test(path)))
      .flatMap(([path, sites]) => (comparedSet.has(path) ? sites : sites.filter((site) => helperSite(path, site)))),
  )
  const pathOf = (site) => site.slice(site.indexOf('@') + 1, site.lastIndexOf(':'))
  for (const site of registered) {
    if (!recorded.has(site)) disagreements.push({ path: pathOf(site), kind: 'missing-hook', item: site })
  }
  for (const site of recorded) {
    if (!registered.has(site)) disagreements.push({ path: pathOf(site), kind: 'extra-hook', item: site })
  }
  return { compared, testsCompared, hooksCompared: registered.size, skipped, disagreements }
}

/**
 * Matches observed disagreements against the known list one for one: a listed entry accounts for
 * one occurrence, so a second copy of a known disagreement is unlisted, and an entry no occurrence
 * matched is stale. Entries are identified by `dir`, `path`, `kind` and `item`.
 */
export function matchKnown(found, known) {
  const key = ({ dir, path, kind, item }) => JSON.stringify([dir, path, kind, item])
  const remaining = new Map()
  for (const entry of known) remaining.set(key(entry), [...(remaining.get(key(entry)) ?? []), entry])
  const unlisted = []
  for (const disagreement of found) {
    const entries = remaining.get(key(disagreement))
    if (entries?.length) entries.pop()
    else unlisted.push(disagreement)
  }
  return { unlisted, stale: [...remaining.values()].flat() }
}

/** Why `root` lacks the pinned Vitest the gate requires, or `null` when it has it. */
export function vitestInstallError(root) {
  const pkg = join(root, 'node_modules', 'vitest', 'package.json')
  if (!existsSync(pkg)) return `${pkg} not found: npm ci (vitest is a pinned devDependency)`
  const pinned = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).devDependencies?.vitest
  const installed = JSON.parse(readFileSync(pkg, 'utf8')).version
  if (installed !== pinned) return `installed Vitest is ${installed}, package.json pins ${pinned}: npm ci`
  return null
}
