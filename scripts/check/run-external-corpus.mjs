#!/usr/bin/env node
// Runs the analyzer over the pinned public Vitest corpus and compares against recorded baselines.
//
// This is the JSCPD-style external check: rather than only testing against fixtures we wrote --
// which encode the failure modes we already thought of -- it measures the analyzer against real
// code at a pinned commit, so a change in findings is attributable to the analyzer rather than to
// upstream drift.
//
// Usage:
//   node scripts/check/run-external-corpus.mjs [--clone-dir <dir>] [--binary <path>] [--update]
//
// `--update` rewrites the baselines in corpus-projects.json: the recorded test/file/error/warning
// counts, the unresolved-context count, and the finding-identity set (`findingIds`). Only do that
// when a change has been reviewed and is intended: an unexamined baseline update converts a
// regression into the new normal, which is the one thing this check exists to prevent. `--update`
// deliberately never touches `knownTruePositives` -- those are hand-reviewed prose describing
// findings that must keep appearing, not derived data, and a project whose run no longer covers
// one is refused an update (see below) rather than having the regression silently absorbed.
// A failed globals check also preserves the project's entire baseline, including missing figures.
//
// What "drift" means here, and why aggregate counts are not enough:
//
//   Aggregate tests/files/errors/warnings counts can stay identical while the *set* of findings
//   is wholly different (one true positive traded for an unrelated false one), while coverage
//   silently narrows (more tests excluded as unresolved, but the same number of findings), or
//   while a specific finding known to be a true positive quietly disappears (a precision fix that
//   over-corrects). All three would look like a clean, unchanged run under a counts-only
//   comparison. This script therefore also compares:
//
//     - `summary.incomplete` against what the baseline's unresolved-context count implies it must
//       be (a corpus with unresolved tests is incomplete by construction -- see analysis.rs).
//     - the recorded `unresolved` count (tests excluded because their context could not be
//       resolved), read from the summary's `unresolvedTests` field.
//     - finding *identity* -- rule plus each finding's sorted participant locations -- not just
//       how many findings exist, so a wholesale swap of which tests are flagged is visible even
//       when the totals match.
//     - what the run leaves out of comparison: tests and files, in all and by reason, read from the
//       summary's `exclusions` and shown as a share of the analyzed tests. A rise fails as a regression;
//       a fall fails until it is recorded with `--update`, so the ceiling cannot go slack. A baseline
//       without the figures is reported as unrecorded and `--update` records it.
//     - each project's `knownTruePositives`: hand-written prose naming a specific location (or
//       pair of locations) that must still appear in a finding. Wired into a real assertion
//       (`isKnownTruePositiveCovered`) rather than left as documentation, so a precision change
//       that silences one of these is caught even though the finding *count* would go down --
//       which, read as a bare number, looks like an improvement.
//     - retention COVERAGE as a floor (V7): every rule the analyzer implements must be named by
//       at least one entry's trailing `(rule, verdict)` clause, or carry a recorded absence in
//       the manifest's `retentionAbsences` with a reason. A rule with no entry and no absence
//       fails the gate by name -- retention cannot catch a rule going silent if nothing anchors
//       it, and that thinness must be counted, not invisible.
import { execFileSync } from 'node:child_process'
import { existsSync, readFileSync, writeFileSync, mkdirSync } from 'node:fs'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defaultBinary, runAnalyzerJsonl } from './lib/run-analyzer.mjs'
import { compareExcluded, excludedFigures, exclusionsDrifted, recordable, shareText } from './lib/excluded-share.mjs'
import { ALL_RULES } from './lib/fixture-directives.mjs'
import {
  clauseRule,
  supplementalFindingsForRule,
  supplementalRetentionRules,
  uncoveredKnownTruePositives,
  validateAnalysisRecords,
} from './lib/external-corpus-retention.mjs'

const here = dirname(fileURLToPath(import.meta.url))

