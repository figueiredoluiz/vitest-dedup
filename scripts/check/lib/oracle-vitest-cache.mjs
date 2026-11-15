// Reuses what the pinned Vitest collected for a directory when nothing it could read has changed.
//
// The Vitest side is about 96% of `check:oracle` (431 of 448 s over 496 directories), and a Rust-only
// change cannot alter it. The key covers what Vitest could read rather than a list of what it does:
// every file git lists for the checkout and every file under the fixture and script trees, except
// Rust sources and the root `.git/`, `target/` and `node_modules/`,
// the installed packages as npm records them, the environment the Vitest child receives, and Node's
// version. One key per run, not per directory: a case can collect a project outside its own
// directory. The analyzer side and every comparison always run.
//
// A symlink in the checkout, a package linked into `node_modules` other than the repository's own
// workspaces, or storage that cannot be read or written, turns the cache off rather than failing
// the gate.
//
// Off under `CI` or `VITEST_DEDUP_ORACLE_CACHE=0`, so CI stays the uncached authority.
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { dirname, join } from 'node:path'
import { oracleEnvironment } from './collection-oracle.mjs'

// Bump when the stored shape or what `collectWithVitest` returns changes.
const SCHEMA = 3
// Root entries Vitest never reads as collection input; installed packages are keyed through npm's record.
const UNKEYED_ROOT = new Set(['.git', 'target', 'node_modules'])
// Walked whole, ignored files included.
const WALKED_TREES = ['fixtures', 'tests/fixtures', 'scripts']
const RETENTION_MS = 7 * 24 * 60 * 60 * 1000

/**
 * Whether the cache may be read and written in this environment.
 * @param {NodeJS.ProcessEnv} [env] Environment to read.
 * @returns {boolean} False under CI or when explicitly disabled.
 */
export function cacheEnabled(env = process.env) {
  return !env.CI && env.VITEST_DEDUP_ORACLE_CACHE !== '0'
}

/**
 * The cache directory for every key.
 * @param {NodeJS.ProcessEnv} [env] Environment to read.
 * @returns {string} `$XDG_CACHE_HOME/vitest-dedup/oracle-vitest`, defaulting to `~/.cache`.
 */
export function cacheRoot(env = process.env) {
  return join(env.XDG_CACHE_HOME || join(homedir(), '.cache'), 'vitest-dedup', 'oracle-vitest')
}

/** Every path under `dir`, relative to `root` with `/` separators, without following links. */
function walk(root, dir) {
  const absolute = join(root, dir)
  if (!existsSync(absolute)) return []
  return readdirSync(absolute).flatMap(name => {
    const path = `${dir}/${name}`
    return lstatSync(join(root, path)).isDirectory() ? walk(root, path) : [path]
  })
}

/**
 * The files the key covers: what git lists for the checkout (tracked, or untracked and not ignored),
 * plus everything under the fixture and script trees, ignored files included, since a fixture's own
 * `node_modules` is read by Vitest and ignored by git. Rust sources and the root `.git/`, `target/`
 * and `node_modules/` are left out.
 * @throws {Error} When git cannot list the checkout.
 */
function keyedPaths(root) {
  const listed = spawnSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'], { cwd: root, encoding: 'utf8', maxBuffer: 256 << 20 })
  if (listed.status !== 0) throw new Error(`git cannot list the checkout: ${(listed.stderr || listed.error?.message || '').trim()}`)
  const paths = new Set(listed.stdout.split('\0').filter(Boolean))
  for (const tree of WALKED_TREES) for (const path of walk(root, tree)) paths.add(path)
  return [...paths].filter(path => !path.endsWith('.rs') && !UNKEYED_ROOT.has(path.split('/')[0])).sort()
}

/**
 * Feed every keyed file into `hash` in a fixed order: path, size and bytes.
 * @throws {Error} At a symlink: a link, or a chain of them, can reach files no key follows, and the
 *   checkout has none, so a run with one goes uncached.
 */
function hashFiles(hash, root) {
  for (const path of keyedPaths(root)) {
    const absolute = join(root, path)
    // A tracked file deleted from the checkout drops out of the hash, which changes the key.
    const stat = lstatSync(absolute, { throwIfNoEntry: false })
    if (!stat) continue
    if (stat.isSymbolicLink()) throw new Error(`${path} is a symlink, which the cache does not follow`)
    if (stat.isFile()) hash.update(`file\0${path}\0${stat.size}\0`).update(readFileSync(absolute))
  }
}

/**
 * Refuse an installed package that is a link the key cannot follow, such as one `npm link` made:
 * npm records no change when its sources change. A link to one of the install's own workspace
 * packages is allowed, since those are this repository's sources.
 * @param {string} root Checkout root.
 * @throws {Error} Naming the first linked package that is not a workspace of the install.
 */
