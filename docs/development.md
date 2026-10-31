# Development

Maintainer reference for building, verifying and landing changes. The short version for
contributors is in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Build and test

```sh
cargo test                  # the Rust suite
npm run check               # npm gates; see the omissions below
npm run test:action          # the Action's logic, no network
```

CI also builds rustdoc with warnings denied, so a broken intra-doc link fails the lint job. Run it locally with
`RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --document-private-items`.

`cargo test --lib` is the fast inner loop. It is not sufficient to trust a change to a rule or to
extraction; see [Fixtures](../CONTRIBUTING.md#fixtures).

Every npm gate that runs the analyzer builds `--release --locked` first, so it never measures a
binary older than the working tree. An up-to-date build is a sub-second no-op.

## Framework seam

Vitest's test API names and default file globs live in `src/framework/vitest.rs`, described as data
(`Dialect`, `FileDefaults`) and reached through `framework::current()`. Its configuration readers
live beside it in `src/framework/vitest/`. Code outside the adapter asks the dialect what a name
means (a test, a hook, a table member) instead of spelling the name. Modules listed as neutral in
`src/framework/tests.rs` may not contain any name the dialect declares as a string literal; the
test names the file and literal it rejects.

### The next core

A second analysis core is being built beside the current one, in layers that each read only the
layer below: `lang` (parsing, scopes, resolution; with `frontend`, the only importer of `oxc_*`),
`inventory` (registration sites by provenance), `normal` (normal forms and identities) and
`corpus` (each test keyed for comparison, or excluded with a reason). No run reaches it yet; unit tests
check every layer, and `normal` has its own mutation scope (`npm run mutants:next-core`).

### Gates `npm run check` does not run

They are slow: `check:memory`, `check:memory:scaling`, `check:coverage:rust`, `check:mutation`.
CI runs the first three; `check:mutation` runs weekly from `mutants.yml`. `npm run check:not-run`
prints the current list.

`npm run evaluate:corpus` is a measurement rather than a gate, so it stays out of `npm run check`
as well: it runs every recall case, reports each violation with its denominator instead of
stopping at the first one, and exits 0 on violations unless `--fail-on-violation` (or
`--fail-on-regression`, with `--baseline-binary`) is given. `npm run check:recall` stays the
enforced contract; `fixtures/recall/README.md` documents the corpus and the comparison classes.

### Collection oracle

`npm run check:oracle` collects every fixture directory and recall case with the pinned Vitest and
compares the result with what the analyzer extracts: test titles per file, and hook declaration
sites. Vitest runs as `vitest list --staticParse=false` with a probe runner, so describe callbacks
run and test and hook bodies do not. An import of a module that does not exist loads as a stub, and
a file whose collection reads a stub, or that imports one only for its side effects, is not
compared. The gate assumes a missing module registers no tests or hooks when it loads.

The same collection checks the next engine's inventory (`VITEST_DEDUP_ENGINE=next`): every test and
suite Vitest collects must have a registration site of its kind on the line Vitest places it on, or
a wrapper call or module load that accounts for it. Sites that register nothing Vitest collected are
counted, not failed. Floors on the files and tasks compared keep a shrunken comparison from passing.

Known disagreements are listed with their
reasons in `scripts/check/collection-oracle-known.json`; the gate fails on a disagreement that is not
listed and on a listed one that no longer occurs, so a fix deletes its entry.

The gate also runs the owned cases in `fixtures/oracle/<dir>/`, each with an `expected.json` that
pins, per file, the tasks Vitest collects, the file's `wrapper` and `hook-wrapper` sites, and what
the next engine excludes. A pin the analyzer does not meet yet is held: `wrapperSitesToday` or
`nextToday` records today's value and `waitsOn` says in plain words what it waits on. A held value
fails the gate the moment the target is met, so the expectation changes with the support; the
`knownSites` and `knownTitles` lists work the same way as the central list. A directory with no
usable `expected.json`, a file it does not declare, and a declared file Vitest cannot compare all
fail, and at least one case file must be compared.

### Hooks-only differential check

`npm run check:hooks-differential` tests the next engine's `hook-wrapper` claim (a helper call that
registers only hooks) against Vitest on generated cases: each known way a helper's call can run a
hidden test, in each way a helper is declared and reached, plus bodies the claim admits. A hidden test
Vitest collects with no site fails, as does an admitted body not read as `hook-wrapper`, a route
Vitest collects nothing from, and a case Vitest cannot collect. Cases the analyzer gets wrong today
are listed with reasons in `scripts/check/hooks-differential-known.json`; one that starts passing
fails, so its entry goes when support lands.

