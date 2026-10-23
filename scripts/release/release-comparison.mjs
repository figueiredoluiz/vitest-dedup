#!/usr/bin/env node
// The release's recall-corpus comparison: runs the candidate build against the previous published
// release's verified binary and turns the result into a section of the release notes.
//
//   node scripts/release/release-comparison.mjs compare --version X.Y.Z --binary FILE --out DIR
//                                                       [--target KEY]
//   node scripts/release/release-comparison.mjs apply --tag vX.Y.Z --section FILE
//
// `compare` finds the newest published release other than this one, downloads its Linux archive,
// checks the archive's digest and build-provenance attestation, runs `evaluate-corpus.mjs` with that
// binary as the baseline, and writes `comparison.json` (the full detail, kept as a workflow
// artifact), `comparison.md` and `notes-section.md`. It fails -- never writes a section that reads
// as a pass -- when the previous binary cannot be fetched, verified, run, or compared. The one
// legitimate absence, a repository with no earlier release, is written as that statement.
//
// `apply` puts the section into the release's notes between markers, replacing any section a
// previous attempt left, so a retried publish keeps exactly one.
import { spawnSync } from 'node:child_process'
import { createHash } from 'node:crypto'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { regressedCases } from '../check/lib/corpus-comparison.mjs'
import { runAsGate } from '../check/lib/gate-main.mjs'

const here = dirname(fileURLToPath(import.meta.url))
const REPOSITORY_ROOT = resolve(here, '..', '..')

/** How many releases to list when looking for the previous one. */
const RELEASE_LIST_LIMIT = 100

/** GitHub's limit on release notes, in characters. */
export const MAX_NOTES_LENGTH = 125000

/** Appended where the generated notes were cut to leave room for the section. */
const TRUNCATION_NOTICE = '\n\n[The rest of the generated notes was cut to fit GitHub\'s size limit.]'

/** The target the comparison runs on: the host the workflow runs on. */
export const COMPARISON_TARGET = 'linux-x64-gnu'

export const START_MARKER = '<!-- vitest-dedup:corpus-comparison:start -->'
export const END_MARKER = '<!-- vitest-dedup:corpus-comparison:end -->'

/** Names the archive a release publishes for `version` and `key`. */
export function archiveName(version, key = COMPARISON_TARGET) {
  return `vitest-dedup-${version}-${key}.tar.gz`
}

/**
 * Picks the release to compare against: the newest published one that is not this release. A stable
 * release is compared with the newest stable one, and with a prerelease only when no stable one exists.
 *
 * @param {Array<{tagName: string, isPrerelease?: boolean, isDraft?: boolean}>} releases Newest first.
 * @param {string} version The version being released, without the leading `v`.
 * @returns {string | null} The tag, or `null` when there is no earlier release.
 */
export function selectPreviousRelease(releases, version) {
  const published = releases.filter(
    (release) =>
      release.isDraft !== true &&
      release.tagName !== `v${version}` &&
      /^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/.test(release.tagName),
  )
  // A stable release is compared with the newest stable one; only when there is none does a
  // prerelease stand in, since an earlier candidate is still the nearest thing users tried.
  const found = version.includes('-') ? published[0] : (published.find((release) => release.isPrerelease !== true) ?? published[0])
  return found?.tagName ?? null
}

/**
 * Reads the digest a `.sha256` file records for `fileName`.
 *
 * @param {string} text The file content (`<hex>  <name>`).
 * @param {string} fileName The archive name it must name.
 * @returns {string} The hex digest.
 * @throws {Error} When the file names another file or holds no digest.
 */
export function expectedDigest(text, fileName) {
  const match = text.trim().match(/^([0-9a-f]{64})\s+\*?(.+)$/)
  if (!match || match[2].trim().split('/').pop() !== fileName) {
    throw new Error(`the checksum file does not record a sha256 for ${fileName}`)
  }
  return match[1]
}

/** Counts a tally of the comparison's classes in one cell-friendly object. */
function figures(comparison) {
  const detector = comparison.totals.detector
  return {
    retained: detector.retained,
    newlySatisfied: detector['newly-satisfied'],
    corrected: detector.corrected,
    regressed: detector.regressed,
    newForbidden: detector['new-forbidden'],
    newUnclaimed: comparison.totals.newUnclaimed,
    extractionRegressed: comparison.totals.extractionRegressed,
    // An evaluator that predates the figure leaves it out; that reads as not measured.
    excluded: comparison.totals.excluded ?? { baseline: 0, candidate: 0, measuredCases: 0, more: [], fewer: 0 },
    unavailable: comparison.unavailableCases,
    changedCases: comparison.labelChangedCases.length,
  }
}

