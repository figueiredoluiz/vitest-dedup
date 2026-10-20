// The two-binary comparison over the corpus evaluation: which expectations a candidate binary keeps,
// gains or loses relative to a baseline binary run on the same labelled corpus.
//
// Pure except `readLabelChanges` and `buildRunRecord`, which ask git and the filesystem. The
// comparison never turns an unusable baseline run into a quiet one: a case the baseline could not
// be evaluated on (a crash, an old report schema, zero extracted tests) is `unavailable` with the
// reason, and it never counts as zero findings.
import { createHash } from 'node:crypto'
import { lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, readlinkSync, realpathSync, rmSync, statSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { spawnSync } from 'node:child_process'

import { explicitConfigPath } from './corpus-evaluation.mjs'
import { LABEL_FIELDS } from './manifest-schema.mjs'

/** What a good outcome is for each kind of expectation, as a status. */
const GOOD = Object.freeze({
  'expected-finding': 'satisfied',
  'expected-absent': 'held',
  'known-miss': 'now-reported',
  'known-false-positive': 'no-longer-reported',
})

/** Kinds whose good outcome is a reported finding; the rest are good when the analyzer is silent. */
const FINDING_KINDS = new Set(['expected-finding', 'known-miss'])

/** Every class an expectation can fall into, in report order. */
export const CHANGE_CLASSES = Object.freeze([
  'retained',
  'newly-satisfied',
  'corrected',
  'regressed',
  'new-forbidden',
  'still-unmet',
])

/**
 * Classifies one expectation by what happened to it between the baseline and the candidate.
 *
 * - `retained`: good in both.
 * - `newly-satisfied`: a wanted finding the baseline lacked and the candidate has.
 * - `corrected`: a forbidden or false-positive finding the baseline emitted and the candidate does not.
 * - `regressed`: a wanted finding the baseline had and the candidate lost.
 * - `new-forbidden`: a forbidden finding the candidate emits and the baseline did not.
 * - `still-unmet`: not good in either.
 *
 * @param {string} kind The expectation kind.
 * @param {string} baselineStatus The expectation's status on the baseline.
 * @param {string} candidateStatus The expectation's status on the candidate.
 * @returns {string} One of {@link CHANGE_CLASSES}.
 */
export function classifyChange(kind, baselineStatus, candidateStatus) {
  const good = GOOD[kind]
  if (good === undefined) throw new Error(`unknown expectation kind "${kind}"`)
  const wasGood = baselineStatus === good
  const isGood = candidateStatus === good
  if (wasGood && isGood) return 'retained'
  if (!wasGood && !isGood) return 'still-unmet'
  if (isGood) return FINDING_KINDS.has(kind) ? 'newly-satisfied' : 'corrected'
  return FINDING_KINDS.has(kind) ? 'regressed' : 'new-forbidden'
}

/** A finding's identity for set comparison across runs. */
function findingKey(finding) {
  return JSON.stringify([finding.rule, [...finding.locations].sort(), [...finding.titles].sort()])
}

/**
 * The findings of `these` that `others` does not account for, counting repeats: two identical
 * findings against one leaves one.
 *
 * @param {object[]} these The findings to list from.
 * @param {object[]} others The findings that cancel them, one for one.
 * @returns {object[]}
 */
function surplus(these, others) {
  const remaining = new Map()
  for (const finding of others) remaining.set(findingKey(finding), (remaining.get(findingKey(finding)) ?? 0) + 1)
  return these.filter((finding) => {
    const left = remaining.get(findingKey(finding)) ?? 0
    if (left === 0) return true
    remaining.set(findingKey(finding), left - 1)
    return false
  })
}

/**
 * How a case's extraction expectation (test count, completeness, unresolved count) changed.
 *
 * @param {boolean} was Whether the baseline's run matched the labels.
 * @param {boolean} is Whether the candidate's run matched them.
 * @returns {string} `retained`, `newly-satisfied`, `regressed` or `still-unmet`.
 */
function extractionChange(was, is) {
  if (was === is) return is ? 'retained' : 'still-unmet'
  return is ? 'newly-satisfied' : 'regressed'
}

/**
 * Compares one case's candidate evaluation with its baseline evaluation.
 *
 * @param {object} candidate The candidate's {@link evaluateCase} result.
 * @param {{evaluated?: object, unavailable?: string}} baseline The baseline's result, or why there is none.
 * @param {string | null} labelChange Why the case's labels or files differ from the baseline's, or `null`.
 * @returns {object} The case's per-expectation classes and unclaimed-finding differences.
 */
export function compareCase(candidate, baseline, labelChange) {
  const common = { name: candidate.name, rule: candidate.rule, pending: candidate.pending, labelChange }
  if (baseline.unavailable !== undefined) {
    return {
      ...common,
      unavailable: baseline.unavailable,
      extraction: 'unavailable',
      expectations: candidate.expectations.map((o) => ({ id: o.id, kind: o.kind, rule: o.rule, class: 'unavailable', candidate: o.status })),
      newUnclaimed: [],
      goneUnclaimed: [],
    }
  }
  const before = new Map(baseline.evaluated.expectations.map((o) => [o.id, o]))
  const expectations = candidate.expectations.map((o) => {
    const was = before.get(o.id)
    if (was === undefined) throw new Error(`${candidate.name}: expectation ${o.id} is missing from the baseline evaluation`)
    return { id: o.id, kind: o.kind, rule: o.rule, class: classifyChange(o.kind, was.status, o.status), baseline: was.status, candidate: o.status }
  })
  return {
    ...common,
    // Whether the run extracted what the case labels say is itself a contract.
    extraction: extractionChange(baseline.evaluated.extraction.matches, candidate.extraction.matches),
    expectations,
    newUnclaimed: surplus(candidate.unclaimed, baseline.evaluated.unclaimed),
    goneUnclaimed: surplus(baseline.evaluated.unclaimed, candidate.unclaimed),
  }
}

/** A tally with every class present and zero. */
function emptyTally() {
  return Object.fromEntries([...CHANGE_CLASSES, 'unavailable'].map((name) => [name, 0]))
}

/**
 * Compares every case and totals the result. Pending cases are listed but excluded from the totals,
 * as they are from the per-rule contract table; cases whose labels or files changed since the
 * baseline are totalled apart, because a difference there is not a detector change.
 *
 * @param {object[]} candidates The candidate's evaluated cases.
 * @param {Array<{evaluated?: object, unavailable?: string}>} baselines One baseline result per case, in order.
 * @param {Map<string, string>} labelChanges Case name to why it differs from the baseline's corpus.
 * @returns {object} The comparison.
 * @throws {Error} When the inputs disagree in length or no case could be compared.
 */
export function compareCorpus(candidates, baselines, labelChanges) {
  if (candidates.length !== baselines.length) {
    throw new Error(`${baselines.length} baseline result(s) for ${candidates.length} candidate case(s)`)
  }
  const cases = candidates.map((candidate, index) => compareCase(candidate, baselines[index], labelChanges.get(candidate.name) ?? null))
  if (cases.length === 0 || cases.every((c) => c.unavailable !== undefined)) {
    throw new Error('the baseline binary produced no comparable run on any case; nothing was compared')
  }
  const detector = emptyTally()
  const labelled = emptyTally()
  const byRule = new Map()
  let newUnclaimed = 0
  let goneUnclaimed = 0
  let extractionRegressed = 0
  for (const item of cases.filter((c) => c.pending === null)) {
    const tally = item.labelChange === null ? detector : labelled
    for (const outcome of item.expectations) {
      tally[outcome.class] += 1
      if (item.labelChange === null) {
        if (!byRule.has(outcome.rule)) byRule.set(outcome.rule, { rule: outcome.rule, ...emptyTally() })
        byRule.get(outcome.rule)[outcome.class] += 1
      }
    }
    if (item.labelChange === null) {
      newUnclaimed += item.newUnclaimed.length
      goneUnclaimed += item.goneUnclaimed.length
      if (item.extraction === 'regressed') extractionRegressed += 1
    }
  }
  return {
    totals: { detector, labelChanged: labelled, newUnclaimed, goneUnclaimed, extractionRegressed },
    regressions: detector.regressed + detector['new-forbidden'] + newUnclaimed + extractionRegressed,
    // A baseline that cannot run a case new since it was built is expected, and stays data.
    unavailableCases: cases.filter((c) => c.unavailable !== undefined && c.labelChange === null).length,
    unavailableOnChangedCases: cases.filter((c) => c.unavailable !== undefined && c.labelChange !== null).length,
    labelChangedCases: cases.filter((c) => c.labelChange !== null).map((c) => ({ name: c.name, reason: c.labelChange })),
    byRule: [...byRule.values()].sort((a, b) => a.rule.localeCompare(b.rule)),
    cases,
  }
}

/**
 * Lines for the terminal.
 *
 * @param {object} comparison Output of {@link compareCorpus}.
 * @returns {string[]}
 */
export function renderComparison(comparison) {
  const { detector, labelChanged, newUnclaimed, goneUnclaimed } = comparison.totals
  const row = (tally) => CHANGE_CLASSES.map((name) => `${name} ${tally[name]}`).join(', ')
  const lines = [
    '',
    `comparison with the baseline binary: ${comparison.regressions} regression(s), ${comparison.unavailableCases} unavailable case(s)` +
      (comparison.unavailableOnChangedCases > 0 ? ` (and ${comparison.unavailableOnChangedCases} more on cases new or changed since the baseline)` : ''),
    `  detector changes   ${row(detector)}, unavailable ${detector.unavailable}`,
    `  new unclaimed findings ${newUnclaimed}, unclaimed findings gone ${goneUnclaimed}, extraction regressions ${comparison.totals.extractionRegressed}`,
  ]
  if (comparison.labelChangedCases.length > 0) {
    lines.push(`  labels or files changed since the baseline (${comparison.labelChangedCases.length} case(s); not detector changes): ${row(labelChanged)}`)
  }
  for (const rule of comparison.byRule) lines.push(`    ${rule.rule}: ${row(rule)}`)
  const changed = comparison.cases.filter(
    (c) =>
      c.pending === null &&
      c.labelChange === null &&
      (c.newUnclaimed.length > 0 || c.extraction === 'regressed' || c.expectations.some((o) => ['regressed', 'new-forbidden'].includes(o.class))),
  )
  for (const c of changed) {
    const items = c.expectations.filter((o) => ['regressed', 'new-forbidden'].includes(o.class)).map((o) => `${o.id} ${o.class}`)
    for (const f of c.newUnclaimed) items.push(`new unclaimed ${f.rule} @ ${f.locations.join(' + ')}`)
    if (c.extraction === 'regressed') items.push('extraction no longer matches the labels')
    lines.push(`  ${c.name}: ${items.join('; ')}`)
  }
  for (const c of comparison.cases.filter((x) => x.unavailable !== undefined)) lines.push(`  unavailable: ${c.name} -- ${c.unavailable}`)
  for (const c of comparison.labelChangedCases) lines.push(`  label-changed: ${c.name} -- ${c.reason}`)
  return lines
}

/**
 * Runs git and returns stdout.
 *
 * @param {string[]} args The git arguments.
 * @param {string} cwd The working directory.
 * @returns {string}
 * @throws {Error} When git cannot run or exits non-zero.
 */
export function runGit(args, cwd) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', maxBuffer: 64 << 20 })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`git ${args.join(' ')} failed: ${(result.stderr ?? '').trim()}`)
  return result.stdout
}

