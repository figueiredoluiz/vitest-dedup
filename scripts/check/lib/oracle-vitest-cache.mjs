// Reuses what the pinned Vitest collected for a directory when nothing it could read has changed.
//
// The Vitest side is about 96% of `check:oracle` (431 of 448 s over 496 directories), and a Rust-only
// change cannot alter it. The key covers every file under `fixtures/` and `scripts/check/`, every file
// at the repository root, Node's version and the installed Vitest versions. One key per run, not per
// directory: a case can collect a project outside its own directory, which a per-directory key
// would not see. The analyzer side and every comparison always run.
//
// The environment Vitest receives is keyed too: a fixture may read a variable while it collects.
// Storage that cannot be read or written turns the cache off rather than failing the gate.
//
// Off under `CI` or `VITEST_DEDUP_ORACLE_CACHE=0`, so CI stays the uncached authority.
import { createHash } from 'node:crypto'
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, renameSync, rmSync, statSync, utimesSync, writeFileSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { oracleEnvironment } from './collection-oracle.mjs'

// Bump when the stored shape or what `collectWithVitest` returns changes.
const SCHEMA = 2
const KEYED_TREES = ['fixtures', join('scripts', 'check')]
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

/** Feed every file under `dir` into `hash` in a fixed order: path, kind and bytes or link target. */
function hashTree(hash, root, dir) {
  const absolute = join(root, dir)
  if (!existsSync(absolute)) return
  for (const name of readdirSync(absolute).sort()) {
    const path = join(dir, name)
    const stat = lstatSync(join(root, path))
    if (stat.isSymbolicLink()) hash.update(`link\0${path}\0${readlinkSync(join(root, path))}\0`)
    else if (stat.isDirectory()) hashTree(hash, root, path)
    else if (stat.isFile()) hash.update(`file\0${path}\0${stat.size}\0`).update(readFileSync(join(root, path)))
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
  for (const name of readdirSync(root).sort()) {
    const path = join(root, name)
    if (lstatSync(path).isFile()) hash.update(`root\0${name}\0`).update(readFileSync(path))
  }
  for (const tree of KEYED_TREES) hashTree(hash, root, tree)
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
 * @returns {{collect: (root: string, dir: string) => Map<string, object>, stats: {hits: number, misses: number, enabled: boolean, storageError: string|null}}}
 */
export function cachedCollector(root, collect, { env = process.env, base = cacheRoot(env), key } = {}) {
  const stats = { hits: 0, misses: 0, enabled: cacheEnabled(env), storageError: null }
  if (!stats.enabled) return { collect, stats }
  let keyDir
  try {
    pruneCache(base)
    keyDir = join(base, key ?? cacheKey(root, process.version, env))
    // A run that only reads a key still counts as using it, so pruning goes by last use.
    if (existsSync(keyDir)) { const now = new Date(); utimesSync(keyDir, now, now) }
  } catch (error) {
    stats.enabled = false
    stats.storageError = error.message
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
        stats.storageError = error.message
        try { rmSync(temporary, { force: true }) } catch { /* its directory is what failed */ }
      }
      return files
    },
  }
}
