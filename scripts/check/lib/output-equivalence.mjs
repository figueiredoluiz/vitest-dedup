// Comparison rules for the output-equivalence check: what in two runs of the analyzer may differ
// (the terminal detection time, the process peak RSS, the version string) and what must not
// (everything else).
import { createHash, randomUUID } from 'node:crypto'
import {
  chmodSync,
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  rmSync,
  symlinkSync,
  utimesSync,
  writeFileSync,
} from 'node:fs'
import { dirname, join, resolve, sep } from 'node:path'

// The footer is a line of its own; a test title or path that merely contains the words is content.
const TERMINAL_TIMING = /^Detection time: \d+(?:\.\d+)? ms$/gm
// `--debug-memory` stamps every `memory` record with the process high-water mark, which varies run
// to run; the same text elsewhere (a title, a path) is content.
const PEAK_RSS = /^(\{"type":"memory",[^\n]*?"peakRssBytes":)\d+/gm

/** `text` made safe to build a regular expression from. */
const escapePattern = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/**
 * What differs between two versions of the analyzer in output: the places the version is printed,
 * as `[from, to, scopes]` replacements for the candidate's text. Each applies only to the output it
 * occurs in (the banner and `--version` line on stdout, the field in a SARIF file) and only as a line
 * of its own, so a test title, a path or an embedded snippet that contains the same words or number
 * is still compared as it is.
 */
export function versionReplacements([from, to]) {
  const version = escapePattern(from)
  return [
    [new RegExp(`^VitestDedup v${version}$`, 'gm'), `VitestDedup v${to}`, ['stdout']],
    [new RegExp(`^vitest-dedup ${version}$`, 'gm'), `vitest-dedup ${to}`, ['stdout']],
    [new RegExp(`(?<=^\\s*)"semanticVersion": "${version}"(?=,?$)`, 'gm'), `"semanticVersion": "${to}"`, ['sarif']],
  ]
}

/** Index and value of the `name` option (default `--output`), or null when the run does not pass it. */
export function outputArgument(args, name = '--output') {
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name && index + 1 < args.length) {
      return { index, inline: false, value: args[index + 1] }
    }
    if (args[index].startsWith(`${name}=`)) {
      return { index, inline: true, value: args[index].slice(name.length + 1) }
    }
  }
  return null
}

/**
 * Text with run-dependent values replaced. `scope` names the output: `stdout` (the terminal footer's
 * timing and a memory record's peak RSS are erased there only), `stderr`, `file`, or `sarif` for a
 * SARIF file. Each `[from, to, scopes]` replacement applies to the scopes it lists, or to all.
 */
export function normalize(text, replacements = [], scope = 'stdout') {
  const stable =
    scope === 'stdout' ? text.replace(TERMINAL_TIMING, 'Detection time: 0 ms').replace(PEAK_RSS, '$10') : text
  return replace(stable, replacements, scope)
}

/**
 * `text` with each side-specific replacement applied, and nothing else. A `from` is a pattern or a
 * literal; a literal match inside a longer number (`0.7.0` within `10.7.0` or `0.7.01`) is another
 * value and is left alone.
 */
function replace(text, replacements, scope) {
  let replaced = text
  for (const [from, to, scopes] of replacements) {
    if (!from || (scopes && !scopes.includes(scope))) continue
    const pattern = from instanceof RegExp ? from : new RegExp(`(?<![\\d.])${escapePattern(from)}(?!\\d|\\.\\d)`, 'g')
    replaced = replaced.replace(pattern, () => to)
  }
  return replaced
}

/** First line at which two texts differ, with both sides, or null when they are equal. */
export function firstDifference(left, right) {
  if (left === right) return null
  const a = left.split('\n')
  const b = right.split('\n')
  for (let line = 0; line < Math.max(a.length, b.length); line += 1) {
    if (a[line] !== b[line]) {
      return { line: line + 1, baseline: a[line] ?? '<end>', candidate: b[line] ?? '<end>' }
    }
  }
  return { line: a.length, baseline: '<trailing>', candidate: '<trailing>' }
}

// `ignoreBOM` keeps a byte-order mark as a character, so output that gains or loses one differs.
const UTF8 = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true })

