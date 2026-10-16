#!/usr/bin/env node
// Test-coverage ratchet, per module, for both languages in this repository.
//
// Usage: `node scripts/check/check-coverage.mjs [--rust] [--js]` (neither flag means both).
//
// ## The destination, and why this is not it
//
// The owner's bar is **above 98%**. Measured 2026-09-18: Rust is 87.84% line / 86.24% region /
// 89.31% function, and the JavaScript had **no coverage measurement at all** -- not a low number,
// an absent one. Pinning this at 98% today would fail on its first run and be routed around
// within a week.
//
// So, like `check-duplication.mjs`, this is a ratchet: the floors in `RUST_MODULES` and
// `JS_MODULES` are what was measured when each module was added, tightening one is a one-line
// edit, and the target is recorded next to them so the direction is not in doubt. Closing the gap
// to the target is tracked in the open issues, not here.
//
// ## Per module, not just a total
//
// A total is the wrong unit for this project. `frontend/fingerprint.rs` is the weakest module at
// 71.3% and it is also where the last four adjudicated false positives lived -- so it is
// simultaneously the least-tested and the most-consequential file in the tree. A single total
// would let it slide from 71% to 60% while `main.rs` picking up a few tests held the average
// still. Every module carries its own floor; a regression names the file.
//
// ## Why nothing is excluded, which is a decision and not an omission
//
// Some of the uncovered code is deliberately unreachable in a test: the recursion depth guards in
// `fingerprint.rs`, the `degraded` fallbacks, match arms that exist so the compiler proves the
// match is total. The tempting move is an exclusion list. It is refused here, for a reason this
// repository already holds elsewhere: an exclusion list has to be maintained by whoever is trying
// to make the number go up, and the cheapest way to satisfy a coverage floor over a depth guard is
// a test asserting *that the guard exists* rather than that it works. That shape is treated as a
// defect here, not a win -- it is the same shape as a fix whose only test is the bug it fixed --
// and a gate that rewards it is a net loss even if the percentage improves.
//
// The floors absorb it instead. Each floor is the value measured *with* the defensive code
// counted as uncovered, so the gate never asks anyone to cover it -- it only asks that the module
// not get worse. When a module's floor is eventually raised toward 98%, the conversation about
// which lines are genuinely unreachable happens then, in a diff, with a human deciding -- not
// silently, in a list, today.
//
// Two measurement caveats, recorded because a number whose caveats are not written down becomes a
// claim nobody can check:
//
//   1. **Rust: inline `#[cfg(test)] mod tests` bodies are counted.** `cargo-llvm-cov` excludes
//      whole test-only files (`src/analysis/tests.rs` does not appear in its report) but has no
//      region-level notion of `#[cfg(test)]`, so the instrumented test functions inside a
//      production file count as covered lines. `reporters/html.rs` is 909 lines of which 540 are
//      its test module, and llvm-cov counts 608 instrumented lines for it. The reported figure is
//      therefore *higher* than production-only coverage would be, by a factor that varies per
//      module. It is stable per module, so the ratchet still works; the absolute number is
//      optimistic and should not be quoted as production coverage.
//   2. **JS: Node's coverage denominator is every line of the file**, blank and comment lines
//      included, where llvm-cov counts only instrumented lines. The two percentages are not
//      comparable to each other. Each is comparable to its own history, which is what a ratchet
//      needs.
//
// ## This gate asserts a floor on work performed
//
// A coverage run that executes no tests reports no coverage, and "no report" is one bad `catch`
// away from printing as a pass. Both halves therefore assert:
//
//   - the report file exists and parses (a missing report is a failure, never 0%);
//   - the test run actually ran tests, against a floor on the count parsed out of its own output;
//   - **every module named in the thresholds table appears in the report**. A module vanishing
//     from the report is the silent failure mode that matters here: it looks exactly like the
//     module being fine.
//   - no production source file is unaccounted for -- every file matched by the scope globs must
//     either carry a floor or be named in `JS_NOT_MEASURED` with a reason. A new file cannot
//     arrive uncovered and unnoticed.
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import * as fs from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..')

