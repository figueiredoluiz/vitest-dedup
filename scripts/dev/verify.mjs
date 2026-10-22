#!/usr/bin/env node
// Git hooks verify immutable snapshots, never the caller's unstaged files.
import { execFileSync, spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, realpathSync, readdirSync, symlinkSync, unlinkSync, mkdirSync, mkdtempSync, openSync, closeSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const SELF = fileURLToPath(import.meta.url)
const ZERO = /^0+$/
const OID = /^(?:[a-f0-9]{40}|[a-f0-9]{64})$/
const GIT_ENV = ['GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR', 'GIT_PREFIX', 'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES', 'GIT_CONFIG_PARAMETERS', 'GIT_CONFIG_COUNT']
// Node's default 1 MiB output limit ends a run with ENOBUFS on a large pure-move diff, so raise it.
const GIT_MAX_BUFFER = 256 * 1024 * 1024

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
  const commits = new Set()
  for (const line of input.trim().split('\n').filter(Boolean)) {
    const fields = line.trim().split(/\s+/)
    if (fields.length !== 4 || !OID.test(fields[1]) || !OID.test(fields[3])) throw new Error('Malformed pre-push ref input')
    if (!ZERO.test(fields[1])) commits.add(fields[1])
  }
  return [...commits]
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
 * Full checks always include Rust lints/tests; the rust flag selects coverage too.
 * @param {'fast'|'full'} profile Verification depth.
 * @param {boolean} [rust=true] Include conditional Rust checks or coverage.
 * @param {string} [docDiff] Original Git diff for an exported hook snapshot.
 * @returns {string[][]} Commands represented as executable followed by arguments.
 * @throws {Error} If the profile is unsupported.
 */
export function commands(profile, rust = true, docDiff) {
  if (!['fast', 'full'].includes(profile)) throw new Error(`Unknown verification profile: ${profile}`)
  const steps = [
    ['node', 'scripts/check/check-docstrings.mjs', ...(docDiff ? ['--diff-file', docDiff] : [])],
    ['npm', 'run', 'check:action-pins'],
    ['npm', 'run', 'check:release-workflow'],
    ['npm', 'run', 'check:versions'],
    ['npm', 'run', 'check:docs'],
    ['npm', 'run', 'check:names'],
    ['npm', 'run', 'skills:check'],
  ]
  if (rust || profile === 'full') steps.push(['cargo', 'fmt', '--all', '--check'])
  if (profile === 'fast') {
    steps.push(['npm', 'run', 'test:dev-checks'], ['npm', 'run', 'test:check-lib'])
    return steps
  }
  steps.push(
    ['cargo', 'clippy', '--all-targets', '--all-features', '--locked'],
    ['cargo', 'test', '--all-targets', '--all-features', '--locked'],
    ['npm', 'run', 'check'],
  )
  if (rust) steps.push(['npm', 'run', 'check:coverage:rust'])
  return steps
}

/**
 * Run checks sequentially with isolated Git variables and warnings denied.
 * Save complete output per step; stop on the first failure with bounded diagnostics.
 * @param {string} root Directory where checks run.
 * @param {string[][]} steps Executables and arguments in execution order.
 * @param {object} options Environment, required logRoot, and optional spawn/print adapters.
 * @returns {void}
 * @throws {Error} If a command fails to start, exits unsuccessfully, or is signalled.
 */
export function runSteps(root, steps, { env = process.env, logRoot, spawn = spawnSync, print = console.log } = {}) {
  const runEnv = cleanEnvironment(env)
  const flags = (runEnv.RUSTFLAGS || '').trim()
  runEnv.RUSTFLAGS = /(?:^|\s)-D\s*warnings$/.test(flags) ? flags : `${flags} -D warnings`.trim()
  mkdirSync(logRoot, { recursive: true })
  for (const [i, [command, ...args]] of steps.entries()) {
    const log = join(logRoot, `${i + 1}-${command}.log`)
    const fd = openSync(log, 'w')
    let result
    try { result = spawn(command, args, { cwd: root, env: runEnv, stdio: ['ignore', fd, fd], shell: process.platform === 'win32' && command === 'npm' }) }
    finally { closeSync(fd) }
    if (result.error || result.status !== 0) {
      print(readFileSync(log, 'utf8').split(/\r?\n/).slice(-100).join('\n'))
      throw new Error(`${command} ${args.join(' ')} failed${result.error ? `: ${result.error.message}` : ` (exit ${result.status ?? result.signal})`}. Log: ${log}`)
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
 * Serialize verification using a lock directory in the common Git directory.
 * Record the owner PID and remove an acquired lock when the synchronous body finishes.
 * Stale locks require manual recovery after confirming the recorded process exited.
 * @template T
 * @param {string} common Common Git directory shared by repository worktrees.
 * @param {() => T} body Synchronous verification operation.
 * @returns {T} The body's result.
 * @throws {Error} If the lock exists; body errors propagate after cleanup.
 */
export function withLock(common, body) {
  const lock = join(common, 'delivery-check.lock')
  try { mkdirSync(lock) }
  catch (error) {
    if (error.code !== 'EEXIST') throw error
    throw new Error(`Repository verification is already running. Lock: ${lock}. If its recorded process and the checks it spawned have all exited, remove the stale lock and retry.`)
  }
  try {
    writeFileSync(join(lock, 'owner'), `pid=${process.pid}\n`)
    return body()
  } finally { rmSync(lock, { recursive: true, force: true }) }
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
  if (!['fast', 'full', 'pre-commit', 'pre-push', 'install'].includes(mode) || (mode !== 'pre-push' && rest.length)) throw new Error('Usage: verify.mjs fast|full|pre-commit|pre-push|install')
  root = resolve(git(root, ['rev-parse', '--show-toplevel'], env))
  if (mode === 'install') { print(`Hooks installed for this worktree: ${installHooks(root)}`); return }
  const commits = mode === 'pre-push' ? pushCommits(input ?? readFileSync(0, 'utf8')) : []
  if (mode === 'pre-push' && !commits.length) { print('No commits to verify (deletion or up-to-date push).'); return }
  const common = resolve(root, git(root, ['rev-parse', '--git-common-dir'], env))
  const cache = env.CARGO_TARGET_DIR ? resolve(root, env.CARGO_TARGET_DIR) : join(dirname(common), 'target')
  const logs = join(common, 'delivery-check-logs', `${Date.now()}-${process.pid}`)
  const runEnv = { ...env, CARGO_TARGET_DIR: cache, CARGO_LLVM_COV_TARGET_DIR: env.CARGO_LLVM_COV_TARGET_DIR ? resolve(root, env.CARGO_LLVM_COV_TARGET_DIR) : join(cache, 'llvm-cov-target') }
  withLock(common, () => {
    if (mode === 'fast' || mode === 'full') {
      withTargetCache(root, cache, () => check(root, commands(mode), { env: runEnv, logRoot: logs, print }))
      return
    }
    const paths = git(root, ['diff', '--cached', '--no-renames', '--name-only', '--diff-filter=ACDMRT', '-z'], env).split('\0').filter(Boolean)
    const revisions = mode === 'pre-commit' ? [null] : commits
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
    // export, and the lock above keeps two runs from sharing it. The directory sits where the
    // temporary one always did, outside the checkout: Cargo reads configuration and the discovery
    // walker reads ignore files from every ancestor, so an export under the checkout or the home
    // directory would see the caller's unstaged `.cargo/config.toml` or `~/.gitignore`. The
    // user's runtime directory is private where a shared `/tmp` is not.
    const key = repositoryKey(common)
    const hookCache = join(cache, 'hook-snapshot', key)
    const coverage = join(hookCache, 'llvm-cov-target')
    const hookEnv = { ...runEnv, CARGO_TARGET_DIR: hookCache, CARGO_BUILD_BUILD_DIR: hookCache, CARGO_LLVM_COV_TARGET_DIR: coverage, CARGO_LLVM_COV_BUILD_DIR: coverage }
    const work = join(env.XDG_RUNTIME_DIR || tmpdir(), `vitest-dedup-hook-${key}`)
    for (const [i, revision] of revisions.entries()) {
      print(revision ? `Verifying proposed commit ${revision}` : 'Verifying the staged index')
      mkdirSync(logs, { recursive: true })
      const docDiff = join(logs, `docstrings-${i}.diff`)
      writeFileSync(docDiff, scopeDiff(root, { revision: revision ?? undefined, staged: !revision, env }))
      withSnapshot(root, revision, snapshot => {
        // Full push verification includes coverage: a new ref may have no available remote base.
        withNodeModules(snapshot, root, () => withTargetCache(snapshot, hookCache, () => check(snapshot, commands(mode === 'pre-commit' ? 'fast' : 'full', revision ? true : rustChanged(paths), docDiff), { env: hookEnv, logRoot: join(logs, String(i)), print })))
      }, env, work)
    }
  })
  print(`Verification complete. Logs: ${logs}`)
}

if (process.argv[1] && existsSync(process.argv[1]) && realpathSync(process.argv[1]) === realpathSync(SELF)) {
  try { main() }
  catch (error) { console.error(error.message); process.exitCode = 1 }
}
