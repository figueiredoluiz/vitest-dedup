#!/usr/bin/env node
// Git hooks verify immutable snapshots, never the caller's unstaged files.
import { execFileSync, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, realpathSync, readdirSync, symlinkSync, unlinkSync, mkdirSync, mkdtempSync, openSync, closeSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { availableParallelism, tmpdir } from 'node:os'
import { dirname, isAbsolute, join, posix, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const SELF = fileURLToPath(import.meta.url)
const ZERO = /^0+$/
const OID = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/
const GIT_ENV = ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR', 'GIT_PREFIX', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_CONFIG_PARAMETERS', 'GIT_CONFIG_COUNT']
// Node's default 1 MiB output limit ends a run with ENOBUFS on a large pure-move diff, so raise it.
const GIT_MAX_BUFFER = 256 * 1024 * 1024
// How many verifications may run at once, each with its own export directory and Cargo cache.
export const LANES = 4
const POLL_MS = 5_000
const QUEUE_MS = 30 * 60 * 1000

// Run logs older than this are removed when the next run starts.
const LOG_RETENTION_MS = 30 * 24 * 60 * 60 * 1000

/**
 * Build jobs and test threads for one lane: the host's parallelism split across the lanes.
 * @param {number} [cpus] Host parallelism; defaults to what Node reports.
 * @param {number} [lanes] Lanes that may run at once.
 * @returns {number} At least 1.
 */
export function laneShare(cpus = availableParallelism(), lanes = LANES) {
  return Math.max(1, Math.floor(cpus / lanes))
}

/**
 * Remove run log directories older than the retention window.
 * Only `<milliseconds>-<pid>` directories directly under `logRoot` are considered, so nothing a
 * run did not create is touched; a missing `logRoot` is not an error.
 * @param {string} logRoot The `delivery-check-logs` directory.
 * @param {number} [nowMs] Current time, for tests.
 * @returns {string[]} Names of the removed directories.
 */
export function pruneLogs(logRoot, nowMs = Date.now()) {
  let entries
  try { entries = readdirSync(logRoot, { withFileTypes: true }) }
  catch (error) { if (error.code === 'ENOENT') return []; throw error }
  const removed = []
  for (const entry of entries) {
    const match = /^(\d+)-\d+$/.exec(entry.name)
    if (!entry.isDirectory() || !match || nowMs - Number(match[1]) <= LOG_RETENTION_MS) continue
    rmSync(join(logRoot, entry.name), { recursive: true, force: true })
    removed.push(entry.name)
  }
  return removed
}

/** Block this synchronous runner for `ms`. */
export const pause = ms => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms)

/**
 * Run Git in the selected repository, capturing output and propagating failures.
 * @param {string} root Repository directory.
 * @param {string[]} args Git arguments, passed without a shell.
 * @param {NodeJS.ProcessEnv} [env] Environment for this Git operation.
 * @returns {string} Captured stdout with surrounding whitespace removed.
 */
const git = (root, args, env = process.env) => execFileSync('git', args, { cwd: root, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: GIT_MAX_BUFFER }).trim()

/** Capture the PR-wide diff, or the local change when no remote main exists. */
export function scopeDiff(root, { revision, staged = false, env = process.env } = {}) {
  /** Capture Git output without shell expansion or inherited terminal output. */
  function git(args) {
    return execFileSync('git', args, { cwd: root, env, encoding: 'utf8', stdio: ['pipe', 'pipe', 'pipe'], maxBuffer: GIT_MAX_BUFFER }).trimEnd()
  }
  let base
  try { base = git(['merge-base', revision ?? 'HEAD', 'origin/main']) }
  catch {
    try { base = git(['rev-parse', '--verify', revision ? `${revision}^` : 'HEAD']) }
    catch { base = execFileSync('git', ['hash-object', '-t', 'tree', '--stdin'], { cwd: root, env, input: '', encoding: 'utf8' }).trim() }
  }
  return git(['-c', 'core.quotePath=false', 'diff', '--no-ext-diff', '--no-color', '--no-renames', '--unified=0', ...(staged ? ['--cached'] : []), base, ...(revision ? [revision] : [])])
}

/**
 * Copy the environment without Git repository paths or command-scoped config.
 * Ordinary variables, including caller build flags, remain available to checks.
 * @param {NodeJS.ProcessEnv} env Caller environment; never modified.
 * @returns {NodeJS.ProcessEnv} Environment for isolated verification subprocesses.
 */
export function cleanEnvironment(env) {
  const result = { ...env }
  for (const key of GIT_ENV) delete result[key]
  for (const key of Object.keys(result)) if (/^GIT_CONFIG_(KEY|VALUE)_[0-9]+$/.test(key)) delete result[key]
  return result
}

/**
 * Read pre-push ref records and deduplicate non-deletion local commit IDs.
 * @param {string} input Git's four-field, newline-separated hook input.
 * @returns {string[]} Unique local object IDs in their first-seen order.
 * @throws {Error} If a record or either object ID is malformed.
 */
export function pushCommits(input) {
  return pushUpdates(input).map(update => update.revision)
}

/** Group every destination of a proposed commit so no remote baseline is discarded. */
function pushUpdates(input) {
  const updates = new Map()
  for (const line of input.trim().split('\n').filter(Boolean)) {
    const fields = line.trim().split(/\s+/)
    if (fields.length !== 4 || !OID.test(fields[1]) || !OID.test(fields[3])) throw new Error('Malformed pre-push ref input')
    const [, revision, remoteRef, remote] = fields
    if (ZERO.test(revision)) continue
    const update = updates.get(revision) ?? { revision, remotes: new Set(), branches: true }
    update.remotes.add(remote)
    update.branches &&= remoteRef.startsWith('refs/heads/')
    updates.set(revision, update)
  }
  return [...updates.values()]
}