function refuseLinkedPackages(root) {
  const modules = join(root, 'node_modules')
  if (!existsSync(modules)) return
  const install = dirname(realpathSync(modules))
  const manifest = join(install, 'package.json')
  const patterns = existsSync(manifest) ? JSON.parse(readFileSync(manifest, 'utf8')).workspaces ?? [] : []
  const workspaces = new Set(patterns.flatMap(pattern => {
    // `dir/*` is the only form this repository uses; anything else matches nothing and is refused.
    const parent = /^(.+)\/\*$/.exec(pattern)?.[1]
    return parent && existsSync(join(install, parent)) ? readdirSync(join(install, parent)).map(name => realpathSync(join(install, parent, name))) : []
  }))
  const entries = readdirSync(modules).flatMap(name => name.startsWith('@') && lstatSync(join(modules, name)).isDirectory()
    ? readdirSync(join(modules, name)).map(scoped => `${name}/${scoped}`)
    : [name])
  for (const name of entries) {
    const path = join(modules, name)
    if (!lstatSync(path).isSymbolicLink()) continue
    let target
    try { target = realpathSync(path) } catch { throw new Error(`node_modules/${name} does not resolve`) }
    if (!workspaces.has(target)) throw new Error(`node_modules/${name} is a linked package the key cannot follow`)
  }
}

/** The installed version of a package under `root/node_modules`, or `absent`. */
function installed(root, name) {
  const manifest = join(root, 'node_modules', name, 'package.json')
  return existsSync(manifest) ? JSON.parse(readFileSync(manifest, 'utf8')).version : 'absent'
}

/**
 * The key for every collection in one run over `root`.
 * @param {string} root Checkout root.
 * @param {string} [node] Node version.
 * @param {NodeJS.ProcessEnv} [env] The gate's environment; the part Vitest receives is keyed.
 * @returns {string} Hex SHA-256.
 */
export function cacheKey(root, node = process.version, env = process.env) {
  const hash = createHash('sha256').update(`schema ${SCHEMA}\0root ${root}\0node ${node}\0`)
  // Exactly what the Vitest child inherits, without the per-call output path.
  const { VITEST_DEDUP_ORACLE_OUT: _out, ...inherited } = oracleEnvironment(env, '')
  for (const name of Object.keys(inherited).sort()) hash.update(`env\0${name}\0${inherited[name]}\0`)
  hash.update(`vitest ${installed(root, 'vitest')}\0vitest4 ${installed(root, 'vitest4')}\0`)
  // npm rewrites its record of the installed tree on every install, `--no-save` included.
  const record = join(root, 'node_modules', '.package-lock.json')
  hash.update('installed\0').update(existsSync(record) ? readFileSync(record) : 'absent')
  refuseLinkedPackages(root)
  hashFiles(hash, root)
  return hash.digest('hex')
}

/** The entry file for `dir` under a key directory. */
const entryPath = (keyDir, dir) => join(keyDir, `${createHash('sha256').update(dir).digest('hex')}.json`)

/**
 * Remove key directories no run has used for longer than the retention window.
 * @param {string} base Cache root.
 * @param {number} [now] Current time, for tests.
 * @returns {void}
 */
export function pruneCache(base, now = Date.now()) {
  if (!existsSync(base)) return
  for (const name of readdirSync(base)) {
    const path = join(base, name)
    if (/^[0-9a-f]{64}$/.test(name) && now - statSync(path).mtimeMs > RETENTION_MS) rmSync(path, { recursive: true, force: true })
  }
}

/**
 * Wrap `collect` so a directory collected under the same key is read back instead of re-collected.
 * @param {string} root Checkout root.
 * @param {(root: string, dir: string) => Map<string, object>} collect The real collection.
 * @param {object} [options] Environment, cache root and key, for tests.
 * @returns {{collect: (root: string, dir: string) => Map<string, object>, stats: {hits: number, misses: number, enabled: boolean, offReason: string|null}}}
 */
export function cachedCollector(root, collect, { env = process.env, base = cacheRoot(env), key } = {}) {
  const stats = { hits: 0, misses: 0, enabled: cacheEnabled(env), offReason: null }
  if (!stats.enabled) return { collect, stats }
  let keyDir
  try {
    pruneCache(base)
    keyDir = join(base, key ?? cacheKey(root, process.version, env))
    // A run that only reads a key still counts as using it, so pruning goes by last use.
    if (existsSync(keyDir)) { const now = new Date(); utimesSync(keyDir, now, now) }
  } catch (error) {
    stats.enabled = false
    stats.offReason = error.message
    return { collect, stats }
  }
  return {
    stats,
    collect(collectRoot, dir) {
      const entry = entryPath(keyDir, dir)
      try {
        const stored = JSON.parse(readFileSync(entry, 'utf8'))
        if (stored.dir === dir && Array.isArray(stored.files)) {
          stats.hits++
          return new Map(stored.files)
        }
      } catch { /* absent or unreadable: collect again */ }
      stats.misses++
      const files = collect(collectRoot, dir)
      const temporary = `${entry}.${process.pid}.tmp`
      try {
        mkdirSync(keyDir, { recursive: true })
        writeFileSync(temporary, JSON.stringify({ dir, files: [...files] }))
        renameSync(temporary, entry)
      } catch (error) {
        // The collection stands; only the cache is lost.
        stats.offReason = error.message
        try { rmSync(temporary, { force: true }) } catch { /* its directory is what failed */ }
      }
      return files
    },
  }
}