/**
 * A case name that is safe to write into the notes: the corpus is checked-in data, but a name is
 * not validated beyond being a string, and one holding a backtick, a newline or the section markers
 * would break the list item or plant a second section.
 *
 * @param {string} name A case name.
 * @returns {string} The name with every character outside a conservative set replaced by `?`.
 */
export function literal(name) {
  return String(name).replace(/[^A-Za-z0-9._ /-]/g, '?').slice(0, 120)
}

/**
 * The notes section for a comparison.
 *
 * @param {object} options
 * @param {object | null} options.comparison The `comparison` of the evaluator's JSON, or `null` when
 *   there was no earlier release to compare with.
 * @param {string} options.version The version released.
 * @param {string | null} options.previousTag The tag compared with.
 * @param {number} options.cases The number of corpus cases.
 * @param {string} [options.key] The target the binaries were built for.
 * @returns {string} Markdown between the start and end markers.
 */
export function renderSection({ comparison, version, previousTag, cases, key = COMPARISON_TARGET }) {
  const lines = [START_MARKER, '## Recall corpus comparison', '']
  if (comparison === null) {
    lines.push(`There is no earlier release of vitest-dedup to compare v${version} with.`)
  } else {
    const f = figures(comparison)
    lines.push(
      `v${version} against ${previousTag} on the ${cases}-case recall corpus, built for ${key}.`,
      '',
      '| Outcome | Count |',
      '| --- | ---: |',
      `| Expectations retained | ${f.retained} |`,
      `| Expectations newly satisfied | ${f.newlySatisfied} |`,
      `| Expectations corrected (a forbidden or false-positive finding gone) | ${f.corrected} |`,
      `| **Expectations regressed** (a wanted finding lost) | ${f.regressed} |`,
      `| **Expectations newly violated** (a forbidden finding appeared) | ${f.newForbidden} |`,
      `| **New findings the labels do not mention** | ${f.newUnclaimed} |`,
      `| **Cases whose extraction regressed** | ${f.extractionRegressed} |`,
      `| **Cases that leave more tests out of comparison than ${previousTag}** | ${f.excluded.more.length} |`,
      `| Tests left out of comparison, over the ${f.excluded.measuredCases} case(s) measured in both builds (${previousTag} to v${version}) | ${
        f.excluded.measuredCases === 0 ? 'not measured' : `${f.excluded.baseline} to ${f.excluded.candidate}`
      } |`,
      `| Cases ${previousTag} could not be run on | ${f.unavailable} |`,
      `| Cases new or changed since ${previousTag}, counted apart | ${f.changedCases} |`,
    )
    const flagged = regressedCases(comparison)
    if (flagged.length > 0) {
      lines.push('', `Cases with a regression (${flagged.length}):`, '')
      for (const c of flagged.slice(0, 20)) lines.push(`- \`${literal(c.name)}\``)
      if (flagged.length > 20) lines.push(`- and ${flagged.length - 20} more, in the artifact`)
    }
    lines.push(
      '',
      "These counts are over this repository's own labelled fixtures, not a measure of precision on real code.",
      'The full per-case detail is the `corpus-comparison` workflow artifact.',
    )
  }
  lines.push(END_MARKER)
  return `${lines.join('\n')}\n`
}

/**
 * Puts `section` at the top of `body`, replacing any earlier copy between the markers.
 *
 * @param {string} body The release notes as they are.
 * @param {string} section A section from {@link renderSection}.
 * @returns {string} The notes with exactly one section.
 */
export function upsertSection(body, section) {
  const pattern = new RegExp(`${START_MARKER}[\\s\\S]*?${END_MARKER}\\n*`, 'g')
  let rest = (body ?? '').replace(pattern, '').trim()
  const head = `${section.trim()}\n\n`
  // GitHub refuses notes over its size limit, and a refusal after publication would leave none of
  // the section; the generated notes give way instead.
  if (head.length + rest.length + 1 > MAX_NOTES_LENGTH) {
    const whole = rest.endsWith(TRUNCATION_NOTICE) ? rest.slice(0, -TRUNCATION_NOTICE.length) : rest
    rest = `${whole.slice(0, Math.max(0, MAX_NOTES_LENGTH - head.length - TRUNCATION_NOTICE.length - 1))}${TRUNCATION_NOTICE}`
  }
  return `${head}${rest}`.trimEnd() + '\n'
}