/**
 * Read staged paths or the union of each destination's outgoing delta, preserving filename bytes.
 * A destination whose remote head is not available locally is diffed from the proposed commit's
 * merge base with `origin/main` instead: main is the last verified state, so that diff holds every
 * path whose checks the push could affect. Non-branch refs, and a push with no readable baseline at
 * all, return null paths, requiring the full profile.
 * @param {string} root Source repository.
 * @param {object} [options] Optional grouped push update and Git environment.
 * @returns {{paths: string[]|null, reason: string}} Changed repository-relative paths, or null for
 *   unknown scope, and how the baseline was chosen.
 */
export function changedPaths(root, { update, env = process.env } = {}) {
  /** Read NUL-delimited names without trimming whitespace from a filename. */
  const names = args => execFileSync('git', ['diff', '--no-ext-diff', '--no-renames', '--name-only', '-z', ...args, '--'], { cwd: root, env, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: GIT_MAX_BUFFER }).split('\0').filter(Boolean)
  if (!update) return { paths: names(['--cached']), reason: 'staged index' }
  if (!update.branches) return { paths: null, reason: 'non-branch destination' }
  const reasons = new Set()
  try {
    const paths = new Set()
    for (const remote of update.remotes) {
      let base
      if (ZERO.test(remote)) {
        base = git(root, ['merge-base', update.revision, 'origin/main'], env)
        reasons.add('new branch: merge base with origin/main')
      } else {
        try {
          base = git(root, ['rev-parse', '--verify', `${remote}^{commit}`], env)
          reasons.add('remote head')
        } catch {
          base = git(root, ['merge-base', update.revision, 'origin/main'], env)
          reasons.add('remote head unavailable: merge base with origin/main')
        }
      }
      for (const path of names([base, update.revision])) paths.add(path)
    }
    return { paths: [...paths], reason: [...reasons].join('; ') }
  } catch { return { paths: null, reason: 'no readable baseline' } }
}

/** Classify each path by the surface it can affect; an unknown path keeps full verification. */
export function hookScope(paths) {
  if (paths === null) return ['full']
  const scopes = new Set()
  for (const path of paths) {
    if (/^(?:README|CONTRIBUTING|CHANGELOG|SECURITY)\.md$/.test(path) || path === 'LICENSE' || /^(?:docs|skills)\/.+\.md$/.test(path)) scopes.add('docs')
    else if (path.startsWith('.github/actions/vitest-dedup/') || path === 'action.yml') scopes.add('action')
    else if (path.startsWith('.github/') || path === 'codecov.yml') scopes.add('ci')
    else if (path.startsWith('npm/') || path.startsWith('scripts/release/')) scopes.add('npm')
    else if (path.startsWith('scripts/dev/') || /^\.githooks\/pre-(?:commit|push)$/.test(path)) scopes.add('tooling')
    else if (path.startsWith('scripts/check/') || path.startsWith('scripts/vitest-harness/')) scopes.add('gates')
    else if (path.startsWith('fixtures/')) scopes.add('fixtures')
    // The npm manifests define every npm gate and pin the Vitest the oracle runs; no Rust test reads them.
    else if (path === 'package.json' || path === 'package-lock.json') scopes.add('node')
    // Rust integration tests read `tests/fixtures/`, and nothing else does.
    else if (/^(?:src|tests|examples|benches)\/.+\.rs$/.test(path) || /^Cargo\.(?:toml|lock)$/.test(path) || path.startsWith('tests/fixtures/')) scopes.add('rust')
    else return ['full']
  }
  return [...scopes].sort()
}

/** The gates that run the analyzer over the owned corpora, with the binary they read. */
const CORPUS_GATES = [
  ['cargo', 'build', '--release', '--locked'],
  ...['check:corpus', 'check:recall', 'verify:titles', 'check:title-properties', 'check:oracle', 'check:behavior-runtime', 'check:hooks-runtime', 'check:hooks-differential', 'check:parameterization-runtime'].map(gate => ['npm', 'run', gate]),
]

const RUST_TESTS = ['cargo', 'test', '--all-targets', '--all-features', '--locked']
const RUST_COVERAGE = ['npm', 'run', 'check:coverage:rust']

/**
 * Select local hook checks by what the change can affect, keeping CI whole.
 * Pre-commit stays compilation-free. Pre-push adds the compiled checks of a surface: Rust gets
 * lints, tests, the corpus gates and its coverage, and skips what packages or wraps the binary.
 * Consumers decide this, not file types: Rust tests read most of `fixtures/`, so a fixture runs
 * them, and `npm/test` reads the workflows, so a workflow runs that. The Rust suite runs once: the
 * coverage gate runs every test target, so when it is selected it takes the suite's place.
 * A manifest change runs the whole npm chain, which holds every npm gate and builds the release
 * binary for the corpus gates, but no Rust lint or test.
 */
export function hookCommands(mode, paths, docDiff) {
  const scopes = hookScope(paths)
  if (scopes.includes('full')) return commands(mode === 'pre-commit' ? 'fast' : 'full', mode === 'pre-push' || rustChanged(paths ?? []), docDiff)
  const has = scope => scopes.includes(scope)
  const push = mode === 'pre-push'
  const chain = push && has('node')
  const steps = staticCommands(docDiff)
  if (has('rust')) steps.push(['cargo', 'fmt', '--all', '--check'])
  if (chain) steps.push(['npm', 'run', 'check'])
  else {
    if (has('action')) steps.push(['npm', 'run', 'test:action'])
    if (has('npm') || has('ci') || has('node')) steps.push(['npm', 'test'])
    if (has('npm')) steps.push(['npm', 'run', 'pack:check'])
    if (['ci', 'tooling', 'gates', 'fixtures', 'node'].some(has)) steps.push(['npm', 'run', 'test:dev-checks'])
    // `test:check-lib` also holds `scripts/release/release-comparison.test.mjs`, which nothing else runs.
    if (['npm', 'tooling', 'gates', 'fixtures', 'node'].some(has)) steps.push(['npm', 'run', 'test:check-lib'])
  }
  if (!push) return steps
  if (has('rust')) {
    steps.push(
      ['cargo', 'clippy', '--all-targets', '--all-features', '--locked'],
      ['cargo', 'doc', '--no-deps', '--document-private-items', '--locked'],
    )
  }
  const coverage = rustChanged(paths)
  const suite = has('rust') || has('fixtures')
  if (suite) steps.push(coverage ? RUST_COVERAGE : RUST_TESTS)
  if (!chain) {
    if (['rust', 'gates', 'fixtures'].some(has)) steps.push(...CORPUS_GATES)
    if (['action', 'npm', 'tooling', 'gates', 'fixtures', 'rust'].some(has)) steps.push(['npm', 'run', 'check:duplication'])
    if (['action', 'npm', 'tooling', 'gates', 'fixtures'].some(has)) steps.push(['npm', 'run', 'check:coverage:js'])
  }
  if (coverage && !suite) steps.push(RUST_COVERAGE)
  return steps
}

