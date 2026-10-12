#!/usr/bin/env node
// Code-duplication ratchet. Measures copy-paste duplication with jscpd and fails when it gets
// worse than the recorded number.
//
// ## The destination
//
// The owner's bar is **below 1%**. Nothing measured it until this script existed, and the first
// measurement was nowhere near 1%: a gate pinned at 1% on day one would have been red on its
// first run, and a gate that is red on arrival gets ignored -- which is worse than no gate,
// because it also teaches everyone that this number does not matter.
//
// `rust` has since reached the bar (2.48% -> 0.65%, by de-duplicating `analysis.rs`,
// `vitest_config.rs` and `vitest_projects.rs`) and its threshold is pinned there. The other three
// scopes now meet it too; see `TARGET_PERCENTAGE` and the per-scope history below.
//
// So this is a ratchet: the threshold is the measured value, the gate fails when the number gets
// worse, and tightening it is a deliberate one-line edit to `THRESHOLDS`. The target is recorded
// next to each number so the ratchet has a direction and so nobody mistakes the current value for
// the goal; the de-duplication work itself is tracked separately from this ratchet.
//
// ## Scope, which is the substance of this gate
//
// A duplication percentage is only as meaningful as the set of files it is measured over. Three
// decisions, each argued because each one moves the number a lot:
//
// **`fixtures/` is excluded.** Not a convenience -- counting it would make the number
// meaningless. The fixture corpus is built out of deliberately near-identical twins: that is the
// entire design. `not-duplicates-*` fixtures exist precisely to be one edit away from their
// positive counterpart. Measuring duplication there
// measures how well the corpus was written, in the wrong direction, and no action could ever
// follow from the number. `examples/` is excluded for the same reason.
//
// **`#[cfg(test)] mod tests` is measured as its own scope, not folded into the Rust number.**
// Rust keeps unit tests inline, so `src/` is about half test code by line count, and roughly half
// the duplicated Rust lines were inside those modules. That repetition is partly deliberate:
// this project's testing philosophy prefers an explicit fixture per case over a shared helper
// that hides what the case is, so a test module that reads repetitively is often correct. Folding
// it into one figure produces a number that cannot be acted on -- you cannot tell whether it moved
// because a reporter grew a fourth near-copy of an escaping routine or because someone added
// twelve honest table-driven cases.
//
// jscpd cannot strip those blocks, so `lib/rust-test-modules.mjs` does, by scanning for
// `#[cfg(test)] mod ... { ... }` with comment- and string-aware brace matching, and splitting each
// file into two mirrors under a temporary directory. **What that costs in fidelity**, stated
// plainly:
//
//   1. The mirrors are compacted, so jscpd's line numbers are mirror line numbers. They are
//      translated back through a line map before anything is printed, so the report names real
//      locations -- but if the map were wrong, the numbers would still look plausible. The split
//      asserts a partition invariant (every source line lands in exactly one half) and is unit
//      tested in `lib/rust-test-modules.test.mjs`.
//   2. Removing a block makes the code above and below it adjacent in the token stream, which can
//      in principle manufacture a clone that does not exist in the real file. The gate counts
//      those joins and prints the count; today it is 1 (`analysis.rs`, whose `mod tests;` is a
//      two-line declaration in the middle of the file), so the exposure is one join across 24
//      files rather than something to hand-wave about.
//   3. `#[cfg(test)]` on non-`mod` items is not stripped and counts as production. See the
//      module header of `lib/rust-test-modules.mjs` for why.
//
// The same production/test split is applied to JavaScript, for symmetry and for the same reason:
// `npm/test/` and the action's suite are large, and their repetition answers a different question.
// Both halves are gated. Excluding test code from the production figure does not mean it stops
// being measured -- it means it stops contaminating a number about `src/`.
//
// **The tool is pinned.** `jscpd@4.0.5`, `--min-tokens 50`, `--min-lines 5`. A floating
// `@latest` would let the threshold drift with someone else's tokenizer, and a threshold that
// moves on its own is not a measurement.
//
// ## This gate asserts a floor on work performed
//
// The repository-wide rule: every gate must assert that the work happened, not only that nothing
// failed. A duplication gate is unusually good at silent success, because
// "0.00% duplication" and "jscpd matched no files" print almost the same way. Two of the four
// scopes here would have been measured over a *fraction* of their files without this, and it took
// a floor to notice: jscpd defaults to skipping any source over **1000 lines or 100kb**, silently.
// Twelve of this project's 24 Rust files are over 1000 lines. The first measurement of `src/`
// covered 4,494 of 31,204 lines and reported the result as a percentage, with no warning.
//
// So each scope asserts: the report exists; jscpd analyzed **exactly** the files that were staged
// for it (not "at least"); and the staged file and line counts clear a floor. Any of those failing
// is an error, not a 0%.
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import * as fs from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

