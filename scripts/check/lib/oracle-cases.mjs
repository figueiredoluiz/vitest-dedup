// Owned cases of the collection oracle (`fixtures/oracle/<dir>/`), each with its own expectations.
//
// A case directory holds a Vitest project and an `expected.json`, so a counterexample or a planned
// behaviour lives next to its fixture and adding one touches no shared file. What Vitest collects
// and what the analyzer reports are pinned per file. A pin the analyzer does not meet yet is held:
// it records today's value and what it waits on, and fails the gate the moment the target is met,
// so the expectation changes in the same commit as the support.
//
// `expected.json`:
//   files: {[path]: {tasks, wrapperSites, next, wrapperSitesToday?, nextToday?, waitsOn?}
//           or {oracle: 'not-collected', next, nextToday?, waitsOn?}}
//   knownSites?, knownTitles?: [{path, kind, item, reason, waitsOn}]
//   blindUnderCurrent?: [path]
// `tasks` is the exact number of tests and suites Vitest collects from the file. `wrapperSites` is
// its exact list of `wrapper@L` and `hook-wrapper@L`, which the site comparison cannot pin: it only
// sees a task with no site. `next` is the next engine's `{reasons, testsExcluded}` for the file,
// `{}` for no record, with the file's primary reason first. `waitsOn` says in plain words what the
// held value waits on.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { isDeepStrictEqual } from 'node:util'
import { matchKnown } from './collection-oracle.mjs'

/** The directory of the cases, relative to the checkout. */
export const CASES_DIR = 'fixtures/oracle'

const CASE_KEYS = new Set(['files', 'knownSites', 'knownTitles', 'blindUnderCurrent'])
const FILE_KEYS = new Set(['oracle', 'tasks', 'wrapperSites', 'wrapperSitesToday', 'next', 'nextToday', 'waitsOn'])
const KNOWN_KEYS = new Set(['path', 'kind', 'item', 'reason', 'waitsOn'])
const LEDGER_KEYS = new Set(['reasons', 'testsExcluded'])

/** @returns {boolean} Whether `value` is an object, not an array. */
const isObject = (value) => typeof value === 'object' && value !== null && !Array.isArray(value)

/** @returns {boolean} Whether `value` is plain words: a non-empty string naming no backlog row. */
const plainWords = (value) => typeof value === 'string' && value.trim() !== '' && !/\b(?:NC|RV|[BDEHMNPQX])-?\d+\b/i.test(value)

/** The keys of `object` outside `allowed`, as problem lines under `where`. */
function unknownKeys(object, allowed, where) {
  return Object.keys(object).filter((key) => !allowed.has(key)).map((key) => `${where}: unknown key ${JSON.stringify(key)}`)
}

/** Why `ledger` is not a next-engine pin: `{}`, or the reasons, primary first, with the tests they exclude. */
function ledgerProblems(ledger, where) {
  if (!isObject(ledger)) return [`${where}: must be an object`]
  const problems = unknownKeys(ledger, LEDGER_KEYS, where)
  if (Object.keys(ledger).length === 0) return problems
  const { reasons, testsExcluded } = ledger
  if (!Array.isArray(reasons) || reasons.length === 0 || !reasons.every((r) => typeof r === 'string')) {
    problems.push(`${where}: reasons must be a non-empty list of strings`)
  }
  if (!Number.isInteger(testsExcluded) || testsExcluded < 0) problems.push(`${where}: testsExcluded must be a count`)
  return problems
}

/** Why the held `today` value of `field` cannot stand: it needs a reason to wait, and a difference. */
function heldProblems(entry, field, today, where) {
  if (entry[today] === undefined) return []
  const problems = []
  if (!plainWords(entry.waitsOn)) problems.push(`${where}: ${today} needs waitsOn, in plain words`)
  if (isDeepStrictEqual(entry[today], entry[field])) problems.push(`${where}: ${today} equals ${field}; nothing is held`)
  return problems
}