/**
 * Decide whether changed paths require Rust checks in the fast profile.
 * Callers include deleted paths and both sides of renames.
 * @param {string[]} paths Repository-relative paths from a NUL-delimited Git diff.
 * @returns {boolean} Whether Rust sources, Cargo metadata, or the coverage gate changed.
 */
export function rustChanged(paths) {
  return paths.some(path => path.endsWith('.rs') || /(^|\/)Cargo\.(toml|lock)$/.test(path) || path === 'scripts/check/check-coverage.mjs')
}

const FORMAT = ['cargo', 'fmt', '--all', '--check']
const CLIPPY = ['cargo', 'clippy', '--all-targets', '--all-features', '--locked']
const RUSTDOC = ['cargo', 'doc', '--no-deps', '--document-private-items', '--locked']
const WINDOWS_CHECK = ['cargo', 'check', '--all-targets', '--all-features', '--locked', '--target', 'x86_64-pc-windows-gnu']
const CARGO_TESTS = ['node', 'scripts/dev/cargo-tests.mjs']
const CLI_PROCESS_TESTS = ['cli_process', 'imported_delivery']
const RUST_INVENTORY = ['node', 'scripts/check/check-coverage.mjs', '--rust-inventory']
const EXCLUSION_PINS = 'tests/fixtures/owned-exclusions.json'
const OWNED_EXCLUSIONS = [...CARGO_TESTS, 'test', 'excluded_files', '--', 'the_analyzer_reads_the_owned_fixtures', '--exact']
const RECALL = 'scripts/check/check-recall.mjs'
// The compilation-free JavaScript suites, all of which an unknown scope runs.
const JS_TESTS = [['npm', 'run', 'test:action'], ['npm', 'test'], ['npm', 'run', 'pack:check'], ['npm', 'run', 'test:dev-checks'], ['npm', 'run', 'test:check-lib']]
// What a changed Rust line names when its behaviour can differ on Windows.
const WINDOWS_SENSITIVE = /\bcfg(?:_attr)?\b|\bfs::|\bPath(?:Buf)?\b|canonicali[sz]e|symlink|MAIN_SEPARATOR|\bjunction\b/

/**
 * Decide whether a change needs the Windows cross-check: a Cargo manifest, a changed Rust line that
 * names `cfg`, filesystem calls or paths, or a scope that is not known.
 * @param {string[]|null} paths Changed paths, or null when the scope is unknown.
 * @param {string} diff Unified diff of the change.
 * @returns {boolean} Whether to run the cross-check.
 */
export function windowsSensitive(paths, diff) {
  if (paths === null || paths.some(path => /^Cargo\.(?:toml|lock)$/.test(path))) return true
  let rust = false
  for (const line of diff.split('\n')) {
    const file = /^diff --git a\/.* b\/(.*)$/.exec(line)
    if (file) rust = file[1].endsWith('.rs')
    else if (rust && /^[+-](?![+-]{2} )/.test(line) && WINDOWS_SENSITIVE.test(line)) return true
  }
  return false
}

/**
 * Select the Rust tests of what changed: the unit tests of each changed library module, the binary's
 * unit and subprocess tests for its own modules, and the changed integration targets.
 * @param {string[]|null} paths Changed paths, or null when the scope is unknown.
 * @param {ReturnType<typeof crateLayout>} layout Which root owns each source file, and the
 *   integration target names.
 * @param {boolean} [wholeLibrary] Run the whole library suite beside the other selections, as an
 *   unknown path does.
 * @returns {string[][]} `cargo-tests.mjs` invocations.
 */
export function quickRustTests(paths, { bin, lib, testTargets }, wholeLibrary = false) {
  if (paths === null) return [[...CARGO_TESTS, 'lib']]
  const modules = new Set()
  const targets = new Set()
  let binary = false
  for (const path of paths) {
    const test = /^tests\/([^/]+?)(?:\.rs|\/.+)$/.exec(path)?.[1]
    // Ownership over-approximates, so every module and root that may hold a file runs its tests.
    const owners = lib.filter(entry => owns(entry.paths, path)).map(entry => entry.module)
    const inBinary = owns(bin, path)
    if (/^Cargo\.(?:toml|lock)$/.test(path) || path === 'src/lib.rs' || owners.includes('testing')) wholeLibrary = true
    else if (inBinary || owners.length) {
      binary ||= inBinary
      for (const owner of owners) modules.add(owner)
    }
    // A source file neither root declares is reached some other way; the library suite runs.
    else if (path.startsWith('src/') && path.endsWith('.rs')) wholeLibrary = true
    else if (test && testTargets.includes(test)) targets.add(test)
    // `tests/support/` and `tests/fixtures/` are read by integration targets this cannot name.
    else if (test) for (const target of testTargets) targets.add(target)
  }
  const steps = []
  if (wholeLibrary) steps.push([...CARGO_TESTS, 'lib'])
  else if (modules.size) steps.push([...CARGO_TESTS, 'lib', ...[...modules].sort()])
  if (binary) {
    steps.push([...CARGO_TESTS, 'bin'])
    for (const target of CLI_PROCESS_TESTS) if (testTargets.includes(target)) targets.add(target)
  }
  if (targets.size) steps.push([...CARGO_TESTS, 'test', ...[...targets].sort()])
  return steps
}