import { splitRustTestModules } from './lib/rust-test-modules.mjs'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

// Node 22 for `fs.globSync`, used to stage the scopes below. The development floor lives in
// `package.json`'s `devEngines.runtime`; repeated as an executable check so running this script
// directly on Node 20 says so rather than throwing a TypeError from inside `stage()`.
{
  const major = Number(process.versions.node.split('.')[0])
  if (major < 22) {
    console.error(
      `this gate needs Node >= 22 for fs.globSync; this is ${process.versions.node}. ` +
        "See package.json's devEngines.",
    )
    process.exit(2)
  }
}

// Bound *after* the version guard above, deliberately. `fs.globSync` landed in Node 22, so a named
// static import of it fails at module-link time on Node 20 -- before any code in this file runs,
// including the guard whose entire purpose is to say which version is needed. A namespace import
// links on every version, so the guard speaks first and the diagnostic is reachable on exactly
// the versions it diagnoses.
const globSync = fs.globSync

const JSCPD_VERSION = '4.0.5'
const MIN_TOKENS = 50
const MIN_LINES = 5

// jscpd skips a file over `--max-lines` lines or `--max-size` bytes without saying so. Both are
// raised far past anything in this repository; the per-scope exact file-count assertion is what
// actually guarantees nothing was skipped.
const MAX_LINES = 1_000_000
const MAX_SIZE = '50mb'

// ---------------------------------------------------------------------------------------------
// THE RATCHET. Each `maxPercentage` is the value measured when the scope was added, carried to
// two decimals with no slack: jscpd is pinned and the detection is deterministic, so there is no
// tool drift for a tolerance to absorb. Adding a clone-free file LOWERS a percentage; only new
// duplication raises it.
//
// To tighten: lower one number. That is the whole workflow. Do not raise one without saying why
// in the commit message -- a raised ceiling is the ratchet running backwards.
// ---------------------------------------------------------------------------------------------
// The owner's bar, now reached by all four scopes; their measured ceilings still prevent drift.
const TARGET_PERCENTAGE = 1.0

const THRESHOLDS = {
  // Both production scopes reached 0.00 on 2026-09-30 -- every clone the pinned jscpd
  // configuration can see was removed, so the bar for them is now zero and a ceiling is no
  // longer a ratchet value but a regression. The two test scopes still ratchet toward the bar.
  rust: 0.0, // 2026-09-30: 0 clones, 0 duplicated lines
  // 2026-10-13 public-repository baseline: 223 duplicated lines / 28 clones. This records the
  // imported tree as it exists; reducing the inherited repetition remains follow-up work.
  'rust-tests': 0.65,
  // 2026-10-13 public-repository baseline: 5 duplicated lines / 1 clone. The prior zero ceiling
  // no longer described the imported tree, so it made every unrelated PR fail.
  js: 0.04,
  // 2026-10-13 public-repository baseline: 23 duplicated lines / 3 clones. This is still below
  // the owner's 1% bar and leaves no headroom above the measured value.
  'js-tests': 0.45,
}

/**
 * A scope is a set of files, a jscpd format, and a floor on how much there is to measure.
 *
 * `minFiles` / `minLines` are the work-performed floors: they are deliberately well under the
 * current
 * counts, because their job is to catch a glob that stopped matching, not to track growth.
 */