/** Why the expectation `entry` of one file is malformed. */
function fileProblems(entry, where) {
  if (!isObject(entry)) return [`${where}: must be an object`]
  const problems = unknownKeys(entry, FILE_KEYS, where)
  problems.push(...ledgerProblems(entry.next, `${where}.next`))
  if (entry.nextToday !== undefined) problems.push(...ledgerProblems(entry.nextToday, `${where}.nextToday`))
  problems.push(...heldProblems(entry, 'next', 'nextToday', where))
  if (entry.waitsOn !== undefined && entry.nextToday === undefined && entry.wrapperSitesToday === undefined) {
    problems.push(`${where}: waitsOn without a held value`)
  }
  if (entry.oracle !== undefined) {
    if (entry.oracle !== 'not-collected') problems.push(`${where}: oracle must be "not-collected"`)
    if (entry.tasks !== undefined || entry.wrapperSites !== undefined || entry.wrapperSitesToday !== undefined) {
      problems.push(`${where}: a file not collected pins no tasks or sites`)
    }
    return problems
  }
  if (!Number.isInteger(entry.tasks) || entry.tasks < 1) problems.push(`${where}: tasks must be a positive count`)
  const siteList = (list) => Array.isArray(list) && list.every((site) => /^(?:hook-)?wrapper@\d+$/.test(site))
  if (!siteList(entry.wrapperSites)) problems.push(`${where}: wrapperSites must list wrapper@L or hook-wrapper@L`)
  if (entry.wrapperSitesToday !== undefined && !siteList(entry.wrapperSitesToday)) {
    problems.push(`${where}: wrapperSitesToday must list wrapper@L or hook-wrapper@L`)
  }
  problems.push(...heldProblems(entry, 'wrapperSites', 'wrapperSitesToday', where))
  return problems
}

/** Why a `knownSites` or `knownTitles` list is malformed. */
function knownProblems(list, where) {
  if (list === undefined) return []
  if (!Array.isArray(list)) return [`${where}: must be a list`]
  return list.flatMap((entry, index) => {
    const at = `${where}[${index}]`
    if (!isObject(entry)) return [`${at}: must be an object`]
    const problems = unknownKeys(entry, KNOWN_KEYS, at)
    for (const key of ['path', 'kind', 'item', 'reason']) {
      if (typeof entry[key] !== 'string' || entry[key] === '') problems.push(`${at}: ${key} is required`)
    }
    if (!plainWords(entry.waitsOn)) problems.push(`${at}: waitsOn is required, in plain words`)
    return problems
  })
}

/**
 * Why `expected` is not a well-formed case.
 *
 * @param {unknown} expected A parsed `expected.json`.
 * @param {string} where The case, for the messages.
 * @returns {string[]} One line per problem; none when it is well formed.
 */
export function expectedProblems(expected, where) {
  if (!isObject(expected)) return [`${where}: must be an object`]
  const problems = unknownKeys(expected, CASE_KEYS, where)
  if (!isObject(expected.files) || Object.keys(expected.files).length === 0) {
    problems.push(`${where}: files must declare at least one file`)
  } else {
    for (const [path, entry] of Object.entries(expected.files)) problems.push(...fileProblems(entry, `${where}: ${path}`))
  }
  problems.push(...knownProblems(expected.knownSites, `${where}: knownSites`))
  problems.push(...knownProblems(expected.knownTitles, `${where}: knownTitles`))
  const blind = expected.blindUnderCurrent
  if (blind !== undefined && !(Array.isArray(blind) && blind.every((path) => typeof path === 'string'))) {
    problems.push(`${where}: blindUnderCurrent must list paths`)
  }
  return problems
}

/**
 * Reads every case under `fixtures/oracle/`.
 *
 * @param {string} root Checkout root.
 * @returns {{cases: {name: string, dir: string, expected: object}[], problems: string[]}} The
 *   well-formed cases, and why the others are not: a directory with no parsable `expected.json`
 *   measures nothing, so it fails instead of being skipped.
 */
export function readCases(root) {
  const base = join(root, CASES_DIR)
  const cases = []
  const problems = []
  const names = existsSync(base) ? readdirSync(base).filter((name) => statSync(join(base, name)).isDirectory()).sort() : []
  for (const name of names) {
    const dir = `${CASES_DIR}/${name}`
    const file = join(base, name, 'expected.json')
    if (!existsSync(file)) {
      problems.push(`${dir}: no expected.json`)
      continue
    }
    let expected
    try {
      expected = JSON.parse(readFileSync(file, 'utf8'))
    } catch (error) {
      problems.push(`${dir}: expected.json does not parse: ${error.message}`)
      continue
    }
    const found = expectedProblems(expected, dir)
    if (found.length > 0) problems.push(...found)
    else cases.push({ name, dir, expected })
  }
  return { cases, problems }
}

/**
 * The paths of the cases that are blind under the current core, as `fixtures/`-relative paths.
 *
 * @param {{name: string, expected: object}[]} cases Cases from `readCases`.
 * @returns {string[]} Their `blindUnderCurrent` files.
 */
export function blindCaseFiles(cases) {
  return cases.flatMap(({ name, expected }) => (expected.blindUnderCurrent ?? []).map((path) => `oracle/${name}/${path}`))
}