/**
 * Build the pre-review lane: what a review or the push hook would otherwise find first, in minutes.
 * Static checks, format, the compilation-free JavaScript suites, duplication, JavaScript coverage,
 * Clippy, rustdoc, the Windows cross-check when the change can differ there, and the Rust tests of
 * what changed. Fixture inputs also run the owned-exclusion assertion or recall with the debug
 * binary. The push hook still runs the release corpus gates, Rust coverage and the full suite.
 * @param {string[]|null} paths Changed paths, or null when the scope is unknown.
 * @param {string} diff Unified diff of the change.
 * @param {string} [docDiff] File holding that diff, for the docstring check.
 * @param {ReturnType<typeof crateLayout>} [layout] Crate layout for test selection.
 * @returns {string[][]} Commands in execution order.
 */
export function quickCommands(paths, diff, docDiff, layout = { bin: [], lib: [], testTargets: [] }) {
  const scopes = hookScope(paths)
  const full = scopes.includes('full')
  const rust = full || scopes.includes('rust')
  const steps = full ? [...staticCommands(docDiff), FORMAT, ...JS_TESTS] : hookCommands('pre-commit', paths, docDiff)
  if (full || paths.some(path => path === RUST_INVENTORY[1] || (path.startsWith('src/') && path.endsWith('.rs')))) steps.unshift(RUST_INVENTORY)
  if (full || scopes.some(scope => scope !== 'docs')) steps.push(['npm', 'run', 'check:duplication'])
  // JavaScript coverage runs wherever the push hook runs it, on its own or inside `npm run check`;
  // Rust coverage never runs here.
  const push = hookCommands('pre-push', paths, docDiff).map(step => step.join(' '))
  if (push.includes('npm run check:coverage:js') || push.includes('npm run check')) steps.push(['npm', 'run', 'check:coverage:js'])
  const rustTests = rust ? quickRustTests(paths?.filter(path => path !== EXCLUSION_PINS) ?? null, layout, full) : []
  if (rust) {
    steps.push(CLIPPY, RUSTDOC)
    if (windowsSensitive(full ? null : paths, diff)) steps.push(WINDOWS_CHECK)
    // An unknown path adds the whole library suite and keeps what the known paths select.
    steps.push(...rustTests)
  }
  const owned = full || scopes.includes('fixtures') || paths.includes(EXCLUSION_PINS)
  if (owned && !rustTests.some(([, , kind, ...targets]) => kind === 'test' && targets.includes('excluded_files'))) steps.push(OWNED_EXCLUSIONS)
  if (full || paths.some(path => path.startsWith('fixtures/recall/') || path === RECALL || path === 'scripts/check/next-duplicates-known.json')) {
    // Integration tests build the standalone binary, as does the binary-test wrapper.
    if (!owned && !rustTests.some(([, , kind]) => kind === 'bin' || kind === 'test')) steps.push(['cargo', 'build', '--locked'])
    steps.push(['node', RECALL, join('target', 'debug', process.platform === 'win32' ? 'vitest-dedup.exe' : 'vitest-dedup')])
  }
  return steps
}

/**
 * Read the working checkout's change against its merge base with `origin/main`: tracked edits,
 * committed or not, plus untracked files that are not ignored.
 * @param {string} root Checkout directory.
 * @param {NodeJS.ProcessEnv} [env] Git environment.
 * @returns {{paths: string[]|null, diff: string, reason: string}} Null paths when no base is readable.
 */
export function quickChange(root, env = process.env) {
  /** Run Git, returning stdout even when it exits 1, as `diff --no-index` does on a difference. */
  const run = args => {
    const result = spawnSync('git', args, { cwd: root, env, encoding: 'utf8', maxBuffer: GIT_MAX_BUFFER })
    if (result.error || (result.status !== 0 && result.status !== 1)) throw new Error(`git ${args.join(' ')} failed`)
    return result.stdout
  }
  const untracked = run(['ls-files', '--others', '--exclude-standard', '-z']).split('\0').filter(Boolean)
  const added = untracked.map(path => run(['-c', 'core.quotePath=false', 'diff', '--no-index', '--no-ext-diff', '--no-color', '--unified=0', '--', '/dev/null', path])).join('')
  // A nonempty diff must start at a file header, so an empty fragment adds no separator.
  const diff = [scopeDiff(root, { env }), added].filter(Boolean).join('\n')
  let base
  try { base = git(root, ['merge-base', 'HEAD', 'origin/main'], env) }
  catch { return { paths: null, diff, reason: 'no merge base with origin/main' } }
  const tracked = execFileSync('git', ['diff', '--no-ext-diff', '--no-renames', '--name-only', '-z', base, '--'], { cwd: root, env, encoding: 'utf8', maxBuffer: GIT_MAX_BUFFER }).split('\0').filter(Boolean)
  return { paths: [...new Set([...tracked, ...untracked])], diff, reason: 'working tree against the merge base with origin/main' }
}

/**
 * Read the file modules a crate root declares, honouring `#[path]`: `mod x;` owns `src/x.rs` and
 * everything under `src/x/`, and `#[path = "d/p.rs"] mod x;` owns `src/d/p.rs` and everything under
 * `src/d/`, where Rust resolves that file's own child modules.
 * @param {string} source The crate root's text.
 * @returns {{module: string, paths: string[]}[]} Owned paths; one ending in `/` is a directory.
 */