/** Output as compared: text when valid UTF-8, otherwise `{ binary }`, a digest of its exact bytes. */
export function comparable(value) {
  if (!Buffer.isBuffer(value)) return value
  try {
    return UTF8.decode(value)
  } catch {
    // Lossy decoding would map different invalid bytes to the same replacement characters.
    return { binary: createHash('sha256').update(value).digest('hex') }
  }
}

/**
 * Differences between a baseline and a candidate run: exit status, streams and written files.
 *
 * Each side is normalized with its own replacements, so both can map their own output directory
 * and version string to one shared spelling.
 */
export function compareRuns(baseline, candidate, { baseline: left = [], candidate: right = [] } = {}) {
  const differences = []
  if (baseline.status !== candidate.status || baseline.signal !== candidate.signal) {
    differences.push({
      part: 'exit',
      baseline: baseline.signal ?? baseline.status,
      candidate: candidate.signal ?? candidate.status,
    })
  }
  /** Adds the difference between two outputs of `part`: text normalized and diffed, binary by digest. */
  const compare = (part, before, after) => {
    const [text, other] = [comparable(before), comparable(after)]
    if (typeof text === 'string' && typeof other === 'string') {
      const scope = part.startsWith('file:') ? (part.endsWith('.sarif') ? 'sarif' : 'file') : part
      const difference = firstDifference(normalize(text, left, scope), normalize(other, right, scope))
      if (difference) differences.push({ part, ...difference })
      return
    }
    /** A label that differs whenever the bytes or the text/binary kind differ. */
    const label = (value) => (typeof value === 'string' ? 'text' : `binary sha256:${value.binary}`)
    if (label(text) !== label(other)) differences.push({ part, baseline: label(text), candidate: label(other) })
  }
  for (const part of ['stdout', 'stderr']) compare(part, baseline[part], candidate[part])
  const baselineFiles = baseline.files ?? new Map()
  const candidateFiles = candidate.files ?? new Map()
  const names = [...new Set([...baselineFiles.keys(), ...candidateFiles.keys()])].sort()
  for (const name of names) {
    /** How one side left `name`: untouched, deleted, a link to a target (with that side's replacements), or written. */
    const state = (files, replacements) => {
      if (!files.has(name)) return 'missing'
      const value = files.get(name)
      if (value === DELETED) return 'deleted'
      // Only a plain object is a link: a string has a `link` method of its own.
      const link = typeof value === 'object' && !Buffer.isBuffer(value)
      return link ? `link -> ${replace(value.link, replacements, 'file')}` : 'present'
    }
    const [before, after] = [state(baselineFiles, left), state(candidateFiles, right)]
    if (before !== 'present' || after !== 'present') {
      if (before !== after) differences.push({ part: `file:${name}`, baseline: before, candidate: after })
      continue
    }
    compare(`file:${name}`, baselineFiles.get(name), candidateFiles.get(name))
  }
  return differences
}

// Opening never follows a link or waits on a FIFO; both flags are absent (0) on Windows.
const NO_FOLLOW = constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0)

/**
 * The non-directory entry at `path`, read through one descriptor so its time and bytes describe the
 * same file: a link by its target (never followed), a regular file by time and bytes.
 */
function leafEntry(path, lenient) {
  let descriptor
  try {
    descriptor = openSync(path, NO_FOLLOW)
  } catch (error) {
    // `O_NOFOLLOW` refuses a link with ELOOP, so a link swapped in after classification stays a leaf.
    if (error.code === 'ELOOP') return { kind: 'link', target: readlinkSync(path) }
    // A socket cannot be opened; to `restore` it is just something to remove.
    if (lenient) return { kind: 'other' }
    throw error
  }
  try {
    const stats = fstatSync(descriptor)
    if (!stats.isFile()) {
      if (lenient) return { kind: 'other' }
      // fail-closed: a FIFO, socket or device in a write location would block or not compare.
      throw new Error(`unsupported file type in a write location: ${path}`)
    }
    return { kind: 'file', time: stats.mtimeMs, mode: stats.mode & 0o7777, bytes: readFileSync(descriptor) }
  } finally {
    closeSync(descriptor)
  }
}