/** The `wrapper@L` and `hook-wrapper@L` of `fileSites`, in line order. */
function wrapperSites(fileSites) {
  return fileSites
    .filter((site) => site.kind === 'wrapper' || site.kind === 'hook-wrapper')
    .sort((a, b) => a.line - b.line)
    .map((site) => `${site.kind}@${site.line}`)
}

/**
 * Judges one observed value against its target, or against a held value that waits on something.
 * Today's value holds; the target met with a held value set fails, so the expectation is updated
 * with the support; anything else fails.
 */
function pin({ where, field, observed, target, today, waitsOn }, out) {
  const shown = JSON.stringify(observed)
  if (today === undefined) {
    if (!isDeepStrictEqual(observed, target)) out.failures.push(`${where}: ${field} is ${shown}, expected ${JSON.stringify(target)}`)
  } else if (isDeepStrictEqual(observed, target)) {
    out.failures.push(`${where}: ${field} reached its target; "${waitsOn}" landed, so drop its today value`)
  } else if (isDeepStrictEqual(observed, today)) {
    out.held.push(`${where}: ${field} waits on "${waitsOn}"`)
  } else {
    out.failures.push(`${where}: ${field} is ${shown}, neither its target nor today's ${JSON.stringify(today)}`)
  }
}

/** Matches `found` disagreements of `check` with the `known` list, one for one. */
function judgeKnown(check, found, known, dir, out) {
  const { unlisted, stale } = matchKnown(
    found.map((d) => ({ dir, ...d })),
    known.map((entry) => ({ dir, ...entry })),
  )
  for (const d of unlisted) out.failures.push(`${dir}/${d.path}: new ${check} disagreement: ${d.kind} ${JSON.stringify(d.item)}`)
  for (const d of stale) {
    out.failures.push(`${dir}/${d.path}: listed ${check} disagreement no longer occurs (${d.kind} ${JSON.stringify(d.item)}); "${d.waitsOn}" landed, so delete it`)
  }
  for (const d of known) out.held.push(`${dir}/${d.path}: ${check} ${d.kind} waits on "${d.waitsOn}"`)
}

/**
 * Judges one case against what Vitest and both engines reported for its directory.
 *
 * @param {{dir: string, expected: object}} oracleCase A case from `readCases`.
 * @param {object} view What was observed.
 * @param {Map<string, {tasks: object[]}>} view.vitest `collectWithVitest`'s result.
 * @param {{compared: string[], disagreements: object[]}} view.located `compareSites`'s result.
 * @param {{disagreements: object[]}} view.titles `compareDirectory`'s result.
 * @param {Map<string, {kind: string, line: number}[]>} view.sites The next engine's sites by file.
 * @param {Map<string, {reasons: string[], testsExcluded: number}>} view.exclusions The next
 *   engine's excluded-file records by file.
 * @returns {{failures: string[], held: string[], files: number}} Failure lines, what is held and
 *   on what, and how many declared files were compared.
 */
export function judgeCase({ dir, expected }, { vitest, located, titles, sites, exclusions }) {
  const out = { failures: [], held: [] }
  const compared = new Set(located.compared)
  const seen = new Set([...vitest.keys(), ...sites.keys(), ...exclusions.keys()])
  for (const path of seen) {
    if (expected.files[path] === undefined) out.failures.push(`${dir}/${path}: collected or analysed, but not declared in expected.json`)
  }
  let files = 0
  for (const [path, entry] of Object.entries(expected.files)) {
    const where = `${dir}/${path}`
    const { waitsOn } = entry
    if (entry.oracle === 'not-collected') {
      if (vitest.has(path)) out.failures.push(`${where}: declared not collected, and Vitest collected it`)
      if (!sites.has(path) && !exclusions.has(path)) out.failures.push(`${where}: declared, but the analyzer reports nothing for it`)
    } else if (!compared.has(path)) {
      out.failures.push(`${where}: declared, but not compared (Vitest could not collect it, or it read a stub)`)
      continue
    } else {
      files += 1
      const tasks = vitest.get(path).tasks.length
      if (tasks !== entry.tasks) out.failures.push(`${where}: Vitest collected ${tasks} tasks, expected ${entry.tasks}`)
      const observed = wrapperSites(sites.get(path) ?? [])
      pin({ where, field: 'wrapperSites', observed, target: entry.wrapperSites, today: entry.wrapperSitesToday, waitsOn }, out)
    }
    pin({ where, field: 'next', observed: exclusions.get(path) ?? {}, target: entry.next, today: entry.nextToday, waitsOn }, out)
  }
  judgeKnown('site', located.disagreements, expected.knownSites ?? [], dir, out)
  judgeKnown('title', titles.disagreements, expected.knownTitles ?? [], dir, out)
  return { ...out, files }
}
