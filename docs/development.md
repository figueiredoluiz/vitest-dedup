# Development

Maintainer reference for building, verifying and landing changes. The short version for
contributors is in [CONTRIBUTING.md](../CONTRIBUTING.md).

## Build and test

```sh
cargo test                  # the Rust suite
npm run check               # npm gates; see the omissions below
npm run test:action          # the Action's logic, no network
```

`cargo test --lib` is the fast inner loop. It is not sufficient to trust a change to a rule or to
extraction; see [Fixtures](../CONTRIBUTING.md#fixtures).

Every npm gate that runs the analyzer builds `--release --locked` first, so it never measures a
binary older than the working tree. An up-to-date build is a sub-second no-op.

### Gates `npm run check` does not run

They are slow: `check:memory`, `check:memory:scaling`, `check:coverage:rust`, `check:mutation`.
CI runs the first three; `check:mutation` runs weekly from `mutants.yml`. `npm run check:not-run`
prints the current list.

## Local verification

```sh
npm run verify:fast          # cheap checks, format, strict Clippy, gate helper tests
npm run verify:full          # also Rust tests, npm gates and Rust coverage
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

## Git hooks

- The pre-commit hook checks a temporary snapshot of the staged index, so unstaged edits and local
  notes cannot make a bad staged change pass. It runs the fast profile and skips Cargo when no
  staged path affects Rust or its coverage gate; Rust deletions and file-type changes count as
  changes, including Unicode paths.
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
  process has exited.
- By default, worktrees share the main checkout's `target/` and `target/llvm-cov-target/`, and
  explicit Cargo target environment variables are respected. Temporary target links let the corpus
  gates use the binary built in that cache, which avoids a cold dependency build per snapshot. An
  existing target directory pointing elsewhere is rejected, and a cache nested inside the checkout's
  `target/` is refused before any directory is created; choose `target/` itself or an external
  cache.

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
