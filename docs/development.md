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

### Gates `npm run check` does not run

They are slow: `check:memory`, `check:memory:scaling`, `check:coverage:rust`, `check:mutation`.
CI runs the first three; `check:mutation` runs weekly from `mutants.yml`. `npm run check:not-run`
prints the current list.

`npm run evaluate:corpus` is a measurement rather than a gate, so it stays out of `npm run check`
as well: it runs every recall case, reports each violation with its denominator instead of
stopping at the first one, and exits 0 on violations unless `--fail-on-violation` (or
`--fail-on-regression`, with `--baseline-binary`) is given. `npm run check:recall` stays the
enforced contract; `fixtures/recall/README.md` documents the corpus and the comparison classes.

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

- The pre-commit hook checks a temporary snapshot of the staged index, so unstaged edits and local
  notes cannot make a bad staged change pass. It runs the fast profile -- the diff-scoped
  docstring check, the static gates, and `cargo fmt --check` when any staged path affects Rust
  (deletions and file-type changes count, including Unicode paths) -- and compiles nothing, so a
  rejected commit costs seconds. Clippy, the Rust suite, the npm gate chain and the Rust coverage
  ratchet run once, at push, in the full profile; both profiles order checks cheapest first and
  stop at the first failure.
- The pre-push hook checks each distinct proposed commit from Git's ref input with the full
  profile; a deletion-only push has no source to check.
- Both hooks reject whitespace errors in the snapshot, and each loads the verifier from the staged
  index or proposed commit, so an unstaged verifier edit cannot change which checks run.

### Snapshots

Snapshots include skip-worktree files omitted by sparse checkout, using their indexed bytes without
changing the checkout's sparse selection or index flags. Exported symlinks must resolve inside the
snapshot; links that escape it, are dangling, or form cycles are rejected. Verification subprocesses
clear source-repository Git paths and command-scoped configuration overrides.

### Installing

Hook installation uses worktree-specific Git configuration and preserves an existing hook manager.
Install again in each worktree where you want the hooks.

They need a POSIX shell, `mktemp`, a Node release `devEngines` admits, npm, Rust, and Git on `PATH`.
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

The pre-push hook checks that proposed commit's coverage too, but hooks are opt-in, so their
existence alone is not evidence that the check ran. New commits invalidate the previous result and
need another check before merge. Coverage floors and scope stay unchanged.

Back to the [documentation index](README.md).