### Next-engine duplicate rules

`npm run check:next-duplicates` runs the analyzer under `VITEST_DEDUP_ENGINE=next` over
`fixtures/duplicates/`, `fixtures/observed/` and `fixtures/mixed/` and judges the rules the
next engine ports (`duplicate-test`, `duplicate-test-name`, `duplicate-each-row`, `duplicate-suite`, `cross-file-duplicate`) pairwise against the fixture
headers: every pair a positive expectation declares must be linked by its rule, no pair declared
silent may be linked, and any other linked pair is a failure until it is adjudicated into a
header or into `scripts/check/next-duplicates-known.json` with a reason. A listed delta that
stops being observed fails, so its entry goes when the header or the engine moves; a rule the
next engine does not port reporting a finding fails outright. Every recall case then runs under
the next engine, and the ported rules' findings are judged against the manifest: a missing, forbidden
or unexpected finding fails until adjudicated. The gate prints how many expectations each rule not
yet ported still holds. Floors on fixtures, cases, tests analyzed and required pairs keep an empty
read from passing.

The experimental name rule compares proven title segments in one file and renders supported
literal table titles. It preserves the corpus's existing exclusions, including empty callbacks and
todos. Unknown title values, unsupported formatting and suite-table titles do not anchor name
findings. Distinct rendered titles across copied tables remain separate findings.

The experimental suite rule compares complete inline blocks with matching literal title paths,
modifiers, options and enclosing setup. It excludes uncertain activation, table-derived suites,
inert suites and blocks containing excluded tests. Suite containers do not increase test counts.
Reported copies suppress cross-copy test and name findings; within-copy and unrelated findings
remain visible. Nested suite copies remain independent suite findings. Mixed child clusters stay
visible when they also contain unrelated or within-copy evidence.

The experimental row rule compares rows within one declaration using proven inputs, rendered
names and equivalent callback/context keys. It supports directly written tables of primitive
values and shallow literal rows. Fixed-parameter arrow callbacks may ignore trailing cells;
rest parameters, destructuring and ordinary functions retain the whole row. Unknown/effectful
cells, nested reference values, suite tables, imported/member constants and unresolved title
formatting remain unsupported. In particular, dollar substitutions, string `%i` conversions and
whole null rows cannot anchor row findings yet. Strings and keys containing the replacement
character are also refused until the owned tree exposes lossless JavaScript code-unit identity. Existing corpus exclusions still apply.

## Writing documentation

The website syncs these pages, so how they are named shows up there.

- **Titles are plain text.** The first heading is the page title and its sidebar label. Give it no
  section number and no inline code, and keep it short. `tests/docs_contract.rs` fails on either.
- **Folders become groups.** A folder's name is displayed as a sidebar group, so name it as a
  readable noun in kebab-case (`vitest-surface`, not `vs` or `part-2`).
- **Every folder has an index page** next to it (`rules.md` for `rules/`) whose first heading names
  the group, and whose table lists the pages in reading order. The website derives the sidebar
  order from that table.
- **Cite pages by name.** Link to "module mocking", not to a section number. The numbers in the
  Vitest surface index exist for code comments; prose does not use them.

## Local verification

```sh
npm run verify:fast          # cheap checks, format, gate helper tests -- no compilation
npm run verify:full          # adds strict Clippy, Rust tests, npm gates and Rust coverage
npm run hooks:install        # opt in for this checkout/worktree
```

The full profile does not run the memory and scaling benchmarks, mutation campaigns, or the inline
CI self-check. Use `npm run check:not-run` and `ci.yml` to select those when a change needs them.
This workflow relaxes no gate threshold or corpus exclusion.