export function rootModules(source) {
  // Comments are whitespace to Rust, so an attribute applies across them; strip them first.
  const code = source.replace(/\/\*[\s\S]*?\*\/|\/\/[^\n]*/g, ' ')
  // The attributes directly before a declaration, on its line or earlier ones, are its own.
  return [...code.matchAll(/((?:#\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;/g)].map(([, attributes, module]) => {
    const path = /#\[\s*path\s*=\s*"([^"]+)"\s*\]/.exec(attributes)?.[1]
    // Git reports normalized forward-slash paths, so `./a.rs` and `a\\b.rs` must match them.
    if (!path) return { module, paths: [`src/${module}.rs`, `src/${module}/`] }
    const file = posix.normalize(`src/${path.replaceAll('\\', '/')}`)
    return { module, paths: [file, `${posix.dirname(file)}/`] }
  })
}

/**
 * Read the crate layout test selection needs from the checkout itself.
 * @param {string} root Checkout directory.
 * @returns {{bin: string[], lib: {module: string, paths: string[]}[], testTargets: string[]}} Paths
 *   the binary owns, the library's file modules, and the integration target names.
 */
export function crateLayout(root) {
  /** Read a crate root's declarations, or none when it is absent. */
  const declared = name => existsSync(join(root, 'src', name)) ? rootModules(readFileSync(join(root, 'src', name), 'utf8')) : []
  const tests = join(root, 'tests')
  // Cargo discovers `tests/<name>.rs` and `tests/<name>/main.rs`; a directory without `main.rs` is a helper.
  const testTargets = existsSync(tests) ? readdirSync(tests, { withFileTypes: true }).flatMap(entry => entry.isDirectory()
    ? (existsSync(join(tests, entry.name, 'main.rs')) ? [entry.name] : [])
    : (/^(.+)\.rs$/.exec(entry.name)?.[1] ?? [])) : []
  return { bin: ['src/main.rs', ...declared('main.rs').flatMap(module => module.paths)], lib: declared('lib.rs'), testTargets }
}

/** Whether `path` is one of `owned`, or under an owned directory. */
const owns = (owned, path) => owned.some(entry => entry.endsWith('/') ? path.startsWith(entry) : path === entry)

/** Build the inexpensive checks shared by every local profile and scope. */
function staticCommands(docDiff) {
  return [
    ['node', 'scripts/check/check-docstrings.mjs', ...(docDiff ? ['--diff-file', docDiff] : [])],
    ['npm', 'run', 'check:action-pins'],
    ['npm', 'run', 'check:release-workflow'],
    ['npm', 'run', 'check:versions'],
    ['npm', 'run', 'check:docs'],
    ['npm', 'run', 'check:names'],
    ['npm', 'run', 'skills:check'],
  ]
}

/**
 * Build an ordered check list without running commands or modifying files.
 *
 * Cheapest first, in both profiles: [`runSteps`] stops at the first failing step, so a failure a
 * static gate can see must surface in seconds, never after a compile. The static npm gates also
 * lead the full profile and then run again inside `npm run check` -- idempotent and seconds each
 * -- because the alternative is re-listing the check chain's tail here, and a second
 * hand-maintained list is how the chain and this profile would drift apart.
 *
 * The fast profile is commit-time and compilation-free: `cargo fmt --check` parses without
 * building, and Clippy, the Rust suite, the npm gate chain and the coverage ratchet run once, at
 * push, in the full profile. A commit that needs them finds that out in seconds instead of after
 * a cold dependency-tree build in the hook's own cargo cache.
 * Full checks always run the Rust suite once: through the coverage gate when the rust flag selects
 * coverage, which runs every test target, and as plain `cargo test` otherwise.
 * @param {'fast'|'full'} profile Verification depth.
 * @param {boolean} [rust=true] Include conditional Rust checks or coverage.
 * @param {string} [docDiff] Original Git diff for an exported hook snapshot.
 * @returns {string[][]} Commands represented as executable followed by arguments.
 * @throws {Error} If the profile is unsupported.
 */
export function commands(profile, rust = true, docDiff) {
  if (!['fast', 'full'].includes(profile)) throw new Error(`Unknown verification profile: ${profile}`)
  const steps = staticCommands(docDiff)
  if (rust || profile === 'full') steps.push(['cargo', 'fmt', '--all', '--check'])
  if (profile === 'fast') {
    steps.push(['npm', 'run', 'test:dev-checks'], ['npm', 'run', 'test:check-lib'])
    return steps
  }
  steps.push(
    ['cargo', 'clippy', '--all-targets', '--all-features', '--locked'],
    ['cargo', 'doc', '--no-deps', '--document-private-items', '--locked'],
    rust ? RUST_COVERAGE : RUST_TESTS,
    ['npm', 'run', 'check'],
  )
  return steps
}

/**
 * Run checks sequentially with isolated Git variables and warnings denied.
 * Save complete output per step; stop on the first failure with bounded diagnostics.
 * `summary.json` beside the logs records each step's command, seconds and exit status, rewritten
 * after every step so a failed or interrupted run keeps what it measured.
 * @param {string} root Directory where checks run.
 * @param {string[][]} steps Executables and arguments in execution order.
 * @param {object} options Environment, required logRoot, optional run metadata for the summary,
 *   and optional spawn/print/clock adapters.
 * @returns {void}
 * @throws {Error} If a command fails to start, exits unsuccessfully, or is signalled.
 */
export function runSteps(root, steps, { env = process.env, logRoot, meta = {}, spawn = spawnSync, print = console.log, now = Date.now } = {}) {
  const runEnv = cleanEnvironment(env)
  // CI denies warnings for the compiler and for rustdoc, whose broken intra-doc links it rejects.
  for (const name of ['RUSTFLAGS', 'RUSTDOCFLAGS']) {
    const flags = (runEnv[name] || '').trim()
    runEnv[name] = /(?:^|\s)-D\s*warnings$/.test(flags) ? flags : `${flags} -D warnings`.trim()
  }
  mkdirSync(logRoot, { recursive: true })
  const summary = { ...meta, steps: [] }
  const save = () => writeFileSync(join(logRoot, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`)
  save()
  for (const [i, [command, ...args]] of steps.entries()) {
    const log = join(logRoot, `${i + 1}-${command}.log`)
    const fd = openSync(log, 'w')
    const started = now()
    let result
    try { result = spawn(command, args, { cwd: root, env: runEnv, stdio: ['ignore', fd, fd], shell: process.platform === 'win32' && command === 'npm' }) }
    finally { closeSync(fd) }
    summary.steps.push({ command: [command, ...args].join(' '), seconds: (now() - started) / 1000, status: result.error ? `error: ${result.error.message}` : (result.status ?? result.signal) })
    save()
    if (result.error || result.status !== 0) {
      print(readFileSync(log, 'utf8').split(/\r?\n/).slice(-100).join('\n'))
      throw new Error(`${command} ${args.join(' ')} failed${result.error ? `: ${result.error.message}` : ` (exit ${result.status ?? result.signal})`}. Log: ${log}`)
    }
    if (command === RUST_INVENTORY[0] && args[0] === RUST_INVENTORY[1] && args[1] === RUST_INVENTORY[2]) {
      const advisory = readFileSync(log, 'utf8').trim()
      if (advisory) print(advisory)
    }
    print(`PASS ${command} ${args.join(' ')}`)
  }
}

/**
 * Validate exported symlinks before a gate can follow them outside the snapshot.
 * Internal links/chains are allowed; directory links are resolved without walking them.
 * @param {string} snapshot Exported source directory, before runner cache aliases exist.
 * @returns {void}
 * @throws {Error} If a link escapes the export, is dangling, or cannot resolve a cycle.
 */
function validateSnapshotLinks(snapshot) {
  const base = realpathSync(snapshot)
  const directories = [snapshot]
  while (directories.length) {
    const directory = directories.pop()
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isSymbolicLink()) {
        let target
        try { target = realpathSync(path) }
        catch { throw new Error(`Snapshot symlink cannot be resolved: ${JSON.stringify(relative(snapshot, path))}`) }
        const outside = relative(base, target)
        if (outside === '..' || outside.startsWith(`..${sep}`) || isAbsolute(outside)) throw new Error(`Snapshot symlink escapes the exported tree: ${JSON.stringify(relative(snapshot, path))}`)
      } else if (entry.isDirectory()) directories.push(path)
    }
  }
}

/**
 * Derive a short stable name for a repository from its common Git directory.
 * Worktrees of one repository share the directory, and so the name; separate clones do not.
 * @param {string} common Common Git directory of the repository.
 * @returns {string} Twelve hex characters.
 */
export function repositoryKey(common) {
  return createHash('sha256').update(realpathSync(common)).digest('hex').slice(0, 12)
}

/**
 * Export the index or a commit, validate links/whitespace, then check synchronously.
 * Include skip-worktree entries without changing the source index or sparse selection.
 * The export directory and commit index are removed on success or failure; a fixed directory is
 * also cleared of anything an interrupted run left before the export starts.
 * @template T
 * @param {string} root Source repository directory.
 * @param {string|null} revision Commit-ish, or null to export the staged index.
 * @param {(snapshot: string) => T} check Synchronous consumer of the exported source.
 * @param {NodeJS.ProcessEnv} [env] Environment used to access the source repository.
 * @param {string|null} [work] Fixed export directory, under a parent the caller owns; null
 *   creates a fresh temporary one.
 * @returns {T} The check's result; errors propagate after cleanup.
 */
export function withSnapshot(root, revision, check, env = process.env, work = null) {
  if (work) {
    // Private like `mkdtemp`'s directory: the export holds staged source other users cannot read.
    // The name is predictable, so the directory is created exclusively: one that appears between
    // the removal and the creation belongs to someone else and fails the run instead of receiving
    // the export.
    try {
      rmSync(work, { recursive: true, force: true })
      mkdirSync(work, { mode: 0o700 })
    } catch (error) {
      throw new Error(`Cannot prepare the hook export directory ${work}: ${error.message}. If another user owns it, point XDG_RUNTIME_DIR or TMPDIR at a private directory.`)
    }
  } else work = mkdtempSync(join(tmpdir(), 'vitest-dedup-hook-'))
  const snapshot = join(work, 'source')
  mkdirSync(snapshot)
  try {
    const exportEnv = { ...env }
    if (revision) {
      const commit = git(root, ['rev-parse', '--verify', `${revision}^{commit}`], env)
      exportEnv.GIT_INDEX_FILE = join(work, 'index')
      git(root, ['read-tree', commit], exportEnv)
    }
    git(root, ['checkout-index', '--all', '--ignore-skip-worktree-bits', `--prefix=${snapshot.replaceAll('\\', '/')}/`], exportEnv)
    validateSnapshotLinks(snapshot)
    const isolated = cleanEnvironment(env)
    git(snapshot, ['init', '--quiet'], isolated)
    git(snapshot, ['add', '--force', '--all'], isolated)
    git(snapshot, ['diff', '--cached', '--check'], isolated)
    return check(snapshot)
  } finally { rmSync(work, { recursive: true, force: true }) }
}

/**
 * Make the selected Cargo cache available at target for existing corpus gates.
 * Preserve real cache directories; remove only a temporary alias created here.
 * Reject nested caches and existing-target conflicts before creating directories.
 * @template T
 * @param {string} root Checkout or snapshot directory.
 * @param {string} cache Absolute Cargo cache path.
 * @param {() => T} body Synchronous checks that need the cache alias.
 * @returns {T} The body's result; errors propagate after alias cleanup.
 * @throws {Error} If the cache is nested under target or conflicts with an existing target.
 */
export function withTargetCache(root, cache, body) {
  const local = join(root, 'target')
  const nested = relative(local, cache)
  if (nested && nested !== '..' && !nested.startsWith(`..${sep}`) && !isAbsolute(nested)) throw new Error('CARGO_TARGET_DIR cannot be inside the checkout target directory; use target itself or a cache outside target')
  if (existsSync(local)) {
    if (!existsSync(cache) || realpathSync(local) !== realpathSync(cache)) throw new Error('Existing target directory conflicts with CARGO_TARGET_DIR; choose its matching cache before verification')
    return body()
  }
  mkdirSync(cache, { recursive: true })
  if (existsSync(local) && realpathSync(local) === realpathSync(cache)) return body()
  symlinkSync(cache, local, process.platform === 'win32' ? 'junction' : 'dir')
  try { return body() }
  finally { unlinkSync(local) }
}

/**
 * Make the checkout's installed dependencies visible in a snapshot for gates that spawn them.
 * A snapshot exports tracked files only and `node_modules` is never tracked, while the title
 * harness in `npm run check` spawns the pinned Vitest from it and fails by design when it is
 * absent. Link rather than copy; remove only the alias created here; never touch a real directory.
 * @template T
 * @param {string} snapshot Exported source directory.
 * @param {string} root Checkout whose `node_modules` is linked in, when it has one.
 * @param {() => T} body Synchronous checks that may spawn installed dependencies.
 * @returns {T} The body's result; errors propagate after alias cleanup.
 */
export function withNodeModules(snapshot, root, body) {
  const installed = join(root, 'node_modules')
  const local = join(snapshot, 'node_modules')
  if (!existsSync(installed) || existsSync(local)) return body()
  symlinkSync(installed, local, process.platform === 'win32' ? 'junction' : 'dir')
  try { return body() }
  finally { unlinkSync(local) }
}

/**
 * Hold one of the repository's verification lanes for `body`.
 * A lane is a lock directory in the common Git directory, and lane 0 keeps the historical name, so
 * a driver from an older commit still excludes it. When every lane is taken the run waits, because
 * a refused commit or push costs more than the queue. A killed process leaves its lane taken:
 * inspect the recorded PID and remove the directory only after confirming it exited.
 * @template T
 * @param {string} common Common Git directory shared by repository worktrees.
 * @param {(lane: number) => T} body Synchronous verification operation, given its lane.
 * @param {object} [options] Lane count, how long to wait, and sleep/print adapters.
 * @returns {T} The body's result.
 * @throws {Error} If every lane stays taken; body errors propagate after cleanup.
 */
export function withLock(common, body, { lanes = LANES, waitMs = 0, sleep = pause, print = () => {} } = {}) {
  const path = lane => join(common, lane === 0 ? 'delivery-check.lock' : `delivery-check.lock.${lane}`)
  /** Take the first free lane, or report that there is none. */
  const take = () => {
    for (let lane = 0; lane < lanes; lane++) {
      try { mkdirSync(path(lane)); return lane }
      catch (error) { if (error.code !== 'EEXIST') throw error }
    }
    return null
  }
  let lane = take()
  if (lane === null && waitMs > 0) print(`All ${lanes} verification lanes are busy; waiting.`)
  for (let waited = 0; lane === null && waited < waitMs; waited += POLL_MS) {
    sleep(POLL_MS)
    lane = take()
  }
  if (lane === null) throw new Error(`Repository verification is already running in every lane. Locks: ${Array.from({ length: lanes }, (_, index) => path(index)).join(', ')}. If a lock's recorded process and the checks it spawned have all exited, remove that stale lock and retry.`)
  try {
    writeFileSync(join(path(lane), 'owner'), `pid=${process.pid}\n`)
    return body(lane)
  } finally { rmSync(path(lane), { recursive: true, force: true }) }
}

/**
 * Install this checkout's hooks using worktree-specific Git configuration.
 * Refuse to replace a different hook manager or install missing hook files.
 * @param {string} root Repository checkout directory.
 * @returns {string} Absolute path to the installed hook directory.
 * @throws {Error} If hook configuration cannot be inspected or would be overwritten.
 */
export function installHooks(root) {
  const hooks = join(root, '.githooks')
  const current = spawnSync('git', ['config', '--get', 'core.hooksPath'], { cwd: root, encoding: 'utf8' })
  if (current.status !== 0 && current.status !== 1) throw new Error('Cannot inspect existing hooks configuration')
  if (current.stdout.trim() && resolve(root, current.stdout.trim()) !== hooks) throw new Error('Existing core.hooksPath was preserved; integrate these hooks with your existing hook manager instead')
  for (const name of ['pre-commit', 'pre-push']) if (!existsSync(join(hooks, name))) throw new Error(`Missing hook: ${name}`)
  git(root, ['config', '--local', 'extensions.worktreeConfig', 'true'])
  git(root, ['config', '--worktree', 'core.hooksPath', hooks])
  return hooks
}

/**
 * Dispatch manual checks, staged/commit snapshot verification, or hook installation.
 * Manual modes check the checkout in its cache; hook modes check isolated exports under one lock,
 * building in `hook-snapshot` beneath that cache so no artifact crosses between a snapshot and
 * the checkout. Deletion-only pushes run no checks. Check logs are retained in the common Git
 * directory.
 * @param {string[]} [args] Mode followed by optional pre-push remote arguments.
 * @param {object} [options] Caller environment, ref input, check/print adapters, and root.
 * @returns {void}
 * @throws {Error} If usage, ref input, snapshot/cache validation, or verification fails.
 */
export function main(args = process.argv.slice(2), { env = process.env, input, check = runSteps, print = console.log, root = process.cwd() } = {}) {
  const [mode, ...rest] = args
  if (!['quick', 'fast', 'full', 'pre-commit', 'pre-push', 'install'].includes(mode) || (mode !== 'pre-push' && rest.length)) throw new Error('Usage: verify.mjs quick|fast|full|pre-commit|pre-push|install')
  root = resolve(git(root, ['rev-parse', '--show-toplevel'], env))
  if (mode === 'install') { print(`Hooks installed for this worktree: ${installHooks(root)}`); return }
  const updates = mode === 'pre-push' ? pushUpdates(input ?? readFileSync(0, 'utf8')) : []
  if (mode === 'pre-push' && !updates.length) { print('No commits to verify (deletion or up-to-date push).'); return }
  const common = resolve(root, git(root, ['rev-parse', '--git-common-dir'], env))
  const cache = env.CARGO_TARGET_DIR ? resolve(root, env.CARGO_TARGET_DIR) : join(dirname(common), 'target')
  const logRoot = join(common, 'delivery-check-logs')
  pruneLogs(logRoot)
  const logs = join(logRoot, `${Date.now()}-${process.pid}`)
  // Each lane gets its share of the host, so lanes running at once do not oversubscribe it.
  const share = String(laneShare())
  const runEnv = { ...env, CARGO_BUILD_JOBS: env.CARGO_BUILD_JOBS || share, RUST_TEST_THREADS: env.RUST_TEST_THREADS || share, CARGO_TARGET_DIR: cache, CARGO_LLVM_COV_TARGET_DIR: env.CARGO_LLVM_COV_TARGET_DIR ? resolve(root, env.CARGO_LLVM_COV_TARGET_DIR) : join(cache, 'llvm-cov-target') }
  // A manual run builds in the checkout's own cache, which a second run must not share.
  const manual = ['quick', 'fast', 'full'].includes(mode)
  withLock(common, lane => {
    if (mode === 'quick') {
      const { paths, diff, reason } = quickChange(root, env)
      const scope = hookScope(paths)
      print(`Quick check scope: ${scope.join(', ') || 'no changed paths'} (${paths === null ? reason : `${paths.length} changed paths; ${reason}`})`)
      mkdirSync(logs, { recursive: true })
      const docDiff = join(logs, 'docstrings.diff')
      writeFileSync(docDiff, diff)
      const meta = { mode, scope, reason, changedPaths: paths?.length ?? null }
      withTargetCache(root, cache, () => check(root, quickCommands(paths, diff, docDiff, crateLayout(root)), { env: runEnv, logRoot: logs, meta, print }))
      return
    }
    if (mode === 'fast' || mode === 'full') {
      withTargetCache(root, cache, () => check(root, commands(mode), { env: runEnv, logRoot: logs, meta: { mode, scope: ['manual'], reason: 'manual profile' }, print }))
      return
    }
    const revisions = mode === 'pre-commit' ? [{ revision: null }] : updates
    // Snapshots never build in the checkout's cache: a test binary bakes its export's
    // `CARGO_MANIFEST_DIR`, and Cargo would hand it to the checkout once the export is gone. The
    // cache is keyed by repository so two clones sharing one external cache cannot trade binaries
    // either, and the caller's coverage and separate build directories are not honored: an
    // instrumented binary is the same artifact class, and `build.build-dir` is where the
    // fingerprints live, so a shared one defeats separate target directories. cargo-llvm-cov
    // forwards its own build-directory variable to Cargo the same way.
    //
    // Every snapshot of the repository is exported to one fixed directory. Cargo names artifacts
    // by their path relative to the workspace root and judges freshness by mtime, so a test binary
    // built in one export could be reused by a later export with older files, carrying the first
    // export's baked `CARGO_MANIFEST_DIR`; one path makes a reused artifact point at the current
    // export, and the lane's lock keeps two runs from sharing it. The directory sits where the
    // temporary one always did, outside the checkout: Cargo reads configuration and the discovery
    // walker reads ignore files from every ancestor, so an export under the checkout or the home
    // directory would see the caller's unstaged `.cargo/config.toml` or `~/.gitignore`. The
    // user's runtime directory is private where a shared `/tmp` is not.
    //
    // Each lane repeats all of that for itself: its own export path and cache, so two lanes share
    // no artifact. Lane 0 keeps the historical paths.
    const key = repositoryKey(common)
    const slot = lane === 0 ? key : `${key}-${lane}`
    const hookCache = join(cache, 'hook-snapshot', slot)
    const coverage = join(hookCache, 'llvm-cov-target')
    const hookEnv = { ...runEnv, CARGO_TARGET_DIR: hookCache, CARGO_BUILD_BUILD_DIR: hookCache, CARGO_LLVM_COV_TARGET_DIR: coverage, CARGO_LLVM_COV_BUILD_DIR: coverage }
    const work = join(env.XDG_RUNTIME_DIR || tmpdir(), `vitest-dedup-hook-${slot}`)
    for (const [i, update] of revisions.entries()) {
      const { revision } = update
      print(revision ? `Verifying proposed commit ${revision}` : 'Verifying the staged index')
      mkdirSync(logs, { recursive: true })
      const docDiff = join(logs, `docstrings-${i}.diff`)
      writeFileSync(docDiff, scopeDiff(root, { revision: revision ?? undefined, staged: !revision, env }))
      const { paths, reason } = changedPaths(root, { update: revision ? update : undefined, env })
      const scope = hookScope(paths)
      print(`Local check scope: ${scope.join(', ') || 'no changed paths'} (${paths === null ? reason : `${paths.length} changed paths; ${reason}`})`)
      const meta = { mode, revision, scope, reason, changedPaths: paths?.length ?? null }
      withSnapshot(root, revision, snapshot => {
        withNodeModules(snapshot, root, () => withTargetCache(snapshot, hookCache, () => check(snapshot, hookCommands(mode, paths, docDiff), { env: hookEnv, logRoot: join(logs, String(i)), meta, print })))
      }, env, work)
    }
  }, { lanes: manual ? 1 : LANES, waitMs: QUEUE_MS, print })
  print(`Verification complete. Logs: ${logs}`)
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(SELF)) {
  try { main() }
  catch (error) { console.error(error.message); process.exitCode = 1 }
}