// The manifest is deliberately NOT tracked. It names public third-party repositories used as a
// precision/recall reference, and those are benchmarks rather than dependencies of this project.
// The harness is ours and stays here; the list of other people's repositories is local. Absence is
// therefore an ordinary state, not a broken checkout, so it gets a message that says what to do
// rather than a stack trace from `readFileSync`.
const manifestPath = join(here, 'corpus-projects.json')
if (!existsSync(manifestPath)) {
  console.error('No external corpus manifest at scripts/check/corpus-projects.json.')
  console.error('')
  console.error('This gate measures retention against pinned third-party repositories, which are')
  console.error('local verification objects rather than tracked ones. It is not on the blocking')
  console.error('path: fixtures/observed/ pins what those repositories already taught, offline and')
  console.error('owned, and `npm run check:corpus` is what has to be green to merge.')
  process.exit(2)
}
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))

// S4/G11: every project must declare `expectedGlobalsDetection` (boolean) -- the config-derived
// globals mode this gate expects the analyzer to reach WITHOUT `--globals`. A missing entry is a
// project whose detection nobody checks, which is the exact silent gap the hand-fed flag used to
// paper over; reject the manifest rather than skipping the project.
//
// The declared mode is *compared against the mode the analyzer actually resolved*, not merely
// quoted back in an error message. Reading it only for prose was the defect a review caught: a
// project whose config implies `globals: true` but whose detection confidently lands on
// `Disabled` leaves `filesGlobalsUndetermined` at zero -- nothing was *undetermined*, it was
// determined wrongly -- so the old check counted that project as verified while never looking at
// `expectedGlobalsDetection` at all. That is precisely the S4 shape this gate exists to catch.
{
  const missing = manifest.projects.filter(
    (project) => !['enabled', 'disabled', 'unknown'].includes(project.expectedGlobalsDetection),
  )
  if (missing.length > 0) {
    console.error(
      `FAIL manifest: ${missing.length} project(s) lack an \`expectedGlobalsDetection\` of ` +
        `"enabled" | "disabled" | "unknown" ` +
        `(${missing.map((project) => project.repo).join(', ')}). Every project's config-derived ` +
        `globals mode must be stated and checked -- the analyzer no longer receives \`--globals\``,
    )
    process.exit(1)
  }
}

// How this gate reads the globals mode the analyzer *resolved*, and why from stderr rather than
// from the report.
//
// The JSONL summary carries no resolved-mode field, so the only surface that states the answer is
// the announcement `main.rs` prints when repository-wide detection lands on `Globals::Enabled`.
// With `--globals` never passed here (see the run below), that announcement is an exact
// biconditional for the resolved mode, not an approximation of it -- every one of the four
// detection outcomes is accounted for:
//
//   GlobalsDetection::Enabled              -> prints this line, resolves Enabled
//   GlobalsDetection::Disabled             -> silent,           resolves Disabled
//   Unknown { "no vitest.config…" }        -> silent,           resolves Disabled
//   Unknown { any other reason }           -> prints a *different* line ("could not determine"),
//                                                               resolves Disabled
//
// So the line's absence is not a conflation of "confidently false" with "no config found": all
// three silent-or-other-line cases resolve to `Disabled`, which is exactly what
// `expectedGlobalsDetection: false` asserts.
//
// Matching on prose is the thing `AnalysisResult::unresolved_tests`' doc comment warns against --
// rewording a human-readable sentence must not silently break a gate. The mitigation is
// `globals_true_announcement_matches_the_string_the_corpus_gate_greps_for` in `src/main.rs`,
// which pins this exact substring in both directions (present under `globals: true`, absent under
// `globals: false`). Change the message and that test fails loudly, here in the repository, rather
// than this gate quietly deciding every project derives `globals: false`.
const GLOBALS_ENABLED_MARKER = 'sets `test.globals = true`'
const GLOBALS_REFUSED_MARKER = 'could not determine `test.globals`'

/**
 * The globals mode the analyzer resolved, as one of three states.
 *
 * Two states was wrong, and the first real corpus run said so. `Unknown` and a confident
 * `Disabled` both resolve to `Globals::Disabled` -- which is why the two-state reading was sound
 * about the MODE -- but they are opposite facts about the run, and the gate's failure message
 * asserted the wrong one of them: "Detection reached an answer and it is the wrong one, so
 * nothing is left `undetermined` to count". For three of the pinned repositories detection had
 * reached no
 * answer at all; it declined, correctly. A gate that misdiagnoses the failure it reports sends
 * the reader after the wrong fix, which is the whole complaint S12 was about one layer down.
 */