/**
 * Runs a command and returns its result, for injection.
 *
 * @param {string} command The program.
 * @param {string[]} args The arguments.
 * @param {object} [options] `spawnSync` options.
 * @returns {{status: number | null, stdout: string, stderr: string}}
 */
export function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', maxBuffer: 256 << 20, ...options })
  if (result.error) throw result.error
  return { status: result.status, stdout: result.stdout ?? '', stderr: result.stderr ?? '' }
}

/** Runs a command that must exit 0, returning its stdout. */
function mustRun(execute, command, args, options) {
  const result = execute(command, args, options)
  if (result.status !== 0) {
    const tail = `${result.stderr}`.trim().split('\n').slice(-6).join('\n')
    throw new Error(`${command} ${args.slice(0, 3).join(' ')} failed (exit ${result.status}): ${tail}`)
  }
  return result.stdout
}

/**
 * Compares the candidate binary with the previous release and writes the outputs.
 *
 * @param {object} options
 * @param {string} options.version The version released.
 * @param {string} options.binary The candidate binary.
 * @param {string} options.out The output directory.
 * @param {string} options.repo `owner/name`.
 * @param {object} [deps] Replaceable for tests.
 * @param {typeof run} [deps.execute] Runs commands.
 * @param {string} [deps.root] The checkout root.
 * @returns {{status: 'compared' | 'no-previous', previousTag: string | null, section: string}}
 * @throws {Error} When the previous release cannot be fetched, verified, run or compared.
 */