const SCOPES = [
  {
    id: 'rust',
    what: 'production Rust (src/, `#[cfg(test)] mod` removed)',
    format: 'rust',
    minFiles: 20,
    minLines: 12_000,
  },
  {
    id: 'rust-tests',
    what: 'inline Rust test modules and test-only Rust files',
    format: 'rust',
    minFiles: 18,
    minLines: 10_000,
  },
  {
    id: 'js',
    what: 'production JS (npm/bin, npm/lib, scripts/, the Action)',
    format: 'javascript',
    minFiles: 20,
    minLines: 3_500,
  },
  {
    id: 'js-tests',
    what: 'JS test suites (npm/test/, scripts/check/**/*.test.mjs, the Action suite)',
    format: 'javascript',
    minFiles: 9,
    minLines: 1_500,
  },
]

// Globs are relative to the repository root. `fixtures/` and `examples/` appear nowhere on
// purpose -- see the header.
const RUST_SOURCES = 'src/**/*.rs'
const JS_PRODUCTION = [
  'npm/bin/**/*.js',
  'npm/lib/**/*.mjs',
  'scripts/**/*.mjs',
  '.github/actions/vitest-dedup/index.js',
]
const JS_TESTS = [
  'npm/test/**/*.test.mjs',
  'scripts/check/**/*.test.mjs',
  '.github/actions/vitest-dedup/index.test.mjs',
]

/** A Rust file that is nothing but tests: `foo/tests.rs` or anything under a `tests/` directory. */
const isTestOnlyRustFile = (path) =>
  path.endsWith(`${sep}tests.rs`) || path.split(sep).slice(0, -1).includes('tests')

const expand = (patterns) =>
  [...new Set([patterns].flat().flatMap((pattern) => globSync(pattern, { cwd: ROOT })))]
    .filter((path) => !path.includes(`${sep}node_modules${sep}`))
    .sort()

/** Stages every scope's files into `<stage>/<scope-id>/`, returning the per-file line maps. */
function stage(stageRoot) {
  const staged = new Map(SCOPES.map((scope) => [scope.id, new Map()]))
  let adjacencyJoins = 0
  // Lines in a half too short for jscpd to ever report a clone from (`--min-lines 5`). Staging
  // them would make jscpd drop the file from its statistics -- it has nothing to tokenize -- and
  // the exact file-count assertion below would then fail on a file that never mattered. They are
  // counted and printed rather than silently dropped: the only one today is `analysis.rs`'s
  // two-line `#[cfg(test)] mod tests;` declaration.
  let belowMinLines = 0

  const place = (scopeId, path, text, lineMap) => {
    if (lineMap.length < MIN_LINES) {
      belowMinLines += lineMap.length
      return
    }
    const destination = join(stageRoot, scopeId, path)
    mkdirSync(dirname(destination), { recursive: true })
    writeFileSync(destination, text)
    staged.get(scopeId).set(path, { lineMap, lines: lineMap.length })
  }

  const rustFiles = expand(RUST_SOURCES)
  if (rustFiles.length === 0) {
    throw new Error(`no Rust sources matched ${RUST_SOURCES} under ${ROOT}`)
  }
  for (const path of rustFiles) {
    const source = readFileSync(join(ROOT, path), 'utf8')
    if (isTestOnlyRustFile(path)) {
      const lines = source.split('\n')
      if (lines[lines.length - 1] === '') lines.pop()
      place(
        'rust-tests',
        path,
        source,
        lines.map((_, index) => index + 1),
      )
      continue
    }
    const split = splitRustTestModules(source)
    adjacencyJoins += split.adjacencyJoins
    if (split.production.lineMap.length > 0) {
      place('rust', path, split.production.text, split.production.lineMap)
    }
    if (split.tests.lineMap.length > 0) {
      place('rust-tests', path, split.tests.text, split.tests.lineMap)
    }
  }

  const jsTests = new Set(expand(JS_TESTS))
  for (const [scopeId, patterns] of [
    ['js', JS_PRODUCTION],
    ['js-tests', JS_TESTS],
  ]) {
    const files = expand(patterns).filter(
      (path) => (scopeId === 'js-tests') === jsTests.has(path),
    )
    if (files.length === 0) throw new Error(`no files matched for scope ${scopeId}`)
    for (const path of files) {
      const source = readFileSync(join(ROOT, path), 'utf8')
      const lines = source.split('\n')
      if (lines[lines.length - 1] === '') lines.pop()
      place(
        scopeId,
        path,
        source,
        lines.map((_, index) => index + 1),
      )
    }
  }

  return { staged, adjacencyJoins, belowMinLines }
}