function derivedGlobals(stderr) {
  if (stderr.includes(GLOBALS_ENABLED_MARKER)) return 'enabled'
  if (stderr.includes(GLOBALS_REFUSED_MARKER)) return 'unknown'
  return 'disabled'
}

const args = process.argv.slice(2)
const opt = (name, fallback) => {
  const i = args.indexOf(name)
  return i >= 0 ? args[i + 1] : fallback
}
const cloneDir = opt('--clone-dir', '/tmp/vitest-dedup-corpus')
const binary = opt('--binary', defaultBinary())
const update = args.includes('--update')

/**
 * Sums the per-project baselines over exactly the keys `manifest.aggregate` records.
 *
 * `aggregate` is derived data, not an independent measurement: it is the sum of the same six
 * per-project baselines this script compares against, over the same scope. Deriving it here --
 * from the aggregate's own key set, so a key nobody records at the top level is never invented
 * and `findingIds` (an array, not a count) is never summed -- is what lets it be *checked* rather
 * than trusted.
 */
function aggregateOfProjectBaselines() {
  const sum = {}
  for (const key of Object.keys(manifest.aggregate)) {
    sum[key] = manifest.projects.reduce(
      (total, project) => total + (project.baseline?.[key] ?? 0),
      0,
    )
  }
  return sum
}

// Why this check exists, and why the numbers it guards were wrong before it did:
//
// `--update` rewrites `project.baseline` for every drifted project and nothing else -- it has
// never touched `manifest.aggregate`, and no code path ever read `aggregate` back. So the
// top-level totals were a hand-maintained copy of derived data with nothing checking the copy,
// and a re-pin that changed per-project error/warning counts left them behind: at the 2026-09-17
// re-pin the recorded aggregate still said `errors: 31, warnings: 1129` while its own six
// siblings summed to `errors: 25, warnings: 1087`. (`tests`/`files` matched only because that
// particular re-pin did not change them -- the same scope, silently disagreeing about two of its
// four numbers.)
//
// The fix is both halves: `--update` now recomputes the aggregate below, and a plain run refuses
// to proceed when the recorded totals do not equal the sum of their parts. Checked before any
// clone or analyzer run, because it needs neither and a manifest that contradicts itself should
// not cost a full corpus pass to discover.
{
  const derived = aggregateOfProjectBaselines()
  const stale = Object.keys(derived).filter((key) => manifest.aggregate[key] !== derived[key])
  if (stale.length > 0 && !update) {
    console.error('corpus-projects.json: `aggregate` does not equal the sum of its per-project baselines.')
    for (const key of stale) {
      console.error(`  ${key}: aggregate says ${manifest.aggregate[key]}, the ${manifest.projects.length} projects sum to ${derived[key]}`)
    }
    console.error('\n`aggregate` is derived from `projects[].baseline`, never measured separately. Re-run with --update to recompute it.')
    process.exit(1)
  }
}

mkdirSync(cloneDir, { recursive: true })

/**
 * A finding's identity: its rule plus every participating location, sorted so that two runs which
 * agree on *which* tests participate but happened to list them in a different order still compare
 * equal. This is what lets the gate notice a wholesale swap of findings even when the aggregate
 * counts (used alone, see the file header) stay identical.
 */
function findingIdentity(finding) {
  const locations = finding.locations
    .map((location) => `${location.path}:${location.line}:${location.column}`)
    .sort()
    .join(',')
  return `${finding.rule}|${locations}`
}

const driftedProjects = new Set()
let knownTruePositiveRegressions = 0
let globalsDetectionChecked = 0
let globalsDetectionPendingHeld = 0