// `devEngines.runtime` in `package.json` mirrors Vitest 5.0.3's own `engines` range (Node 22.12+,
// 24 or 26+): Vitest is the differential-title harness's runtime oracle, and a developer Node it
// does not support must fail `npm ci` by name rather than run the harness on it. `engines.node`
// stays at 20 for the published launcher, which needs none of this and never installs
// devDependencies. This gate's own floor is 22.5 — what `fs.globSync` needs — which that range
// implies. Asserted here too so a
// direct `node scripts/check/check-coverage.mjs` on an older runtime names the version instead of
// dying on `globSync is not a function` or on an unrecognized `--test-coverage-exclude`.
const NODE_FLOOR = [22, 5]
{
  const [major, minor] = process.versions.node.split('.').map(Number)
  if (major < NODE_FLOOR[0] || (major === NODE_FLOOR[0] && minor < NODE_FLOOR[1])) {
    console.error(
      `this gate needs Node >= ${NODE_FLOOR.join('.')} (fs.globSync, --test-coverage-exclude); ` +
        `this is ${process.versions.node}. package.json's devEngines records a range that implies it.`,
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

const TARGET_PERCENTAGE = 98 // The owner's bar. Not yet reachable; MINIMUM ratchets toward it.

// Drift allowance, applied to every floor. A Rust toolchain bump changes which lines LLVM
// considers instrumented, and a Node bump changes its line accounting, so a zero-slack floor
// would go red on a dependency update rather than on a regression. 0.2pp of the largest module
// here is about five lines: too small to hide a real regression, large enough to absorb a
// retooling. If a floor needs more than this, lower the floor by hand with a note -- do not widen
// the allowance.
const TOLERANCE_PP = 0.2

// ---------------------------------------------------------------------------------------------
// THE RATCHET. Line-coverage floors are whole percentages, rounded down from the measurement.
// To tighten: raise one number.
// ---------------------------------------------------------------------------------------------
const RUST_TOTALS = { lines: 96, regions: 96, functions: 95 }
const RUST_MIN_TESTS = 600 // 676 today (565 lib + 51 bin + 45 matrix + 15 contract).
const RUST_MIN_INSTRUMENTED_LINES = 12_000 // 15,261 today.

const RUST_MODULES = {
  'src/analysis.rs': 84,
  'src/analysis/pairs.rs': 100,
  'src/cli.rs': 98,
  'src/config.rs': 98,
  // Extracted CLI rendering: measured at 98.72% (308/312 instrumented lines).
  'src/diagnostics.rs': 98,
  'src/discovery.rs': 94,
  'src/frontend.rs': 100,
  'src/frontend/extract.rs': 98,
  // First-party imported registration frames: first measured at 93.01% (399/429 lines).
  'src/frontend/extract/imported.rs': 93,
  // Formerly the weakest module at 71.3%, and the one the last four adjudicated false positives
  // lived in. Raised after a survivor-driven pass over the 466 depth-guard and 135 deleted-arm
  // mutants; the documented-unreachable lines (a Windows-only path component, `should not happen`
  // floors, span fallbacks for node kinds the arms above them already total) are counted as
  // uncovered here, never excluded. The figure below is the one llvm-cov reports, which per
  // caveat 1 also counts this file's inline test module.
  //
  // Re-measured 2026-09-25: 98.29% reported (3852/3919), 98.39% production-only -- 29 of its
  // 1796 production lines uncovered, every one of them a defensive floor its own comments name
  // as unreachable (verified case by case, including `ModuleExportName::IdentifierReference`,
  // which oxc's parser marks `unreachable!()` for an import specifier). Closing it needs
  // production lines covered, not more tests: a table-driven test adds mostly uninstrumented
  // data lines, so volume alone moves this figure by hundredths.
  'src/frontend/fingerprint.rs': 98,
  // Raised 2026-09-25 from 97.6, measured 97.88% (2309/2359). Production-only coverage is 98.45%
  // -- 13 of its 841 production lines uncovered, enumerated rather than excluded:
  //
  //   - 2 AST variants the parser never produces in this position (an `ImportSpecifier` whose
  //     `imported` is a `ModuleExportName::IdentifierReference`, which oxc's own parser marks
  //     `unreachable!()`; a namespace specifier whose symbol failed to bind).
  //   - 6 type re-checks whose caller already matched the shape they re-test
  //     (`is_namespace`, `literal_table_rows`, `inherits_default_string_coercion`,
  //     `array_elements`, `object_fields`, `fold_signed_number`).
  //   - 3 recursion-depth guards (`render_coerce`, `render_json`, `fold_template`) that the
  //     renderer calling them refuses one level earlier, so the guard is never the one that
  //     fires. The two bounds that ARE reachable -- the composite fold and the ToString
  //     renderer -- are asserted at the edge, readable side included, rather than left to a
  //     test that only proves a guard exists.
  //   - 1 template-arity invariant a parsed template cannot violate, and 1 `%j` fallthrough
  //     reachable only for values `%s` has already refused.
  //
  // The gap between 98.45% and the figure below is the never-taken `panic!` arms in this file's
  // own test helpers, which caveat 1 makes visible.
  // Imported-helper resolution. What stays uncovered is unreachable by construction: a `let-else`
  // after an `is_none` check, and a default-export function-expression arm the parser's
  // paren handling never produces.
  'src/frontend/helpers.rs': 98,
  'src/frontend/registry.rs': 97,
  'src/frontend/resolver.rs': 100,
  'src/main.rs': 97,
  // New in this PR (commit a84a42d) and initially landed with no floor at all, which is what
  // caught it: the module added functions the tests did not reach, and `rust total functions`
  // dropped to 95.10%, below the 95.4% floor. Raised by testing what each `HeapBytes` impl
  // *computes* -- every `Modifier`/`EachRows`/`MockFactory`/`FixtureValue` variant's arm, the
  // `Arc<str>` apportionment through `MockedModule`, and `FieldBytes`/`ContextFieldBytes` summing
  // exactly to their totals -- rather than asserting any of them merely returns. Functions are
  // 100% (49/49) after that pass. Two of those functions are exercised only directly, not from
  // any production call site, and are tested anyway rather than left unmeasured:
  // `Context::heap_bytes` (production reaches `ContextFieldBytes::of` directly, bypassing the
  // trait method -- the direct test pins the two paths agreeing) and the `usize` arm of
  // `inline_only!` (no field in today's IR is a bare `usize`; every collection reports capacity
  // through arithmetic that never calls `.heap_bytes()` on the count itself).
  'src/memory.rs': 98,
  'src/model.rs': 99,
  'src/modes.rs': 97,
  // `src/reporters.rs` is the module root: the `Reporter` trait, the dispatch table, and the shared
  // test-support constructors. It was in the coverage report with no floor here, which this gate
  // refuses by design -- an unlisted module is retention nobody budgeted, the same argument
  // `check-memory.mjs` makes about an unlisted context field. Recorded at its measured 100.0%, so it
  // cannot drift down unnoticed.
  'src/reporters.rs': 100,
  'src/reporters/html.rs': 99,
  'src/reporters/json.rs': 98,
  'src/reporters/jsonl.rs': 99,
  'src/reporters/sarif.rs': 99,
  'src/reporters/shared.rs': 100,
  'src/reporters/terminal.rs': 96,
  // Raised together after a pass driven by llvm-cov's own uncovered-line list rather than by
  // guesswork, so what is left is named rather than merely unmeasured. Measured by this gate at
  // 97.04% (885/912) and 98.89% (2494/2522).
  //
  // `vitest_config.rs`: 97.64% of its PRODUCTION lines. The 12 that are not covered are two
  // blocks that no input can reach. `detect_globals`'s `TestObject::Malformed` arm needs a
  // `test: {` whose `}` is missing, but that arm only runs inside an object literal whose own
  // closing brace has already been matched, and brace depth cannot fall from that literal's
  // opening level to zero without passing through the level that closes the inner `test: {` on
  // the way -- so `locate_test_object` always answers `Found` there. (The arm itself is pinned by
  // a direct call in `locate_test_object_reports_malformed_when_the_test_brace_never_closes`.)
  // `read_bounded`'s post-read length check is the losing side of a deliberate stat/read race: it
  // fires only if the file grows between `metadata()` and `read_to_end`, which no deterministic
  // test can arrange. The remaining gap to the reported figure is this file's own inline test
  // module -- 12 never-taken `panic!` arms in tests that predate this pass, exactly the effect
  // caveat 1 above describes.
  'src/vitest_config.rs': 97,
  // `vitest_projects.rs`: 99.83% of its PRODUCTION lines. The two that are not covered are the
  // `MAX_GLOB_WALK_ENTRIES` break in `expand_project_path` (20,000 filesystem entries to trip,
  // which is not worth putting in a unit test) and the `i >= element.end` guard at the top of
  // `resolve_project_entry`, which re-does a `skip_trivia` that `split_top_level` has already
  // used to filter the element out -- it cannot be true.
  'src/vitest_projects.rs': 98,
}

// Re-pinned 2026-09-19, after `check-mutation-score.mjs` and then the memory gates' five modules
// joined the table. It went DOWN, from 89.4 to 86.7, because two of the five are gate entry
// points whose measurement paths need a release binary -- a lower total over more measured files
// is the honest direction here, and the alternative was leaving them in JS_NOT_MEASURED where
// their real coverage would not have counted at all. The note this replaces deferred the move until the concurrent change landed;
// it has, so the floor is measured rather than deferred again. Every module in the table also
// carries its own floor, so the total is a backstop and not the only thing guarding anything.
//
// Re-pinned 2026-09-20 to 88.2, up from 86.7, when `check-doc-accuracy.mjs` joined the table at
// 95.9%. A module that arrives ABOVE the total has to move the total with it, or the backstop
// silently acquires slack -- the whole set could then regress by the difference and still pass.
// Re-pinned 2026-09-27 to 83.2, DOWN from 88.2 -- the one direction this ratchet is not supposed
// to move, so the reason is recorded here rather than only in a commit message. Nothing became
// less tested. `lib/fixture-directives.test.mjs` landed and measured two files that had been
// exempt in JS_NOT_MEASURED: `lib/fixture-directives.mjs` (97.4%, fine) and `lib/isolation.mjs`
// (48.6%, not fine). Adding a large poorly-covered file to the denominator moved the average.
// The exemption was hiding it; the floor now shows it. `lib/isolation.mjs` is the debt this step
// creates and it is the next thing to raise -- when it does rise, this total must be re-pinned
// upward in the same commit.
const JS_TOTAL = 83
const JS_MIN_TESTS = 100 // 235 today across the 15 enumerated suite files.

const JS_MODULES = {
  'scripts/dev/verify.mjs': 100,
  // Action pin gate measured 67/67 lines with owned YAML cases.
  'scripts/check/check-action-pins.mjs': 100,
  'scripts/check/check-docstrings.mjs': 100,
  // Both were in JS_NOT_MEASURED as "reached only through a gate that needs a release binary".
  // `lib/fixture-directives.test.mjs` (2026-09-27) made that false for the parser, and pulled
  // `lib/isolation.mjs` in with it. Floors are set at the measured value, as ratchets. The
  // parser is the gate for every fixture in the corpus, so its floor is deliberately high;
  // isolation.mjs is the dimension-checking half of a gate whose other half still needs the
  // binary, which is why it sits under 50%.
  'scripts/check/lib/fixture-directives.mjs': 97,
  'scripts/check/lib/isolation.mjs': 48,
  '.github/actions/vitest-dedup/index.js': 85,
  'npm/lib/launcher.mjs': 98,
  'npm/lib/targets.mjs': 100,
  // The two memory gates are entry points that need a release binary, so most of each file is a
  // measurement path no unit test can reach. What IS covered is the part that runs before the
  // binary is resolved -- argument handling -- which `lib/memory-record.test.mjs` spawns both
  // scripts to exercise, because an unknown flag silently ignored is its own defect. They are in
  // this table rather than in JS_NOT_MEASURED because the coverage is real; the floors are low
  // and honest rather than absent and invisible.
  'scripts/check/check-memory-scaling.mjs': 54,
  'scripts/check/check-memory.mjs': 65,
  // The documentation-accuracy gate. Unlike the two memory gates above it is a pure reader --
  // no binary, no corpus, no spawn -- so its own suite reaches nearly all of it, and the
  // floor is correspondingly high rather than honestly low.
  'scripts/check/check-doc-accuracy.mjs': 95,
  'scripts/check/check-mutation-score.mjs': 98,
  // Both modules arrived in the public-repository tree with dedicated suites but no coverage
  // policy entry. The secret scanner's suite was also omitted from this enumerated run. Register
  // the measured floors rather than leaving every unrelated change behind a red completeness gate.
  'scripts/check/check-secret-scan.mjs': 82,
  'scripts/check/lib/external-corpus-retention.mjs': 98,
  // The attribution-leak gate. A pure reader like the two above, and the floor is 100 because this
  // one has a trap worth recording: its name sweep only runs where the untracked benchmark manifest
  // is on disk, so the first floor here was measured at 88.6% on a machine that HAD the manifest and
  // CI -- which does not -- reached 84.2% and failed. The number was real and its scope was not
  // checked. The fix was not a lower floor but a test that builds a manifest in a temporary root, so
  // both branches are driven wherever the suite runs and the two environments agree.
  'scripts/check/check-name-leak.mjs': 100,
  // The entry point every pure-reader gate shares. Its whole body is the three decisions a gate
  // would otherwise each make alone -- a throw is a failure, failures go to stderr, an imported
  // module does nothing -- so its own suite drives all of them with the streams injected.
  'scripts/check/lib/gate-main.mjs': 100,
  'scripts/check/lib/js-tokens.mjs': 87,
  'scripts/check/lib/memory-corpus.mjs': 97,
  'scripts/check/lib/memory-record.mjs': 94,
  // Reached only as a dependency of `memory-record.mjs`, which imports it for the exit-code
  // contract. Its own callers all spawn a release binary, so the covered lines are the parsing
  // and the contract check, not the spawn paths -- it was in JS_NOT_MEASURED until the memory
  // gates started importing it, and the gate caught the stale exemption rather than leaving a
  // hole that is no longer there.
  'scripts/check/lib/run-analyzer.mjs': 58,
  'scripts/check/lib/rust-test-modules.mjs': 97,
  // 79.3, not 81.7, and nothing became less tested: `planChangelog` was removed with the manual
  // changelog, and it was 77 COVERED lines. What remains uncovered is the same CLI entrypoint block
  // as before -- unreachable from an import-based test by construction -- so a fixed numerator over
  // a smaller denominator is a lower percentage. Two guards the deleted tests reached incidentally
  // (the exactly-one-site check, `cargoLockVersion` by name) now have direct tests, which is the
  // part of this drop that was real.
  'scripts/release/bump-version.mjs': 79,
  // The pure release identity guard is imported by recovery-release.test.mjs (15/30 lines).
  // The gh CLI adapter runs as a separate process in the workflow, outside this coverage run.
  'scripts/release/check-recovery-release.mjs': 50,
  'scripts/release/platform-package.mjs': 100,
  'scripts/release/prepare-npm-packages.mjs': 65,
  'scripts/release/publish-npm-packages.mjs': 97,
  // The pure tag/ref/commit guard is exercised by release-context.test.mjs. Its CLI environment
  // adapter runs in the workflow, outside the in-process coverage run (21/36 lines measured).
  'scripts/release/validate-release-context.mjs': 58,
  'scripts/release/verify-npm-binaries.mjs': 76,
  'scripts/release/version-sites.mjs': 97,
}

/**
 * Production JS that the suites below do not load, each with the reason. Named here rather than
 * left out, because "absent from the report" and "fine" print identically -- this is the list that
 * makes the hole countable; closing it is tracked in the open issues.
 *
 * A file that is neither here nor in `JS_MODULES` fails the gate.
 */
const JS_NOT_MEASURED = {
  'npm/bin/vitest-dedup.js': 'six-line shim; exercised only as a spawned process by the install simulation',
  'scripts/check/check-agent-skills.mjs': 'gate entry point, run as a process by CI and never imported',
  'scripts/check/check-coverage.mjs': 'this file',
  'scripts/check/check-duplicates-corpus.mjs': 'gate entry point; needs a release binary',
  'scripts/check/check-duplication.mjs': 'gate entry point; needs jscpd',
  'scripts/check/check-fixture-isolation.mjs': 'gate entry point; needs a release binary',
  'scripts/check/check-recall.mjs': 'gate entry point; needs a release binary, and its assertions are over that binary\'s output rather than over in-process state',
  'scripts/check/check-release-workflow.mjs': 'gate entry point, run as a process by CI',
  'scripts/check/run-action-tests.mjs': 'test-runner wrapper; running it under coverage would recurse',
  'scripts/check/run-external-corpus.mjs': 'needs ~1GB of clones and a local, untracked manifest of the third-party repositories to clone',
  'scripts/check/verify-titles.mjs': 'gate entry point; needs a release binary AND the pinned Vitest devDependency (it spawns both to diff rendered titles against the runtime), and its interesting logic is the spawn sequence rather than in-process state',
  'scripts/check/run-npm-tests.mjs': 'test-runner wrapper; running it under coverage would recurse',
  'scripts/release/check-npm-trusted-publishing.mjs': 'queries the npm registry over the network',
  'scripts/release/check-release-version.mjs': 'gate entry point, run as a process by CI',
}

/** The JS suites the coverage run drives. Enumerated, never globbed -- see `run-npm-tests.mjs`. */
const JS_TEST_FILES = [
  'scripts/dev/verify.test.mjs',
  'scripts/check/check-docstrings.test.mjs',
  ...globSync('npm/test/*.test.mjs', { cwd: ROOT }).sort(),
  ...globSync('scripts/check/lib/*.test.mjs', { cwd: ROOT }).sort(),
  'scripts/check/check-doc-accuracy.test.mjs',
  'scripts/check/check-mutation-score.test.mjs',
  'scripts/check/check-name-leak.test.mjs',
  'scripts/check/check-secret-scan.test.mjs',
  '.github/actions/vitest-dedup/index.test.mjs',
]

const JS_PRODUCTION_GLOBS = [
  'npm/bin/**/*.js',
  'npm/lib/**/*.mjs',
  'scripts/**/*.mjs',
  '.github/actions/vitest-dedup/index.js',
]

/** Normalizes paths from platform-specific coverage reports to the recorded POSIX keys. */
const posix = (path) => path.split(sep).join('/')
/** Aligns numeric coverage columns without changing their values. */
const pad = (value, width) => String(value).padStart(width)
/** Renders a percentage for the table without throwing when the report did not supply one. */
const pct = (value) => (Number.isFinite(value) ? value.toFixed(2) : String(value))

/**
 * Deletes a half's scratch directory, unless `VITEST_DEDUP_KEEP_COVERAGE_LOG` asks to keep it.
 *
 * The env var previously printed a path to a file nothing had written, from a `finally` block that
 * had already deleted the directory it named -- so the one facility for debugging this gate
 * advertised an artefact that never existed. Both halves now write their captured output into the
 * directory and this decides its fate, so the notice is true for the Rust half and the JS half
 * alike.
 */
function keepOrRemove(work, what) {
  if (process.env.VITEST_DEDUP_KEEP_COVERAGE_LOG) {
    console.error(`(kept: ${what})`)
    return
  }
  rmSync(work, { recursive: true, force: true })
}

/** Compares a measured percentage against its recorded floor, tolerance included. */
function checkFloor(failures, label, measured, floor) {
  // Type before floor, and this is the chokepoint every percentage in this file passes through.
  // `undefined + TOLERANCE_PP` and a NaN percentage both compare `false` against the floor, so a
  // report that lost a field, or an lcov line that failed to parse into a number, would satisfy
  // every floor here and the gate would report success over a measurement it never made. That is
  // the same shape `.github/workflows/ci.yml`'s smoke-test floors carried.
  if (!Number.isFinite(measured)) {
    failures.push(
      `${label}: the measured value is ${JSON.stringify(measured)}, not a number. A floor cannot ` +
        'be checked against a non-number, and a comparison that silently returns false is a ' +
        'passing gate over a failed measurement.',
    )
    return false
  }
  if (measured + TOLERANCE_PP < floor) {
    failures.push(
      `${label}: ${measured.toFixed(2)}% is below the recorded floor of ${floor}% ` +
        `(tolerance ${TOLERANCE_PP}pp). Coverage got worse. Add the tests, or -- if the drop is ` +
        'justified -- lower the floor in this script with the reason in the commit message.',
    )
    return false
  }
  return true
}

// ---------------------------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------------------------

/** Runs instrumented Rust tests and checks each measured module against its recorded floor. */
function checkRust(failures) {
  const work = mkdtempSync(join(tmpdir(), 'vitest-dedup-cov-rust-'))
  const reportPath = join(work, 'rust-cov.json')
  const logPath = join(work, 'rust-cov.log')
  try {
    // Output is redirected, not streamed: an instrumented `cargo test` over the oxc dependency
    // tree prints thousands of lines (`AGENTS.md`, bounded command output).
    let log
    try {
      log = execFileSync(
        'cargo',
        [
          'llvm-cov',
          '--all-features',
          '--workspace',
          '--locked',
          '--summary-only',
          '--json',
          '--output-path',
          reportPath,
        ],
        { cwd: ROOT, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 256 * 1024 * 1024 },
      )
    } catch (error) {
      log = `${error.stdout ?? ''}${error.stderr ?? ''}`
      writeFileSync(logPath, log)
      console.error(log.split('\n').slice(-100).join('\n'))
      throw new Error(
        `cargo llvm-cov exited ${error.status}. Last 100 lines above. Install it with ` +
          '`cargo install cargo-llvm-cov` if it is missing.',
      )
    }
    // Written unconditionally, kept only when asked for. On a run that SUCCEEDS this text is
    // available nowhere else -- the failure path prints its last 100 lines and the success path
    // used to discard it -- and a successful run is exactly when it is wanted, since the
    // `RUST_MIN_TESTS` floor below is a count scraped out of it and an operator doubting that
    // count has no other artefact to read.
    writeFileSync(logPath, log)

    // A floor on work performed: the test run must have run tests. A filtered or empty run
    // produces a report whose
    // percentages are meaningless, and the only witness is the runner's own summary.
    const ran = [...log.matchAll(/test result: ok\. (\d+) passed/g)].reduce(
      (sum, match) => sum + Number(match[1]),
      0,
    )
    if (ran < RUST_MIN_TESTS) {
      failures.push(
        `rust: the coverage run executed ${ran} test(s), floor is ${RUST_MIN_TESTS}. ` +
          'A coverage percentage over a suite that did not run is not a measurement.',
      )
    }

    let report
    try {
      report = JSON.parse(readFileSync(reportPath, 'utf8'))
    } catch (error) {
      throw new Error(
        `cargo llvm-cov produced no parseable report at ${reportPath} (${error.message}). ` +
          'A missing report is a failed measurement, never 0% coverage.',
      )
    }
    const data = report.data?.[0]
    if (!data?.totals || !Array.isArray(data.files)) {
      throw new Error('the llvm-cov report carries no totals or file list; nothing was measured')
    }
    // Type before floor: `undefined < RUST_MIN_INSTRUMENTED_LINES` is false, so a renamed field
    // would satisfy this floor and the gate would go on to grade a report it could not read.
    const instrumented = data.totals.lines?.count
    if (!Number.isInteger(instrumented)) {
      throw new Error(
        `the llvm-cov report's totals.lines.count is ${JSON.stringify(instrumented)}, not an ` +
          'integer; this gate and cargo-llvm-cov disagree about the report schema',
      )
    }
    if (instrumented < RUST_MIN_INSTRUMENTED_LINES) {
      failures.push(
        `rust: only ${instrumented} instrumented line(s), floor is ` +
          `${RUST_MIN_INSTRUMENTED_LINES}. The report covers a fraction of the crate.`,
      )
    }

    const measured = new Map(
      data.files.map((file) => [posix(relative(ROOT, file.filename)), file.summary]),
    )

    console.log('\nrust coverage, per module (llvm-cov line coverage)')
    console.log('   line%   floor   covered/lines  module')
    for (const [module, floor] of Object.entries(RUST_MODULES)) {
      const summary = measured.get(module)
      if (!summary) {
        failures.push(
          `rust: module "${module}" carries a coverage floor but does not appear in the report. ` +
            'A module vanishing from a coverage report looks exactly like a module that is fine.',
        )
        continue
      }
      const percentage = summary.lines?.percent
      const ok = checkFloor(failures, `rust ${module}`, percentage, floor)
      console.log(
        `  ${ok ? ' ' : '!'}${pad(pct(percentage), 6)} ${pad(String(floor), 7)} ` +
          `${pad(`${summary.lines?.covered}/${summary.lines?.count}`, 14)}  ${module}`,
      )
    }

    // The reverse direction: a new production module must not arrive without a floor.
    for (const module of measured.keys()) {
      if (!(module in RUST_MODULES)) {
        failures.push(
          `rust: "${module}" is in the coverage report but has no floor in RUST_MODULES. ` +
            'Add it with its measured value so it cannot regress unnoticed.',
        )
      }
    }

    console.log('\nrust totals')
    for (const [kind, floor] of Object.entries(RUST_TOTALS)) {
      const percentage = data.totals[kind]?.percent
      checkFloor(failures, `rust total ${kind}`, percentage, floor)
      console.log(
        `  ${pad(pct(percentage), 6)}% ${kind} (floor ${floor}%, target ${TARGET_PERCENTAGE}%)`,
      )
    }
    console.log(`  ${ran} tests executed`)
  } finally {
    keepOrRemove(work, `cargo llvm-cov output at ${logPath}, report at ${reportPath}`)
  }
}

// ---------------------------------------------------------------------------------------------
// JavaScript
// ---------------------------------------------------------------------------------------------

/** Parses the subset of lcov that Node's reporter emits: `SF:`, `LF:`, `LH:`. */
function parseLcov(text) {
  const files = new Map()
  let current = null
  let found = 0
  let hit = 0
  for (const line of text.split('\n')) {
    if (line.startsWith('SF:')) {
      current = posix(line.slice(3).trim())
      found = 0
      hit = 0
    } else if (line.startsWith('LF:')) {
      found = Number(line.slice(3))
    } else if (line.startsWith('LH:')) {
      hit = Number(line.slice(3))
    } else if (line.trim() === 'end_of_record' && current !== null) {
      files.set(current, { found, hit, percent: found === 0 ? 0 : (100 * hit) / found })
      current = null
    }
  }
  return files
}

/** Runs the enumerated JS suites and checks line coverage without treating missing files as zero. */
function checkJs(failures) {
  const work = mkdtempSync(join(tmpdir(), 'vitest-dedup-cov-js-'))
  const lcovPath = join(work, 'js-cov.lcov')
  const testLogPath = join(work, 'js-test.log')
  try {
    const argv = [
      '--test',
      '--experimental-test-coverage',
      // Only production sources carry floors; the suites themselves are measured by
      // `check-duplication.mjs`'s `js-tests` scope, not by this one.
      '--test-coverage-exclude=**/*.test.mjs',
      '--test-reporter=spec',
      `--test-reporter-destination=${testLogPath}`,
      '--test-reporter=lcov',
      `--test-reporter-destination=${lcovPath}`,
      ...JS_TEST_FILES,
    ]
    let status = 0
    try {
      execFileSync(process.execPath, argv, {
        cwd: ROOT,
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe'],
        maxBuffer: 64 * 1024 * 1024,
      })
    } catch (error) {
      status = error.status ?? 1
    }

    let testLog = ''
    try {
      testLog = readFileSync(testLogPath, 'utf8')
    } catch {
      throw new Error(`the JS test run produced no report at ${testLogPath}`)
    }
    const read = (label) => Number(testLog.match(new RegExp(`^. ${label} (\\d+)$`, 'm'))?.[1] ?? NaN)
    const pass = read('pass')
    const fail = read('fail')
    const skipped = read('skip')
    if (!Number.isFinite(pass) || !Number.isFinite(fail)) {
      throw new Error('could not read a pass/fail summary from the JS coverage run')
    }
    if (status !== 0 || fail > 0) {
      failures.push(
        `js: ${fail} test(s) failed during the coverage run (exit ${status}). Coverage measured ` +
          'against a failing suite is not a measurement.',
      )
    }
    // A floor on work performed. Skipped counts, for the same reason `run-npm-tests.mjs` counts
    // it: the install
    // simulation legitimately skips without a release build and must not drag the floor down.
    const ran = pass + (Number.isFinite(skipped) ? skipped : 0)
    if (ran < JS_MIN_TESTS) {
      failures.push(
        `js: the coverage run executed ${ran} test(s) across ${JS_TEST_FILES.length} file(s), ` +
          `floor is ${JS_MIN_TESTS}.`,
      )
    }

    let lcov
    try {
      lcov = parseLcov(readFileSync(lcovPath, 'utf8'))
    } catch (error) {
      throw new Error(
        `Node produced no parseable lcov report at ${lcovPath} (${error.message}). ` +
          'A missing report is a failed measurement, never 0% coverage.',
      )
    }
    if (lcov.size === 0) {
      throw new Error('the lcov report names no files; nothing was measured')
    }

    console.log('\njs coverage, per module (node --experimental-test-coverage, line coverage)')
    console.log('   line%   floor   covered/lines  module')
    let totalFound = 0
    let totalHit = 0
    for (const [module, floor] of Object.entries(JS_MODULES)) {
      const entry = lcov.get(module)
      if (!entry) {
        failures.push(
          `js: module "${module}" carries a coverage floor but does not appear in the lcov ` +
            'report. A module that stopped being loaded looks exactly like a module that is fine.',
        )
        continue
      }
      totalFound += entry.found
      totalHit += entry.hit
      const ok = checkFloor(failures, `js ${module}`, entry.percent, floor)
      console.log(
        `  ${ok ? ' ' : '!'}${pad(pct(entry.percent), 6)} ${pad(String(floor), 7)} ` +
          `${pad(`${entry.hit}/${entry.found}`, 14)}  ${module}`,
      )
    }

    // Nothing production may be silently absent: every file the scope globs match must either
    // carry a floor or be named, with a reason, in JS_NOT_MEASURED.
    const production = [...new Set(JS_PRODUCTION_GLOBS.flatMap((pattern) => globSync(pattern, { cwd: ROOT })))]
      .map(posix)
      .filter((path) => !path.includes('/node_modules/') && !path.endsWith('.test.mjs'))
      .sort()
    for (const module of production) {
      if (module in JS_MODULES || module in JS_NOT_MEASURED) continue
      failures.push(
        `js: "${module}" is production JavaScript with neither a coverage floor nor an entry in ` +
          'JS_NOT_MEASURED. Measure it, or record why it cannot be measured.',
      )
    }
    for (const module of Object.keys(JS_NOT_MEASURED)) {
      if (lcov.has(module)) {
        failures.push(
          `js: "${module}" is listed in JS_NOT_MEASURED but the run did measure it. Move it into ` +
            'JS_MODULES with its measured value -- the exemption is now a hole that is not there.',
        )
      }
    }

    const total = totalFound === 0 ? 0 : (100 * totalHit) / totalFound
    checkFloor(failures, 'js total', total, JS_TOTAL)
    console.log('\njs totals')
    console.log(
      `  ${total.toFixed(2)}% line over the ${Object.keys(JS_MODULES).length} measured module(s) ` +
        `(floor ${JS_TOTAL}%, target ${TARGET_PERCENTAGE}%)`,
    )
    console.log(
      `  ${Object.keys(JS_NOT_MEASURED).length} production file(s) not measured at all, each with ` +
        'a recorded reason in JS_NOT_MEASURED',
    )
    console.log(`  ${ran} tests executed across ${JS_TEST_FILES.length} file(s)`)
  } finally {
    keepOrRemove(work, `node --test output at ${testLogPath}, lcov at ${lcovPath}`)
  }
}

/** Selects requested coverage gates and reports all floor failures together. */
function main() {
  const argv = process.argv.slice(2)
  const unknown = argv.filter((flag) => flag !== '--rust' && flag !== '--js')
  if (unknown.length > 0) {
    console.error(`unknown argument(s): ${unknown.join(', ')}. Usage: [--rust] [--js]`)
    process.exit(2)
  }
  const wantRust = argv.length === 0 || argv.includes('--rust')
  const wantJs = argv.length === 0 || argv.includes('--js')

  const failures = []
  if (wantJs) checkJs(failures)
  if (wantRust) checkRust(failures)

  if (failures.length > 0) {
    console.error('\nFAIL: coverage gate')
    for (const failure of failures) console.error(`  ${failure}`)
    process.exit(1)
  }
  console.log('\nok: every module measured, and none is below its recorded floor.')
}

if (resolve(process.argv[1] ?? '') === resolve(fileURLToPath(import.meta.url))) {
  try {
    main()
  } catch (error) {
    console.error(`\nFAIL: coverage gate could not complete a measurement.\n  ${error.message}`)
    process.exit(1)
  }
}