/**
 * Every directory, file and symbolic link at or below `path`, keyed by absolute path; empty when
 * it does not exist. Take it only while nothing writes to the location (between analyzer runs): a
 * concurrent swap of a listed directory for a link is not detected. Links, the location itself included, carry their target and are never
 * followed, so a link loop or a link out of the location cannot hang the walk or pull in outside paths.
 * A FIFO, socket or device throws, unless `lenient` (for `restore`, which only removes it) records it
 * as `{ kind: "other" }`.
 */
export function snapshot(path, { lenient = false } = {}) {
  // `resolve` drops a trailing separator, which would make `lstat` resolve a link to its target.
  const location = resolve(path)
  const entries = new Map()
  /** Records the directory `path`, once listed, and everything below it. */
  const visit = (path) => {
    const children = readdirSync(path, { withFileTypes: true })
    entries.set(path, { kind: 'directory' })
    for (const entry of children) {
      const child = join(path, entry.name)
      if (entry.isSymbolicLink()) entries.set(child, { kind: 'link', target: readlinkSync(child) })
      else if (entry.isDirectory()) visit(child)
      else entries.set(child, leafEntry(child, lenient))
    }
  }
  let root
  try {
    root = lstatSync(location, { throwIfNoEntry: false })
  } catch (error) {
    // A parent that is now a file (ENOTDIR) leaves the location as absent as a missing one.
    if (error.code !== 'ENOTDIR') throw error
  }
  if (root?.isSymbolicLink()) entries.set(location, { kind: 'link', target: readlinkSync(location) })
  else if (root?.isDirectory()) visit(location)
  else if (root) entries.set(location, leafEntry(location, lenient))
  return entries
}

/** Content recorded for a file a run deleted; a symbol, so no file content can equal it. */
export const DELETED = Symbol('deleted')

/** Whether `entry` differs from `prior`: another kind, a moved link, or a file rewritten by time or bytes. */
function changed(prior, entry) {
  if (!prior || prior.kind !== entry.kind) return true
  if (entry.kind === 'link') return prior.target !== entry.target
  // Time catches a rewrite with identical bytes; bytes catch one within a coarse timestamp tick.
  return entry.kind === 'file' && (prior.time !== entry.time || !prior.bytes.equals(entry.bytes))
}

/**
 * Files and links at or below `location` created, changed or deleted since the `before` snapshot:
 * a file as its bytes, a link as `{ link: target }`, a deletion as `DELETED`.
 */
export function changedFiles(location, before) {
  const now = snapshot(location)
  const written = [...now]
    .filter(([path, entry]) => entry.kind !== 'directory' && changed(before.get(path), entry))
    .map(([path, entry]) => [path, entry.kind === 'link' ? { link: entry.target } : entry.bytes])
  const deleted = [...before]
    // A leaf whose path now holds a directory is gone too; the directory itself is not output.
    .filter(([path, entry]) => entry.kind !== 'directory' && (!now.has(path) || now.get(path).kind === 'directory'))
    .map(([path]) => [path, DELETED])
  return [...written, ...deleted]
}

/**
 * Returns `location` to its `before` snapshot: removes what is new and puts back what changed. A
 * changed file is recreated, never written into, so a run that left a hard link to an outside file
 * cannot have that file overwritten; links are put back as links and never followed.
 */
export function restore(location, before) {
  // Lenient: an unsupported entry a run created is removed, not a reason to leave the location changed.
  const now = snapshot(location, { lenient: true })
  for (const path of now.keys()) if (!before.has(path)) rmSync(path, { recursive: true, force: true })
  // Parents precede children in a snapshot, so a directory exists before its entries are restored.
  for (const [path, entry] of before) {
    const current = now.get(path)
    if (current && current.kind !== entry.kind) rmSync(path, { recursive: true, force: true })
    if (entry.kind === 'directory') mkdirSync(path, { recursive: true })
    else if (entry.kind === 'link') {
      if (current?.kind === 'link' && current.target === entry.target) continue
      rmSync(path, { force: true })
      symlinkSync(entry.target, path)
    } else if (changed(current, entry)) {
      rmSync(path, { force: true })
      // The new file gets the original mode (the umask can only narrow it, `chmod` then sets it exactly),
      // so a recreated file never has more access than it had; modes a run changes are not restored.
      writeFileSync(path, entry.bytes, { mode: entry.mode })
      chmodSync(path, entry.mode)
      // The original time too, so a later write in the same clock tick still reads as a change.
      utimesSync(path, entry.time / 1000, entry.time / 1000)
    }
  }
}