/** Resolves a jscpd command whose version is exactly `JSCPD_VERSION`. */
function resolveJscpd() {
  const attempts = [
    { command: 'jscpd', argv: [] },
    { command: 'npx', argv: ['--yes', `jscpd@${JSCPD_VERSION}`] },
  ]
  for (const attempt of attempts) {
    let version
    try {
      version = execFileSync(attempt.command, [...attempt.argv, '--version'], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim()
    } catch {
      continue
    }
    if (version === JSCPD_VERSION) return attempt
  }
  throw new Error(
    `could not obtain jscpd ${JSCPD_VERSION}. Install it (npm i -g jscpd@${JSCPD_VERSION}) or ` +
      'make npx able to fetch it. The version is pinned on purpose: an unpinned tokenizer would ' +
      "drift this gate's thresholds on its own.",
  )
}

/**
 * Runs jscpd over one staged scope and returns its statistics and clones.
 *
 * Failing to produce a parseable report is an error. That is the work-performed floor again: the
 * alternative --
 * treating a missing or empty report as "no duplication found" -- is how a gate reports 0.00% over
 * nothing.
 */
function measure(jscpd, scope, stageRoot, logPath) {
  const cwd = join(stageRoot, scope.id)
  const outputDir = join(stageRoot, 'reports', scope.id)
  const argv = [
    ...jscpd.argv,
    '.',
    '--min-tokens',
    String(MIN_TOKENS),
    '--min-lines',
    String(MIN_LINES),
    '--max-lines',
    String(MAX_LINES),
    '--max-size',
    MAX_SIZE,
    '--format',
    scope.format,
    '--formats-exts',
    'javascript:js,mjs,cjs;rust:rs',
    '--reporters',
    'json',
    '--output',
    outputDir,
    '--silent',
  ]
  let log = ''
  try {
    log = execFileSync(jscpd.command, argv, { cwd, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  } catch (error) {
    log = `${error.stdout ?? ''}${error.stderr ?? ''}`
    writeFileSync(logPath, log)
    throw new Error(
      `jscpd exited ${error.status} for scope "${scope.id}"; last lines of ${logPath}:\n` +
        log.split('\n').slice(-20).join('\n'),
    )
  }
  writeFileSync(logPath, log)

  const reportPath = join(outputDir, 'jscpd-report.json')
  let report
  try {
    report = JSON.parse(readFileSync(reportPath, 'utf8'))
  } catch (error) {
    throw new Error(
      `scope "${scope.id}": jscpd produced no parseable report at ${reportPath} (${error.message}). ` +
        'A missing report is a failed measurement, never 0% duplication.',
    )
  }
  const statistics = report.statistics?.formats?.[scope.format]
  if (!statistics || !statistics.total) {
    throw new Error(
      `scope "${scope.id}": jscpd reported no statistics for format "${scope.format}". ` +
        'It matched no file it was willing to tokenize, which is a failed measurement, not 0%.',
    )
  }
  // Type before floor. `measurementFloorViolations` asks `total.lines <= 0` and `main` asks
  // `percentage > threshold`; against `undefined` both are false, so a jscpd release that renamed
  // a totals field would sail through every floor and get its number recorded as the new ratchet.
  // Validated here, once, because this is the only place the totals enter the gate.
  // `Number.isFinite` alone is too weak to be a validation here: it accepts `-1`, and a
  // `percentage` of -1 clears every threshold comparison, so an invalid report would produce a
  // *passing* gate and then be recorded as the ratchet. Counts must be non-negative integers and
  // a percentage must be a real percentage; anything else is a failed measurement, not a result.
  const counts = ['lines', 'tokens', 'sources', 'duplicatedLines', 'clones']
  for (const field of counts) {
    const value = statistics.total[field]
    if (!Number.isInteger(value) || value < 0) {
      throw new Error(
        `scope "${scope.id}": jscpd's total \`${field}\` is not a non-negative integer ` +
          `(got ${JSON.stringify(value)}). This gate and jscpd ${JSCPD_VERSION} disagree about ` +
          'the report schema, which is a failed measurement rather than a result.',
      )
    }
  }
  const percentage = statistics.total.percentage
  if (!Number.isFinite(percentage) || percentage < 0 || percentage > 100) {
    throw new Error(
      `scope "${scope.id}": jscpd's total \`percentage\` is not within 0..100 ` +
        `(got ${JSON.stringify(percentage)}). A negative percentage would clear every threshold ` +
        'in this gate, so it is rejected rather than compared.',
    )
  }
  if (statistics.total.duplicatedLines > statistics.total.lines) {
    throw new Error(
      `scope "${scope.id}": jscpd reports ${statistics.total.duplicatedLines} duplicated line(s) ` +
        `out of ${statistics.total.lines} total, which cannot both be true.`,
    )
  }
  return { total: statistics.total, sources: statistics.sources ?? {}, clones: report.duplicates ?? [] }
}

/**
 * The work-performed floor, as a pure function so it can be tested without running jscpd.
 *
 * @returns {string[]} one message per violated floor; empty means the scope really was measured.
 */
function measurementFloorViolations(scope, staged, measured) {
  const problems = []
  const stagedFiles = staged.size
  const stagedLines = [...staged.values()].reduce((sum, file) => sum + file.lines, 0)

  if (stagedFiles < scope.minFiles) {
    problems.push(
      `staged only ${stagedFiles} file(s), floor is ${scope.minFiles}. The glob for this scope ` +
        'matched almost nothing, so any percentage it produced is about almost nothing.',
    )
  }
  if (stagedLines < scope.minLines) {
    problems.push(
      `staged only ${stagedLines} line(s), floor is ${scope.minLines}. See above -- a percentage ` +
        'over a handful of lines is noise wearing a measurement costume.',
    )
  }
  const analyzed = Object.keys(measured.sources).length
  if (analyzed !== stagedFiles) {
    const missing = [...staged.keys()].filter((path) => !(path in measured.sources))
    problems.push(
      `jscpd analyzed ${analyzed} of ${stagedFiles} staged file(s). jscpd SKIPS oversized ` +
        'sources silently (--max-lines / --max-size), so a mismatch means the percentage is ' +
        `computed over a subset. Unanalyzed: ${missing.slice(0, 5).join(', ')}` +
        `${missing.length > 5 ? ` (+${missing.length - 5} more)` : ''}`,
    )
  }
  if (measured.total.lines <= 0 || measured.total.tokens <= 0) {
    problems.push(
      `jscpd counted ${measured.total.lines} line(s) and ${measured.total.tokens} token(s). ` +
        'Zero work was performed.',
    )
  }
  return problems
}

const pad = (value, width) => String(value).padStart(width)

/** The files with duplicated lines, most-duplicated first, capped at five. */
function worstFiles(measured) {
  return Object.entries(measured.sources)
    .filter(([, file]) => file.duplicatedLines > 0)
    .sort((a, b) => b[1].duplicatedLines - a[1].duplicatedLines)
    .slice(0, 5)
}

function main() {
  const jscpd = resolveJscpd()
  const stageRoot = mkdtempSync(join(tmpdir(), 'vitest-dedup-jscpd-'))
  const failures = []
  const rows = []

  try {
    const { staged, adjacencyJoins, belowMinLines } = stage(stageRoot)

    console.log(
      `jscpd ${JSCPD_VERSION} via ${jscpd.command}, --min-tokens ${MIN_TOKENS} ` +
        `--min-lines ${MIN_LINES}. Target for every scope: ${TARGET_PERCENTAGE.toFixed(2)}%.`,
    )
    console.log(
      `Rust test-module removal created ${adjacencyJoins} newly-adjacent production boundary/ies ` +
        "(each one a place a clone could be manufactured; see this script's header), and left " +
        `${belowMinLines} line(s) in halves too short for --min-lines ${MIN_LINES} to reach.\n`,
    )

    for (const scope of SCOPES) {
      const scopeStaged = staged.get(scope.id)
      const measured = measure(jscpd, scope, stageRoot, join(stageRoot, `${scope.id}.log`))
      const problems = measurementFloorViolations(scope, scopeStaged, measured)
      for (const problem of problems) failures.push(`scope "${scope.id}": ${problem}`)

      const threshold = THRESHOLDS[scope.id]
      if (threshold === undefined) {
        failures.push(`scope "${scope.id}" has no recorded threshold in THRESHOLDS`)
      }
      const percentage = measured.total.percentage
      rows.push({ scope, measured, percentage, threshold })

      if (threshold !== undefined && percentage > threshold) {
        failures.push(
          `scope "${scope.id}": duplication is ${percentage.toFixed(2)}%, above the recorded ` +
            `${threshold.toFixed(2)}%. Duplication got worse. Either remove the duplication or, ` +
            'if the increase is justified, raise THRESHOLDS with the reason in the commit message.',
        )
        const worst = worstFiles(measured)
        for (const [path, file] of worst) {
          failures.push(
            `  ${path}: ${file.duplicatedLines} duplicated line(s) in ${file.clones} clone(s)`,
          )
        }
        for (const clone of measured.clones.slice(0, 3)) {
          const first = describe(clone.firstFile, scopeStaged)
          const second = describe(clone.secondFile, scopeStaged)
          failures.push(`  clone (${clone.lines} lines): ${first} <-> ${second}`)
        }
      }
    }

    console.log('scope          files    lines    dup lines   clones   %      threshold')
    for (const row of rows) {
      console.log(
        `${row.scope.id.padEnd(14)} ${pad(row.measured.total.sources, 5)} ${pad(row.measured.total.lines, 8)} ` +
          `${pad(row.measured.total.duplicatedLines, 12)} ${pad(row.measured.total.clones, 8)} ` +
          `${pad(row.percentage.toFixed(2), 6)} ${pad((row.threshold ?? 0).toFixed(2), 10)}  ${row.scope.what}`,
      )
    }

    console.log('\nworst production files (the de-duplication work, in order):')
    for (const scopeId of ['rust', 'js']) {
      const row = rows.find((candidate) => candidate.scope.id === scopeId)
      const worst = worstFiles(row.measured)
      for (const [path, file] of worst) {
        console.log(
          `  ${scopeId.padEnd(5)} ${pad(file.duplicatedLines, 4)} dup line(s) ` +
            `${pad(file.percentage.toFixed(1), 5)}%  ${path}`,
        )
      }
    }
  } finally {
    rmSync(stageRoot, { recursive: true, force: true })
  }

  if (failures.length > 0) {
    console.error('\nFAIL: duplication gate')
    for (const failure of failures) console.error(`  ${failure}`)
    process.exit(1)
  }
  console.log('\nok: every scope measured, and none is worse than its recorded threshold.')
}

/** Renders a jscpd clone endpoint at its real source line, undoing the mirror's compaction. */
function describe(endpoint, scopeStaged) {
  const path = endpoint.name.replace(/^\.[\\/]/, '')
  const file = scopeStaged.get(path) ?? scopeStaged.get(path.split('/').join(sep))
  const map = (line) => file?.lineMap[line - 1] ?? line
  return `${path}:${map(endpoint.start)}-${map(endpoint.end)}`
}

if (resolve(process.argv[1] ?? '') === resolve(fileURLToPath(import.meta.url))) {
  try {
    main()
  } catch (error) {
    console.error(`\nFAIL: duplication gate could not complete a measurement.\n  ${error.message}`)
    process.exit(1)
  }
}