export function compare({ version, binary, out, repo, key = COMPARISON_TARGET }, { execute = run, root = REPOSITORY_ROOT } = {}) {
  mkdirSync(out, { recursive: true })
  const releases = JSON.parse(
    mustRun(execute, 'gh', ['release', 'list', '--repo', repo, '--exclude-drafts', '--limit', String(RELEASE_LIST_LIMIT), '--json', 'tagName,isPrerelease,isDraft']),
  )
  const previousTag = selectPreviousRelease(releases, version)
  // A full page with nothing usable in it does not show that no earlier release exists.
  if (previousTag === null && releases.length >= RELEASE_LIST_LIMIT) {
    throw new Error(`none of the newest ${RELEASE_LIST_LIMIT} releases is a published version to compare with, and older ones were not listed`)
  }
  const finish = (comparison, cases) => {
    const section = renderSection({ comparison, version, previousTag, cases, key })
    writeFileSync(join(out, 'notes-section.md'), section)
    writeFileSync(join(out, 'comparison.md'), section)
    return section
  }
  if (previousTag === null) {
    writeFileSync(join(out, 'comparison.json'), `${JSON.stringify({ previousTag: null, comparison: null }, null, 2)}\n`)
    return { status: 'no-previous', previousTag, section: finish(null, 0) }
  }

  const scratch = mkdtempSync(join(tmpdir(), 'release-comparison-'))
  try {
    const previousVersion = previousTag.slice(1)
    const archive = archiveName(previousVersion, key)
    mustRun(execute, 'gh', ['release', 'download', previousTag, '--repo', repo, '--pattern', archive, '--pattern', `${archive}.sha256`, '--dir', scratch])
    const archivePath = join(scratch, archive)
    const recorded = expectedDigest(readFileSync(join(scratch, `${archive}.sha256`), 'utf8'), archive)
    const actual = createHash('sha256').update(readFileSync(archivePath)).digest('hex')
    if (recorded !== actual) throw new Error(`${archive} does not match its published sha256`)
    // The digest travels with the archive and so cannot vouch for it; the attestation is signed by
    // GitHub's identity for the workflow that built it.
    mustRun(execute, 'gh', ['attestation', 'verify', archivePath, '--repo', repo])
    mustRun(execute, 'tar', ['-xzf', archivePath, '-C', scratch])
    const baseline = join(scratch, `vitest-dedup-${previousVersion}-${key}`, 'vitest-dedup')

    const jsonPath = join(out, 'evaluation.json')
    mustRun(
      execute,
      process.execPath,
      [join(root, 'scripts', 'check', 'evaluate-corpus.mjs'), resolve(binary), '--baseline-binary', baseline, '--baseline-ref', previousTag, '--json', jsonPath],
      { cwd: root },
    )
    const evaluation = JSON.parse(readFileSync(jsonPath, 'utf8'))
    if (!evaluation.comparison) throw new Error('the evaluation produced no comparison')
    writeFileSync(join(out, 'comparison.json'), `${JSON.stringify({ previousTag, comparison: evaluation.comparison, work: evaluation.work }, null, 2)}\n`)
    return { status: 'compared', previousTag, section: finish(evaluation.comparison, evaluation.work.cases) }
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
}

/**
 * Writes the section into the release's notes.
 *
 * @param {object} options
 * @param {string} options.tag The release tag.
 * @param {string} options.sectionFile The section to apply.
 * @param {string} options.repo `owner/name`.
 * @param {object} [deps] Replaceable for tests.
 * @param {typeof run} [deps.execute] Runs commands.
 * @returns {string} The notes that were written.
 */
export function apply({ tag, sectionFile, repo }, { execute = run } = {}) {
  const section = readFileSync(sectionFile, 'utf8')
  if (!section.includes(START_MARKER) || !section.includes(END_MARKER)) {
    throw new Error(`${sectionFile} is not a comparison section`)
  }
  const body = JSON.parse(mustRun(execute, 'gh', ['release', 'view', tag, '--repo', repo, '--json', 'body'])).body ?? ''
  const notes = upsertSection(body, section)
  const scratch = mkdtempSync(join(tmpdir(), 'release-notes-'))
  try {
    const file = join(scratch, 'notes.md')
    writeFileSync(file, notes)
    mustRun(execute, 'gh', ['release', 'edit', tag, '--repo', repo, '--notes-file', file])
  } finally {
    rmSync(scratch, { recursive: true, force: true })
  }
  return notes
}

/**
 * Parses `--name value` arguments after the command.
 *
 * @param {string[]} argv The arguments after the script name.
 * @returns {{command: string, options: Record<string, string>}}
 * @throws {Error} On an unknown command, a missing value or an unexpected argument.
 */
export function parseArguments(argv) {
  const [command, ...rest] = argv
  const names = { compare: ['version', 'binary', 'out', 'target'], apply: ['tag', 'section'] }[command]
  if (names === undefined) throw new Error('usage: release-comparison.mjs compare|apply ...')
  const options = {}
  for (let index = 0; index < rest.length; index += 2) {
    const name = rest[index]?.replace(/^--/, '')
    const value = rest[index + 1]
    if (!names.includes(name) || value === undefined || value === '' || value.startsWith('--')) {
      throw new Error(`unexpected or incomplete argument ${rest[index] ?? ''}`)
    }
    options[name] = value
  }
  const missing = names.filter((name) => name !== 'target' && options[name] === undefined)
  if (missing.length > 0) throw new Error(`missing --${missing.join(', --')}`)
  // The workflow validates the version before it gets here; a hand run is held to the same shape,
  // since the value is written into the release notes.
  if (command === 'compare' && !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/.test(options.version)) {
    throw new Error('--version must look like 1.2.3 or 1.2.3-rc.1')
  }
  if (command === 'apply' && !/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.]+)?$/.test(options.tag)) {
    throw new Error('--tag must look like v1.2.3')
  }
  return { command, options }
}

/**
 * Runs the command named by `argv`.
 *
 * @param {string[]} argv The arguments after the script name.
 * @param {object} [environment] Process environment.
 * @param {object} [deps] Replaceable for tests; passed to {@link compare} or {@link apply}.
 * @returns {{status: number, lines: string[]}}
 * @throws {Error} On bad arguments or a failed comparison, which the entry point reports as a failure.
 */
export function runCli(argv, environment = process.env, deps = {}) {
  const { command, options } = parseArguments(argv)
  const repo = environment.GH_REPO ?? environment.GITHUB_REPOSITORY
  if (!repo) throw new Error('GH_REPO is required')
  if (command === 'compare') {
    const result = compare({ version: options.version, binary: options.binary, out: options.out, repo, key: options.target }, deps)
    return { status: 0, lines: [`Comparison ${result.status}${result.previousTag ? ` against ${result.previousTag}` : ''}.`] }
  }
  apply({ tag: options.tag, sectionFile: options.section, repo }, deps)
  return { status: 0, lines: [`Comparison section applied to ${options.tag}.`] }
}

runAsGate(import.meta.url, runCli, 'release comparison')