/** Whether anything, a dangling link included, exists at `path`; a parent that is a file means no. */
function present(path) {
  try {
    return lstatSync(path, { throwIfNoEntry: false }) !== undefined
  } catch (error) {
    if (error.code === 'ENOTDIR') return false
    throw error
  }
}

/** Outermost missing path among `path` and its ancestors, or null when `path` exists. */
export function missingAncestor(path) {
  let missing = null
  for (let current = resolve(path); !present(current); current = dirname(current)) missing = current
  return missing
}

/**
 * Runs `baseline`, then `candidate`, each with the files and links it created, changed or deleted in
 * `locations` added to its `files`. The locations return to their pre-baseline state before the
 * candidate runs, and the candidate's snapshot is taken after that, so a restored file is never
 * counted as a candidate write. Like `snapshot`, it requires that nothing else writes there meanwhile.
 * Limits: a location that is already a symbolic link is recorded as the link, not followed, and one
 * inside another through a link is rejected; locations are compared as spelled after `resolve`, so two
 * spellings differing only in letter case are distinct; files sharing an inode (hard links) are
 * restored as separate files; permission changes a run makes are neither compared nor restored.
 */
export function pairRuns(locations, baseline, candidate) {
  // Resolved like snapshot keys, so a location's spelling cannot drop its output from the comparison.
  const resolved = [...new Set(locations.map((location) => resolve(location)))]
  // A missing location is watched from its outermost missing ancestor, so restoring removes every
  // directory a run created on the way to it.
  const watched = [...new Set(resolved.map((location) => missingAncestor(location) ?? location))]
  /** Whether `path` is `parent` or below it. */
  const within = (path, parent) => path === parent || path.startsWith(parent.endsWith(sep) ? parent : parent + sep)
  // A root inside another is covered by the outer snapshot; restoring it separately could write
  // through a link the outer root was replaced with.
  for (const root of watched) {
    for (const outer of watched.filter((other) => other !== root && within(root, other))) {
      // Every component from the outer location down to the nested one's parent, the outer one included.
      for (let current = dirname(root); ; current = dirname(current)) {
        // fail-closed: the outer snapshot stops at a link, so a location reached through one would be
        // neither compared nor restored.
        if (lstatSync(current, { throwIfNoEntry: false })?.isSymbolicLink()) {
          throw new Error(`unsupported write location ${root}: inside ${outer} through the symbolic link ${current}`)
        }
        if (current === outer) break
      }
    }
  }
  const roots = watched.filter((root) => !watched.some((other) => other !== root && within(root, other)))
  /** Whether `path` is one of the locations or below one; a missing parent is watched only to restore it. */
  const inside = (path) => resolved.some((location) => within(path, location))
  /** Every watched root with its current snapshot. */
  const capture = () => roots.map((root) => [root, snapshot(root)])
  /** `run()` with what it changed inside the locations since `before` added to its files. */
  const observe = (run, before) => {
    const result = run()
    const written = before.flatMap(([root, entries]) => changedFiles(root, entries)).filter(([path]) => inside(path))
    return { ...result, files: new Map([...(result.files ?? []), ...written]) }
  }
  /** Returns every watched root to `before`, attempting each even when another fails. */
  const restoreAll = (before) => {
    const errors = []
    for (const [root, entries] of before) {
      try {
        restore(root, entries)
      } catch (error) {
        errors.push(error)
      }
    }
    if (errors.length === 1) throw errors[0]
    if (errors.length > 1) throw new AggregateError(errors, 'watched locations could not be restored')
  }
  const initial = capture()
  let first
  try {
    first = observe(baseline, initial)
  } catch (error) {
    // A failed baseline still leaves its partial changes behind; undo them, keep its error first.
    try {
      restoreAll(initial)
    } catch (restoreError) {
      throw new AggregateError([error, restoreError], 'baseline run failed and its locations could not be restored')
    }
    throw error
  }
  // A failed restore throws here, so the candidate never starts from a half-restored location.
  restoreAll(initial)
  return [first, observe(candidate, capture())]
}