for (const project of manifest.projects) {
  const name = project.repo.split('/')[1]
  const path = join(cloneDir, name)

  if (!existsSync(path)) {
    console.log(`cloning ${project.repo} …`)
    // A shallow clone cannot check out an arbitrary older commit, so fetch that commit directly.
    execFileSync('git', ['init', '--quiet', path])
    execFileSync('git', ['-C', path, 'remote', 'add', 'origin', `https://github.com/${project.repo}.git`])
    execFileSync('git', ['-C', path, 'fetch', '--quiet', '--depth', '1', 'origin', project.commit])
    execFileSync('git', ['-C', path, 'checkout', '--quiet', 'FETCH_HEAD'])
  }

  const actualCommit = execFileSync('git', ['-C', path, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
  if (actualCommit !== project.commit) {
    console.error(`FAIL ${project.repo}: checked out ${actualCommit}, manifest pins ${project.commit}`)
    driftedProjects.add(project.repo)
    continue
  }

  // No `--globals` here, deliberately. The manifest used to hand-feed the flag from a
  // hand-written `globals` field, so the config-detection path -- the one costing 11.9% of all
  // tests on the 2026-09-27 scan, S4 -- was never exercised by this gate. `expectedGlobalsDetection`
  // below states what the analyzer must now DERIVE on its own, and the assertion beside the run
  // checks it.
  const flags = [path]

  // Exit 0 (clean run) and exit 1 (findings exist) are both legitimate outcomes; anything else --
  // notably exit 2, an operational failure -- must not be mistaken for either. See
  // `lib/run-analyzer.mjs` for why this check lives in a shared helper rather than here.
  let records
  let stderr
  let summary
  let findings
  try {
    ;({ records, stderr } = runAnalyzerJsonl(binary, flags))
    ;({ summary, findings } = validateAnalysisRecords(records))
  } catch (error) {
    console.error(`FAIL ${project.repo}: ${error.message}`)
    driftedProjects.add(project.repo)
    continue
  }

  // What the run leaves out of comparison, in the summary's own words: tests and files, in all and by
  // reason. The unresolved count above moves when a pattern stops being read; these say which
  // pattern and how many files, and they are read from a binary that states them or not at all.
  let excluded
  try {
    excluded = excludedFigures(summary)
  } catch (error) {
    console.error(`FAIL ${project.repo}: ${error.message}`)
    driftedProjects.add(project.repo)
    continue
  }
  const numericKeys = ['tests', 'files', 'errors', 'warnings', 'unresolved']
  // The globals-derivation check (S4/G11): with `--globals` no longer passed, a project whose
  // config implies `globals: true` or `globals: false` must have that mode DERIVED, and derived
  // *correctly*. Two distinct failures, checked separately because they need opposite fixes:
  //
  //   - the derived mode disagrees with `expectedGlobalsDetection` -- detection read the config
  //     and reached the wrong answer. Nothing is undetermined, so no counter moves; the only
  //     downstream symptom is a test-count delta the baseline compare would blame on anything.
  //   - `filesGlobalsUndetermined` is non-zero -- detection could not reach an answer for some
  //     files, leaving them unattributed. A counter, not a guess.
  //
  // Only a project that clears both is counted toward the work-performed floor below.
  {
    const undetermined = summary.incompleteCauses?.filesGlobalsUndetermined
    const describeMode = (state) =>
      ({
        enabled: '`globals: true`',
        disabled: '`globals: false`',
        unknown: 'no single repo-wide answer (correctly refused)',
      })[state]
    const expectedMode = describeMode(project.expectedGlobalsDetection)
    const derived = derivedGlobals(stderr)
    if (
      project.globalsDetectionPending &&
      derived !== project.expectedGlobalsDetection
    ) {
      // A one-way ratchet, `knownMisses`' shape applied to globals detection: the expectation
      // records what the REPOSITORY means, the pending note records that the analyzer cannot
      // reach it yet and why. Held, not passed -- and the moment it starts matching, the branch
      // below fails and demands the note come off, so a fix cannot land unnoticed.
      globalsDetectionPendingHeld += 1
      console.log(
        `note ${project.repo}: globals detection still ${describeMode(derived)} where ` +
          `${expectedMode} is the truth -- ${project.globalsDetectionPending}`,
      )
    } else if (project.globalsDetectionPending) {
      console.error(
        `FAIL ${project.repo}: globals detection now reaches ${expectedMode}, which its ` +
          `\`globalsDetectionPending\` note says it cannot. Delete the note so the expectation ` +
          `becomes a hard assertion`,
      )
      driftedProjects.add(project.repo)
    } else if (derived !== project.expectedGlobalsDetection) {
      // Three outcomes, three diagnoses. The old message assumed the middle one always.
      const diagnosis =
        derived === 'unknown'
          ? 'Detection REFUSED -- it reached no repo-wide answer, so files fall back to the ' +
            'per-project map and anything that map cannot attribute is analyzed as ' +
            '`globals: false`'
          : project.expectedGlobalsDetection === 'unknown'
            ? 'Detection reached a repo-wide answer where none is correct; a monorepo whose ' +
              'projects disagree about `globals` has no single answer, and claiming one scopes ' +
              "the wrong mode over some project's files"
            : 'Detection reached an answer and it is the wrong one, so nothing is left ' +
              '`undetermined` to count'
      console.error(
        `FAIL ${project.repo}: the analyzer derived ${describeMode(derived)} but this project ` +
          `expects ${expectedMode}. ${diagnosis} -- this is the S4 shape, and every test whose ` +
          `ambient declarations hinge on the mode is mis-analyzed by it`,
      )
      driftedProjects.add(project.repo)
    } else if (typeof project.maxGlobalsUndeterminedFiles !== 'number') {
      console.error(
        `FAIL manifest: ${project.repo} lacks a numeric \`maxGlobalsUndeterminedFiles\`; a ` +
          `project whose per-file ceiling nobody records is one whose attribution nobody watches`,
      )
      driftedProjects.add(project.repo)
    } else if (undetermined > project.maxGlobalsUndeterminedFiles) {
      console.error(
        `FAIL ${project.repo}: ${undetermined} file(s) have an undetermined globals mode, above ` +
          `the recorded ceiling of ${project.maxGlobalsUndeterminedFiles}. Attribution got ` +
          `worse: a file the project map cannot place is analyzed as \`globals: false\` whatever ` +
          `its project says (S3/S4)`,
      )
      driftedProjects.add(project.repo)
    } else if (undetermined < project.maxGlobalsUndeterminedFiles) {
      // A ratchet, not a range. Attribution improving is good news the manifest must record, or
      // the ceiling silently becomes slack that a later regression hides inside -- the same
      // reason every other floor in this repository is pinned at its measured value.
      console.error(
        `FAIL ${project.repo}: ${undetermined} file(s) have an undetermined globals mode, BELOW ` +
          `the recorded ceiling of ${project.maxGlobalsUndeterminedFiles}. Attribution improved; ` +
          `lower the ceiling to ${undetermined} so the gain cannot be given back unnoticed`,
      )
      driftedProjects.add(project.repo)
    } else {
      globalsDetectionChecked += 1
    }
  }
  const actual = {
    tests: summary.testsAnalyzed,
    files: summary.filesAnalyzed,
    errors: summary.errors,
    warnings: summary.warnings,
    unresolved: summary.unresolvedTests,
    incomplete: summary.incomplete,
  }
  const base = project.baseline
  // `incomplete` is not stored in the manifest as its own field: it is implied by `unresolved` --
  // analysis.rs sets the result incomplete whenever any test's context could not be resolved -- so
  // the "expected" value is derived from the recorded baseline rather than duplicated as a second
  // number that could drift out of sync with it.
  const expected = { ...base, incomplete: (base.unresolved ?? 0) > 0 }
  const changed = [...numericKeys, 'incomplete'].filter((k) => actual[k] !== expected[k])

  const currentIds = findings.map(findingIdentity).sort()
  let identityNote = null
  let identityAdded = []
  let identityRemoved = []
  let identityDrifted = false
  let identityUnrecorded = false
  if (!base.findingIds) {
    // Not drift. A baseline predating identity tracking has nothing to compare against, and
    // reporting that as "identities changed" told the operator to investigate a change that never
    // happened -- while also making this gate permanently red, since no committed baseline had the
    // field. Report it as the distinct condition it is: the check is unavailable until recorded.
    identityUnrecorded = true
    identityNote = `no finding-identity baseline recorded yet (${currentIds.length} current finding(s)); review them and run --update to record`
  } else {
    const baseSet = new Set(base.findingIds)
    const currentSet = new Set(currentIds)
    identityAdded = currentIds.filter((id) => !baseSet.has(id))
    identityRemoved = base.findingIds.filter((id) => !currentSet.has(id))
    if (identityAdded.length || identityRemoved.length) {
      identityDrifted = true
      identityNote = `finding identity set changed (+${identityAdded.length} -${identityRemoved.length})`
    }
  }

  const entries = project.knownTruePositives ?? []
  let knownTruePositiveFailures = uncoveredKnownTruePositives(entries, findings, ALL_RULES)
  // Counts and identity baselines describe the shipped defaults, so never contaminate them with
  // opt-in findings. A missing named positive gets a separate, targeted run instead. This makes
  // an Off-by-default rule observable without turning its corpus-wide volume into baseline drift.
  for (const rule of supplementalRetentionRules(knownTruePositiveFailures)) {
    const forRule = knownTruePositiveFailures.filter((entry) => clauseRule(entry) === rule)
    let supplementalFindings
    try {
      const supplemental = runAnalyzerJsonl(binary, [path, '--rule', `${rule}=warning`])
      supplementalFindings = supplementalFindingsForRule(supplemental.records, rule)
    } catch (error) {
      console.error(`FAIL ${project.repo}: supplemental retention run for ${rule}: ${error.message}`)
      continue
    }
    const stillMissing = new Set(uncoveredKnownTruePositives(forRule, supplementalFindings, ALL_RULES))
    const checked = new Set(forRule)
    knownTruePositiveFailures = knownTruePositiveFailures.filter(
      (entry) => !checked.has(entry) || stillMissing.has(entry),
    )
  }

  // A rise is a regression and a fall is a gain that must be recorded, or the ceiling becomes
  // slack a later regression hides inside; both are drift. A baseline without the figures is a
  // distinct, reported condition, as an unrecorded finding-identity set is.
  let exclusionChange
  try {
    exclusionChange = compareExcluded(base.excluded, excluded)
  } catch (error) {
    console.error(`FAIL ${project.repo}: ${error.message}`)
    driftedProjects.add(project.repo)
    continue
  }
  // Capture eligibility before ordinary baseline drift is recorded below: reviewed drift may be
  // updated, but a run that failed an independent check must not establish any baseline fields.
  const canUpdate = update && !driftedProjects.has(project.repo) && knownTruePositiveFailures.length === 0
  if (update && driftedProjects.has(project.repo)) {
    console.log(`     refusing to update ${project.repo}'s baseline: a project check failed`)
  }
  const ok =
    changed.length === 0 &&
    !identityDrifted &&
    knownTruePositiveFailures.length === 0 &&
    !exclusionsDrifted(exclusionChange)
  if (ok) {
    console.log(
      `ok   ${project.repo.padEnd(22)} ${actual.tests} tests, ${actual.errors}E ${actual.warnings}W, ${actual.unresolved} unresolved, ${excluded.tests} excluded (${shareText(excluded)}) in ${excluded.files} file(s)`,
    )
    if (exclusionChange.unrecorded) {
      console.log('     note: no exclusion baseline recorded yet; review the figures above and run --update to record')
      if (canUpdate) {
        base.excluded = recordable(excluded)
        console.log('     recorded the exclusion baseline')
      }
    }
    // Counts and known true positives matched, but identity cannot be checked until a baseline
    // exists. Say so on the passing line rather than staying silent: a check nobody knows is
    // unavailable is indistinguishable from one that is passing.
    if (identityUnrecorded) console.log(`     note: ${identityNote}`)
    // Record the identity set even on a passing run. Recording used to live only on the failure
    // path, so once an unrecorded baseline stopped counting as drift, `--update` reported
    // "baselines updated" while writing nothing -- a silent no-op wearing a success message.
    if (canUpdate && identityUnrecorded) {
      base.findingIds = currentIds
      console.log(`     recorded ${currentIds.length} finding identities`)
    }
    continue
  }

  driftedProjects.add(project.repo)
  console.log(`DIFF ${project.repo}`)
  for (const k of changed) console.log(`       ${k}: baseline ${expected[k]} -> now ${actual[k]}`)
  console.log(`       excluded from comparison: ${excluded.tests} of ${actual.tests} tests (${shareText(excluded)}), ${excluded.files} file(s)`)
  if (exclusionChange.unrecorded) console.log('       exclusions: no baseline recorded yet, so they were not compared')
  for (const item of exclusionChange.rose) console.log(`       EXCLUSION REGRESSION: ${item}`)
  for (const item of exclusionChange.fell) console.log(`       exclusion improved, record it with --update: ${item}`)
  if (identityNote) {
    console.log(`       identity: ${identityNote}`)
    for (const id of identityAdded.slice(0, 5)) console.log(`         + ${id}`)
    for (const id of identityRemoved.slice(0, 5)) console.log(`         - ${id}`)
  }
  for (const entry of knownTruePositiveFailures) {
    console.log(`       KNOWN TRUE POSITIVE NO LONGER REPORTED: ${entry}`)
  }

  if (knownTruePositiveFailures.length > 0) {
    knownTruePositiveRegressions += 1
    if (update) {
      console.log(`       refusing to update ${project.repo}'s baseline: a known true positive would be silently dropped`)
    }
  } else if (canUpdate) {
    for (const k of numericKeys) base[k] = actual[k]
    base.findingIds = currentIds
    base.excluded = recordable(excluded)
  }
}

if (update) {
  // Recomputed from whatever the per-project baselines now say -- including the ones this run
  // deliberately left untouched (a project refused an update over a dropped known true positive
  // keeps its old baseline, and the aggregate must keep agreeing with it). This is the half of
  // the fix that keeps the totals from going stale again; the check at the top of this file is
  // the half that notices if they somehow do.
  const derivedAggregate = aggregateOfProjectBaselines()
  const recomputed = Object.keys(derivedAggregate).filter(
    (key) => manifest.aggregate[key] !== derivedAggregate[key],
  )
  manifest.aggregate = derivedAggregate
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
  console.log('\nbaselines updated')
  for (const key of recomputed) {
    console.log(`     aggregate.${key} recomputed to ${derivedAggregate[key]}`)
  }
  if (knownTruePositiveRegressions > 0) {
    console.error(
      `\n${knownTruePositiveRegressions} project(s) have a known true positive that this run no longer reports. ` +
        "Their baselines were left untouched by --update. Fix the regression, or if the change is deliberate and " +
        'reviewed, edit knownTruePositives by hand first.',
    )
    process.exit(1)
  }
}

// Work-performed floor for retention COVERAGE (V7): every rule the analyzer implements must be
// named by at least one `knownTruePositives` entry somewhere in the manifest. Retention exists
// so a rule going silent is caught by a named finding disappearing, not by an aggregate
// percentage moving -- but a rule with no entry at all has no named finding to disappear, and
// its silence is invisible to this gate. The floor makes that thinness a gate failure naming
// the uncovered rules, instead of a gap nobody counts.
//
// Entries name their rule in a trailing `(rule, verdict)` clause; the matcher keys on that
// clause, anchored at the end of the entry, rather than a bare substring, because
// `duplicate-test` is a substring of `duplicate-test-name` and a substring check would
// mis-attribute. Two entries predate the convention and carry no clause, so they count toward
// no rule's coverage -- the floor names them implicitly by leaving their rules uncovered until
// the entries gain the clause. The rule set is the one `check-recall.mjs` uses (`ALL_RULES`).
const implementedRules = [...ALL_RULES]
const retentionProse = manifest.projects
  .flatMap((project) => (project.knownTruePositives ?? []).map((entry) => `${project.repo}: ${entry}`))
// A rule with no entry can ALSO be held by a RECORDED ABSENCE: a per-rule reason, 20+ chars,
// in the manifest's `retentionAbsences`, mirroring `mixedRuleCoverage`'s `cannotCoOccur` --
// "no verifiable true positive exists on this corpus" is a real state for a young corpus, and
// forcing a fabricated entry to satisfy a floor would corrupt the hand-reviewed record. An
// absence is a dated claim to re-take (the B4 re-record is the natural moment), never a
// permanent exemption: the reason must say what was scanned and what would reopen it.
const absences = manifest.retentionAbsences ?? {}
const uncoveredRules = implementedRules.filter((rule) => {
  // `clauseRule` is anchored to the END of the entry, so an incidental mid-prose mention of
  // another rule's clause cannot count as coverage. It also keeps the floor and the per-entry
  // matcher on one grammar, including accepted trailing prose punctuation.
  return !retentionProse.some((entry) => clauseRule(entry) === rule)
})
// The absences are a hand-written map, so they are validated like `mixedRuleCoverage`'s: a key
// naming no implemented rule holds nothing (a typo would leave the rule it meant uncovered with
// no hint the absence exists), and an absence for a rule that has since gained an entry is a
// stale claim -- "no verifiable true positive exists" beside an entry that is one.
const unknownAbsences = Object.keys(absences).filter((rule) => !ALL_RULES.has(rule))
if (unknownAbsences.length > 0) {
  console.error(
    `\nretentionAbsences names ${unknownAbsences.length} rule(s) the analyzer does not ` +
      `implement: ${unknownAbsences.join(', ')}. Implemented: ${implementedRules.join(', ')}.`,
  )
  process.exit(1)
}
const staleAbsences = Object.keys(absences).filter((rule) => !uncoveredRules.includes(rule))
if (staleAbsences.length > 0) {
  console.error(
    `\nretentionAbsences records "no verifiable true positive" for ${staleAbsences.length} ` +
      `rule(s) that now have a knownTruePositives entry: ${staleAbsences.join(', ')}. Delete ` +
      'the absence; an entry and an absence for one rule contradict each other.',
  )
  process.exit(1)
}
// An absence holds only when its reason carries the contract's two clauses in so many words:
// what was SCANNED (`scan`/`scanned`/`found`) and what would REOPEN it (`re-take`). A
// 20-character placeholder is not a reason; the vocabulary check is the executable form of the
// comment above, the same way MARKERS is for the name-leak gate.
const absenceHolds = (reason) =>
  typeof reason === 'string' &&
  reason.trim().length >= 20 &&
  /\b(scan|scanned|found)\b/i.test(reason) &&
  /\bre-?take\b/i.test(reason)
const unheldRules = uncoveredRules.filter((rule) => !absenceHolds(absences[rule]))
if (unheldRules.length > 0) {
  console.error(
    `\n${unheldRules.length} implemented rule(s) have no knownTruePositives entry anywhere in ` +
      'the manifest and no recorded absence that says what was scanned and what would re-take ' +
      'it: ' + unheldRules.join(', ') +
      '. Retention cannot catch these rules going silent. Hand-review findings over the pinned ' +
      'corpus and add entries naming the rule in a trailing `(rule, verdict)` clause -- or, if ' +
      'no verifiable true positive exists, record in retentionAbsences what was scanned and ' +
      'when to re-take the claim.',
  )
  process.exit(1)
}
console.log(
  `retention accounted for every implemented rule (${implementedRules.length} rules: ` +
    `${retentionProse.length} entries, ${uncoveredRules.length} recorded absence(s))`,
)

// The drift exit deliberately sits AFTER the retention floor: a drifted run must still be
// told which rules retention does not cover, because the re-record that resolves the drift is
// exactly the moment to source the missing entries.
if (driftedProjects.size > 0) {
  console.error(`\n${driftedProjects.size} project(s) drifted from baseline. Review before re-running with --update.`)
  process.exit(1)
}

// Work-performed floor for the globals-derivation check: every project in the manifest declares
// `expectedGlobalsDetection`, so every project must have had its derived mode compared against
// that declaration. A `continue` or a reshaped loop that skipped some would otherwise report a
// clean run over a subset -- the silent-success shape every gate in this repository is required
// to refuse.
// Checked OR held by a pending note -- every project must be accounted for one way or the other.
// Counting only the checked ones would fail this floor the moment a legitimate pending note
// exists, which is the wrong reason to go red and would push the next reader to delete the note.
if (globalsDetectionChecked + globalsDetectionPendingHeld !== manifest.projects.length) {
  console.error(
    `\nOnly ${globalsDetectionChecked} checked + ${globalsDetectionPendingHeld} held = ` +
      `${globalsDetectionChecked + globalsDetectionPendingHeld} of ${manifest.projects.length} ` +
      'project(s) had their config-derived globals mode accounted for. A recall over a subset ' +
      'is not a check',
  )
  process.exit(1)
}
console.log(
  `globals detection derived without --globals, and matched \`expectedGlobalsDetection\`, for ` +
    `${globalsDetectionChecked}/${manifest.projects.length} project(s)` +
    (globalsDetectionPendingHeld > 0
      ? `; ${globalsDetectionPendingHeld} held by a \`globalsDetectionPending\` note (listed above)`
      : ''),
)