### Docstring guard

Both verification profiles first check documentation on touched named functions with an 80%
minimum. `npm run check:docstrings` runs that check alone.

- Its scope is the diff from the branch's merge base with `origin/main`; commit and push hooks use
  the staged or proposed-commit snapshot.
- It covers named Rust functions and methods, and JavaScript/TypeScript `function` declarations and
  named function expressions, including nested declarations.
- Anonymous callbacks, arrow functions and JS method syntax are outside this named-function check;
  CodeRabbit's separate docstring check remains required.
- JavaScript/TypeScript ranges come from the development-only Babel parser; Rust uses the existing
  comment/string masker. Source files that cannot be parsed fail the check.
- Use attached `///` or `/** ... */` comments to explain what a function guarantees.
- Stage new files before running the check; untracked and ignored files are outside its Git diff.

### Clippy and formatting

Clippy runs `--all-targets --all-features --locked` with `-D warnings`, matching CI. `Cargo.toml`
also rejects debug macros, unfinished `todo!`/`unimplemented!` code, and undocumented unsafe blocks
through that strict invocation. Run `cargo fmt --all` to repair formatting before retrying a failed
check. JavaScript and TypeScript corpus files are allowed to contain invalid or awkward source;
these checks do not lint or reformat them.

### Output equivalence

Refactors and performance changes must not change what the analyzer reports. `npm run
check:equivalence` builds the newest `v*` tag reachable from `HEAD` (or `--ref REF`) and caches it
under `target/equivalence`. It then runs the corpus, recall and fixture-isolation gates and every
Rust test executable with a shim in place of the binary. For each invocation the shim runs both
binaries and compares exit status, stdout, stderr and each file the run writes: the report of every
file reporter and, under `--update-baseline`, the baseline file.

- Only three values are normalized, each in the output it occurs in: the terminal detection time
  and `--debug-memory` `peakRssBytes` on stdout, and the candidate's version string on stdout (the
  banner and `--version` line) and in a SARIF file. Any other difference fails the check.
- A suite that made no invocation fails, and a gate or test that fails against the candidate is
  reported apart from equivalence. `--suites corpus,recall,isolation,rust` selects suites.
- Build the release binary first (`npm run check:equivalence` does). macOS and Linux only; a cold
  baseline build takes minutes.
- A test that resolves the binary at compile time with `env!("CARGO_BIN_EXE_vitest-dedup")` bypasses
  the shim; `assert_cmd`'s `cargo_bin` reads the variable at run time and does not.
- It compares bytes, not expectations. `npm run evaluate:corpus` with `--baseline-binary` answers
  the different question of which labelled expectations a change gained or lost.

## Git hooks

The local hooks select checks from changed paths. CI and the explicit `verify:fast` and
`verify:full` commands keep their existing behavior. Each hook prints its selected scope.

| Changed surface | Pre-commit | Pre-push |
| --- | --- | --- |
| Root README/contributing/changelog/security Markdown, LICENSE, `docs/**/*.md`, `skills/**/*.md` | Static checks | Static checks |
| `.github/actions/vitest-dedup/` | Static checks and Action tests | Same, plus duplication and JS coverage |
| `npm/` | Static checks, npm tests and package validation | Same, plus duplication and JS coverage |
| `scripts/dev/verify.mjs`, its test, `.githooks/pre-commit` and `.githooks/pre-push` | Static checks and developer/check-helper tests | Same, plus duplication and JS coverage |
| Rust, fixtures, dependency manifests, workflows, other paths, or unknown push scope | Existing compilation-free fast checks | Existing full checks, including Rust coverage |

Static checks include touched-function docstrings, Action pins, release workflow/version consistency,
documentation accuracy, repository names and shipped skills. Pre-commit still scans the staged index
for secrets before running the verifier. A mixed change selects all its relevant checks; any path
requiring full verification takes precedence. Selection uses paths, not diff size: a small Rust edit
still selects Rust verification. Markdown documents consumed by runtime contract tests receive static
local checks; those runtime tests remain in CI and `verify:full`.