/** Every value `name` is given, in both spellings, in order; for options the analyzer lets repeat. */
function optionValues(args, name) {
  const values = []
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === name && index + 1 < args.length) values.push(args[(index += 1)])
    else if (args[index].startsWith(`${name}=`)) values.push(args[index].slice(name.length + 1))
  }
  return values
}

/** The options of `args`: everything before a `--` terminator, after which words are paths. */
function optionsOf(args) {
  const terminator = args.indexOf('--')
  return terminator === -1 ? args : args.slice(0, terminator)
}

/** File extension of each reporter that writes a file; `terminal` and `jsonl` write to stdout. */
const FILE_REPORTERS = { json: 'json', html: 'html', sarif: 'sarif' }

/**
 * The components of `path` that name something, refusing a `..` that follows one: the analyzer
 * leaves it to the filesystem, which follows a symbolic link in front of it, while this check
 * resolves paths lexically, so it would watch a different file.
 */
function plainParts(path) {
  const parts = path.split('/').filter((part) => part !== '' && part !== '.')
  const firstName = parts.findIndex((part) => part !== '..')
  if (firstName !== -1 && parts.slice(firstName).includes('..')) {
    throw new Error(`unsupported write location ${path}: a ".." after a named component may follow a symbolic link`)
  }
  return parts
}

/**
 * The file `--output PREFIX` plus a reporter's extension names, as the analyzer derives it: the
 * prefix is a filename stem, so its last component gains `.EXT`; a prefix with no such component
 * (empty, `.`, ending in `..`) is a directory that gets `vitest-dedup.EXT` beside it.
 */
function reportPath(prefix, extension, cwd) {
  const parts = plainParts(prefix)
  const last = parts.at(-1)
  if (last === undefined || last === '..') return resolve(cwd, prefix, `vitest-dedup.${extension}`)
  return resolve(cwd, prefix, '..', `${last}.${extension}`)
}

/**
 * Files a run writes: one per file reporter in `--reporters`, named from `--output` (default
 * `reports/vitest-dedup`), and the `--baseline` file under `--update-baseline`. Reporters and output
 * are command-line only, and every path is relative to `cwd`.
 */
export function writeLocations(args, cwd) {
  const options = optionsOf(args)
  // The analyzer appends repeated `--reporters`, so every occurrence selects reports.
  const reporters = optionValues(options, '--reporters').flatMap((value) => value.split(','))
  // `--output` and `--baseline` are global: given before and after `check`, the later one wins. Every
  // spelling is watched, which is a superset of the file written.
  const given = optionValues(options, '--output')
  const prefixes = given.length > 0 ? given : ['reports/vitest-dedup']
  const reports = prefixes.flatMap((prefix) =>
    reporters.filter((name) => name in FILE_REPORTERS).map((name) => reportPath(prefix, FILE_REPORTERS[name], cwd)),
  )
  const baselines = options.includes('--update-baseline') ? optionValues(options, '--baseline') : []
  for (const baseline of baselines) plainParts(baseline)
  return [...reports, ...baselines.map((baseline) => resolve(cwd, baseline))]
}

/**
 * Writes one shim record as its own file in `directory`. Parallel test threads can run many shims at
 * once; separate files, created exclusively, cannot interleave the way appends to one log could.
 */
export function writeRecord(directory, record) {
  writeFileSync(join(directory, `${process.pid}-${randomUUID()}.json`), JSON.stringify(record), { flag: 'wx' })
}

/** Every record `writeRecord` left in `directory`. */
export function readRecords(directory) {
  return readdirSync(directory)
    .sort()
    .map((name) => JSON.parse(readFileSync(join(directory, name), 'utf8')))
}

/** Verdict over the shim log: invocations per suite, mismatches, and suites that ran nothing. */
export function summarize(records, suites) {
  const invocations = Object.fromEntries(suites.map((suite) => [suite, 0]))
  const mismatches = []
  for (const record of records) {
    invocations[record.suite] = (invocations[record.suite] ?? 0) + 1
    if (record.differences.length > 0) mismatches.push(record)
  }
  const empty = suites.filter((suite) => invocations[suite] === 0)
  return { invocations, mismatches, empty, ok: mismatches.length === 0 && empty.length === 0 }
}