/** The configuration file name the analyzer discovers above an analyzed directory. */
const CONFIG_FILE_NAME = '.vitest-dedup.json'

/** The ignore files the analyzer's walk reads in each directory above a case. */
const IGNORE_FILE_NAMES = Object.freeze(['.gitignore', '.ignore', '.vitest-dedupignore'])

/** The matcher lists whose order means nothing, because the recall matcher sorts them. */
const SET_FIELDS = ['expectedFindings', 'expectedAbsent', 'knownMisses', 'knownFalsePositives']

/** A manifest case reduced to what it expects: no provenance labels, matcher lists as sets. */
function expectationsOf(testCase) {
  const copy = Object.fromEntries(Object.entries(testCase).filter(([key]) => !LABEL_FIELDS.includes(key)))
  for (const field of SET_FIELDS) {
    if (!Array.isArray(copy[field])) continue
    copy[field] = copy[field]
      .map((matcher) =>
        canonicalJson({
          ...matcher,
          ...(Array.isArray(matcher.titles) ? { titles: [...matcher.titles].sort() } : {}),
          ...(Array.isArray(matcher.locations) ? { locations: [...matcher.locations].sort() } : {}),
        }),
      )
      .sort()
  }
  return canonicalJson(copy)
}

/** JSON with object keys sorted at every depth, so equal values compare equal in any key order. */
function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`
  if (value !== null && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`).join(',')}}`
  }
  return JSON.stringify(value)
}

/** The bytes of a regular file, or `null` when there is none. */
function readIfFile(path) {
  try {
    return statSync(path).isFile() ? readFileSync(path) : null
  } catch {
    return null
  }
}

/**
 * Everything on disk that a case's run reads, as one digest: its files (by path and content) and the
 * configuration that governs it, found as the analyzer finds it. An explicit `--config` replaces the
 * upward search; otherwise the nearest `.vitest-dedup.json` from the case directory up to the top
 * of the tree is the one. The same function runs over the current tree and over the tree at the
 * baseline ref, so a difference between the two digests is a difference in what the case reads, in
 * whatever way it is spelled. A case is taken to be self-contained: a file it imports from outside
 * its own directory is not followed, so a change confined to such a file is not seen.
 *
 * @param {{root: string, top: string}} tree A corpus root and the top of the tree it sits in.
 * @param {string} currentTop The top of the current checkout, to place an absolute `--config` in `tree`.
 * @param {object} testCase A raw manifest case.
 * @returns {string} Hex digest.
 */
export function caseDigest(given, currentTop, testCase) {
  const hash = createHash('sha256')
  // Real paths throughout, so a link in the path to the tree (/var on macOS) cannot hide the top.
  const tree = { root: realpathSync(given.root), top: realpathSync(given.top) }
  // An absolute path that lies in the current checkout names the same place in the baseline tree.
  const place = (path) => {
    const inCheckout = isAbsolute(path) && relative(currentTop, path).split(sep)[0] !== '..'
    return inCheckout ? join(tree.top, relative(currentTop, path)) : resolve(tree.root, path)
  }
  const digestOf = (content) => createHash('sha256').update(content).digest('hex')
  // What a path holds, as the analyzer meets it: a symlink is its own kind of entry (the walk skips
  // it) with a target, and an absolute target in the current checkout is the same place in `tree`.
  const entry = (path) => {
    let link = null
    try {
      if (lstatSync(path).isSymbolicLink()) link = readlinkSync(path)
    } catch {
      return null
    }
    const through = link === null ? path : place(isAbsolute(link) ? link : resolve(dirname(path), link))
    const content = readIfFile(through)
    return content === null && link === null ? null : `${link === null ? 'file' : `link:${link}`}\0${content === null ? 'dangling' : digestOf(content)}`
  }
  // The analyzer canonicalizes the analyzed directory before it looks for configuration.
  let directory = place(testCase.path)
  try {
    // A link that points into the current checkout points into the baseline tree for the baseline.
    directory = place(realpathSync(directory))
  } catch {
    // A directory that does not exist is hashed as given.
  }
  let names = []
  try {
    // Repository metadata exists only in the current tree. The manifest at the corpus root is
    // compared as expectations, so its provenance labels must not make a case look changed.
    names = readdirSync(directory, { recursive: true })
      .map(String)
      .filter((name) => name.split(sep)[0] !== '.git' && !(directory === tree.root && name === 'manifest.json'))
      .sort()
  } catch {
    // A case directory that does not exist hashes as empty, which differs from one that does.
  }
  for (const name of names) {
    const held = entry(join(directory, name))
    if (held !== null) hash.update(`${name.split(sep).join('/')}\0${held}\n`)
  }
  const explicit = explicitConfigPath(testCase.arguments ?? [])
  let config = null
  if (explicit !== null) {
    const path = isAbsolute(explicit) ? place(explicit) : resolve(directory, explicit)
    config = { path, held: entry(path) }
  }
  // The analyzer's discovery also reads the ignore files above a case, and without an explicit
  // configuration the nearest `.vitest-dedup.json`.
  for (let dir = directory; ; dir = dirname(dir)) {
    for (const name of config === null ? [CONFIG_FILE_NAME, ...IGNORE_FILE_NAMES] : IGNORE_FILE_NAMES) {
      const held = entry(join(dir, name))
      if (held === null) continue
      if (name === CONFIG_FILE_NAME) config = { path: join(dir, name), held }
      else hash.update(`ignore\0${relative(tree.top, join(dir, name)).split(sep).join('/')}\0${held}\n`)
    }
    if (dir === tree.top || dirname(dir) === dir) break
  }
  // A path outside the tree is the same file for both, so it is named as it is, not relative to a top.
  const named = (path) => (relative(tree.top, path).split(sep)[0] === '..' ? path : relative(tree.top, path).split(sep).join('/'))
  hash.update(`config\0${config === null ? 'none' : `${named(config.path)}\0${config.held ?? 'absent'}`}\n`)
  return hash.digest('hex')
}

/**
 * Writes the tree at `ref` into `destination`, with `git archive` and `tar`.
 *
 * @param {string} ref The git ref.
 * @param {string} destination An empty directory.
 * @param {string} top The top of the checkout.
 * @param {(args: string[]) => string} git Runs git in the checkout.
 */
function extractTree(ref, destination, top, git) {
  const archive = join(destination, '..', `${basename(destination)}.tar`)
  // `git archive` run below the top would archive only that directory.
  git(['-C', top, 'archive', '--format=tar', '-o', archive, ref])
  const result = spawnSync('tar', ['-xf', archive, '-C', destination], { encoding: 'utf8' })
  if (result.error) throw result.error
  if (result.status !== 0) throw new Error(`tar failed extracting ${ref}: ${(result.stderr ?? '').trim()}`)
}

/**
 * Why each case differs from the corpus at `ref`: new there, its expectations changed, or what its
 * run reads (its files and governing configuration) changed. Provenance labels, key order and the
 * order of a matcher's lists do not count. Cases not listed are unchanged, so a difference in their
 * results is the detector's.
 *
 * The baseline tree is extracted to a temporary directory and read with the same code as the current
 * one, so how git spells a path (renames, quoting, a trailing slash) cannot hide a change.
 *
 * @param {object} options
 * @param {string} options.root The recall corpus root (holds `manifest.json`).
 * @param {string} options.ref The baseline git ref.
 * @param {object[]} options.manifestCases The current manifest's raw `cases`.
 * @param {(args: string[]) => string} options.git Runs git in the checkout.
 * @returns {Map<string, string>} Case name to reason.
 * @throws {Error} When the ref does not exist or has no manifest, since nothing could then be attributed.
 */
export function readLabelChanges({ root, ref, manifestCases, git }) {
  const top = realpathSync(git(['rev-parse', '--show-toplevel']).trim())
  const rel = relative(top, realpathSync(root))
  if (rel.split(sep)[0] === '..' || isAbsolute(rel)) {
    throw new Error(`the corpus root ${root} is outside the checkout at ${top}, so ${ref} cannot be compared with it`)
  }
  const scratch = mkdtempSync(join(tmpdir(), 'corpus-baseline-'))
  const tree = join(scratch, 'tree')
  mkdirSync(tree)
  try {
    extractTree(ref, tree, top, git)
    const then = JSON.parse(readFileSync(join(tree, rel, 'manifest.json'), 'utf8'))
    if (!Array.isArray(then.cases)) throw new Error(`the manifest at ${ref} has no cases`)
    const before = new Map(then.cases.map((c) => [c.name, c]))
    const reasons = new Map()
    for (const testCase of manifestCases) {
      const was = before.get(testCase.name)
      if (was === undefined) reasons.set(testCase.name, `new case, absent at ${ref}`)
      else if (expectationsOf(was) !== expectationsOf(testCase)) reasons.set(testCase.name, `expectations differ from ${ref}`)
      else if (caseDigest({ root: join(tree, rel), top: tree }, top, testCase) !== caseDigest({ root: realpathSync(root), top }, top, testCase)) {
        reasons.set(testCase.name, `its files or configuration differ from ${ref}`)
      }
    }
    return reasons
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

/** The sha256 of a file, hex. */
function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex')
}

/** The sha256 of a binary, or `null` when `path` is not a readable file (a name found on PATH). */
function tryDigest(path) {
  try {
    return sha256File(path)
  } catch {
    return null
  }
}

/**
 * A digest of every file under `root`, by relative path and content, so two runs on the same corpus
 * can be told apart from two runs on different ones.
 *
 * @param {string} root The corpus root.
 * @returns {string} Hex digest.
 */
export function corpusDigest(root) {
  const hash = createHash('sha256')
  const names = readdirSync(root, { recursive: true }).map(String).sort()
  for (const name of names) {
    const path = join(root, name)
    // A link is digested as its target, so a dangling one is a fact about the corpus, not a crash.
    const link = lstatSync(path).isSymbolicLink() ? readlinkSync(path) : null
    if (link !== null) hash.update(`${name.split(sep).join('/')}\0link:${link}\n`)
    else if (statSync(path).isFile()) hash.update(`${name.split(sep).join('/')}\0${sha256File(path)}\n`)
  }
  return hash.digest('hex')
}

/**
 * What a comparison ran on: the binaries by digest and version, the corpus by digest, the checkout
 * commit and the baseline ref's commit, and the CLI arguments each case ran with.
 *
 * @param {object} options
 * @param {string} options.candidate The candidate binary.
 * @param {string} options.baseline The baseline binary.
 * @param {string} options.root The corpus root.
 * @param {string | null} options.ref The baseline git ref, when given.
 * @param {object[]} options.cases The case labels, for their arguments.
 * @param {(args: string[]) => string} options.git Runs git in the checkout.
 * @param {(binary: string) => string} options.version Reports a binary's version line.
 * @returns {object}
 */
export function buildRunRecord({ candidate, baseline, root, ref, cases, git, version }) {
  const tryGit = (args) => {
    try {
      return git(args).trim()
    } catch {
      return null
    }
  }
  return {
    candidate: { binary: candidate, sha256: tryDigest(candidate), version: version(candidate) },
    baseline: { binary: baseline, sha256: tryDigest(baseline), version: version(baseline), ref, commit: ref === null ? null : tryGit(['rev-parse', `${ref}^{commit}`]) },
    corpus: { root, sha256: corpusDigest(root) },
    checkoutCommit: tryGit(['rev-parse', 'HEAD']),
    caseArguments: Object.fromEntries(cases.filter((c) => c.arguments.length > 0).map((c) => [c.name, c.arguments])),
    node: process.version,
  }
}

/**
 * A binary's `--version` line.
 *
 * @param {string} binary The binary path.
 * @returns {string} The trimmed output, or `unavailable` when it cannot be run.
 */
export function binaryVersion(binary) {
  const result = spawnSync(binary, ['--version'], { encoding: 'utf8' })
  return result.status === 0 ? result.stdout.trim() : 'unavailable'
}