- Pre-commit classifies and checks the staged index, including deleted paths, both rename sides and
  type changes. Unstaged edits and local notes cannot change the selection or make it pass.
- Pre-push checks each distinct proposed commit from Git's ref input. An existing branch uses the
  delta from the remote head supplied by Git; a new branch uses its merge base with local
  `origin/main`. Thus a docs-only update after already-pushed Rust work does not repeat that Rust
  work locally. Missing baseline objects, unavailable merge bases and non-branch destinations
  require full checks. Multiple destinations for one commit combine their changed paths.
- Deletion-only pushes have no source to check. Both hooks reject snapshot whitespace errors, load
  the verifier from the staged index or proposed commit, and stop at the first selected failure.
  The touched-function documentation gate keeps its cumulative PR scope independently of check
  selection.

### Snapshots

Snapshots include skip-worktree files omitted by sparse checkout, using their indexed bytes without
changing the checkout's sparse selection or index flags. Exported symlinks must resolve inside the
snapshot; links that escape it, are dangling, or form cycles are rejected. Verification subprocesses
clear source-repository Git paths and command-scoped configuration overrides.

### Installing

Hook installation uses worktree-specific Git configuration and preserves an existing hook manager.
Install again in each worktree where you want the hooks.

They need a POSIX shell, `awk`, `mktemp`, a Node release `devEngines` admits, npm, Rust, and Git on `PATH`.
Full verification also needs `cargo-llvm-cov` and LLVM tools, as the coverage gate does, and the
checkout's `node_modules` (`npm ci`), which the snapshot links in for the title harness. The hooks
are local safeguards; CI remains authoritative and Git can bypass them.

## Logs, locks and caches

- Commands run sequentially with complete output saved under the common Git directory's
  `delivery-check-logs/`; a failure prints at most the last 100 lines.
- A repository lock prevents two instances of this runner from sharing Cargo concurrently; other
  Cargo invocations must still be serialized by hand. A killed process can leave
  `delivery-check.lock` behind. Inspect its recorded PID and remove it only after confirming that
  process and the checks it spawned (Cargo, Node) have all exited: the next hook run clears the
  shared export directory, which a surviving check could still be reading.
- By default, worktrees share the main checkout's `target/` and `target/llvm-cov-target/`, and
  explicit Cargo target environment variables are respected by the manual profiles. An existing
  target directory pointing elsewhere is rejected, and a cache nested inside the checkout's
  `target/` is refused before any directory is created; choose `target/` itself or an external
  cache.
- Hook snapshots build in `hook-snapshot/<repository>/` beneath that cache, never in the
  checkout's own cache and never in a configured coverage or separate build directory: a test
  binary bakes the path it was built in, and Cargo would otherwise hand a binary built in a deleted
  snapshot to the next checkout run, or to another clone sharing the cache. Every snapshot of a
  repository is exported to the same private directory under `XDG_RUNTIME_DIR`, or the system
  temporary directory without one, so a reused artifact always points at the current export; the
  export stays outside the checkout because Cargo reads configuration files, and the discovery
  walker ignore files, from every ancestor directory (the walker's `.gitignore` lookup stops at a
  repository root when there is one; other ignore files do not). On a host whose temporary directory is shared,
  another user could hold that name; the run then fails naming the directory, and `TMPDIR` or
  `XDG_RUNTIME_DIR` selects a private one. The first hook run after a fresh clone builds the
  dependency tree into that cache once; a temporary target link lets the corpus gates reuse it on
  later runs.

## Rust coverage before merge

CI must be green. The Rust coverage ratchet runs on pushes to `main` and on manual workflow
dispatches, not on pull requests, to avoid a second instrumented build on every one. So before
merging:

1. Run `npm run verify:full` on the exact reviewed PR head.
2. Record the full commit SHA and the Rust coverage result in the PR description.
3. Verify the main coverage job after merging as well.

The pre-push hook runs Rust coverage when its selected scope requires full checks. A scoped push
does not establish full-head coverage; hooks are opt-in, so their existence alone is not evidence
that a check ran. New commits invalidate the previous result and
need another check before merge. Coverage floors and scope stay unchanged.

Back to the [documentation index](README.md).
