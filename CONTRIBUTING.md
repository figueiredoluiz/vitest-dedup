# Contributing to VitestDedup

## Setup

Rust stable and a Node release that `package.json`'s `devEngines` admits: 22.12.x, 24.x, or 26
and later, the range the pinned Vitest supports. CI uses Node 24. The published launcher supports
Node 20+.

```sh
npm install
cargo build
```

## Build and test

```sh
cargo test                  # the Rust suite
npm run check               # npm gates; see the omissions below
node --test .github/actions/vitest-dedup/   # the Action's logic, no network
```

`cargo test --lib` is the fast inner loop. It is not sufficient to trust a change to a rule or to
extraction; see [Fixtures](#fixtures).

Every npm gate that runs the analyzer builds `--release --locked` first, so it never measures a
binary older than the working tree. An up-to-date build is a sub-second no-op.

Gates `npm run check` does not run, because they are slow:
`check:memory`, `check:memory:scaling`, `check:coverage:rust`, `check:mutation`.
CI runs the first three; `check:mutation` runs weekly from `mutants.yml`. `npm run check:not-run`
prints the current list.

## Local verification and Git hooks

```sh
npm run verify:fast          # cheap checks, format, strict Clippy, gate helper tests
npm run verify:full          # also Rust tests, npm gates and Rust coverage
npm run hooks:install        # opt in for this checkout/worktree
```

Both verification profiles first check documentation on touched named functions with an 80%
minimum. `npm run check:docstrings` runs that check alone. Its scope is the diff from the branch's
merge base with `origin/main`; commit and push hooks use the staged or proposed-commit snapshot.
The local check covers named Rust functions/methods and JavaScript/TypeScript `function`
declarations and named function expressions, including nested declarations. Anonymous callbacks, arrow functions and JS method
syntax are outside this named-function check; CodeRabbit's separate docstring check remains required.
JavaScript/TypeScript ranges come from the development-only Babel parser; Rust uses the existing
comment/string masker. Source files that cannot be parsed fail the check.
Use attached `///` or `/** ... */` comments to explain what a function guarantees.
Stage new files before running the check; untracked and ignored files are outside its Git diff.

The pre-commit hook checks a temporary snapshot of the staged index, so unstaged edits and local
notes cannot make a bad staged change pass. It runs the fast profile and skips Cargo when no
staged path affects Rust or its coverage gate; Rust deletions and file-type changes count as
changes, including Unicode paths. The pre-push hook checks each distinct proposed commit from
Git's ref input with the full profile; a deletion-only push has no source to check. Both hooks
reject whitespace errors in the snapshot, and each loads the verifier from the staged index or
proposed commit, so an unstaged verifier edit cannot change which checks run.

Snapshots include skip-worktree files omitted by sparse checkout, using their indexed bytes without
changing the checkout's sparse selection or index flags. Exported symlinks must resolve inside the
snapshot; links that escape it, are dangling, or form cycles are rejected. Verification
subprocesses clear source-repository Git paths and command-scoped configuration overrides.

Hook installation uses worktree-specific Git configuration and preserves an existing hook manager.
Install again in each worktree where you want the hooks. They need a POSIX shell, `mktemp`, a Node
release `devEngines` admits, npm, Rust, and Git on `PATH`. Full verification also needs
`cargo-llvm-cov` and LLVM tools, as the coverage gate does, and the checkout's `node_modules`
(`npm ci`), which the snapshot links in for the title harness. The hooks are local safeguards; CI
remains authoritative and Git can bypass them.

Clippy runs `--all-targets --all-features --locked` with `-D warnings`, matching CI. Cargo.toml
also rejects debug macros, unfinished `todo!`/`unimplemented!` code, and undocumented unsafe blocks
through that strict invocation. Run `cargo fmt --all` to repair formatting before retrying a failed
check. JavaScript and TypeScript corpus files are allowed to contain invalid or awkward source;
these checks do not lint or reformat them.

Commands run sequentially with complete output saved under the common Git directory's
`delivery-check-logs/`; a failure prints at most the last 100 lines. A repository lock prevents two
instances of this runner from sharing Cargo concurrently; other Cargo invocations must still be
serialized by hand. A killed process can leave `delivery-check.lock` behind. Inspect its recorded
PID and remove it only after confirming that process has exited.

By default, worktrees share the main checkout's `target/` and `target/llvm-cov-target/`, and
explicit Cargo target environment variables are respected. Temporary target links let the corpus
gates use the binary built in that cache, which avoids a cold dependency build per snapshot. An
existing target directory pointing elsewhere is rejected, and a cache nested inside the checkout's
`target/` is refused before any directory is created; choose `target/` itself or an external cache.

The full profile does not run the memory and scaling benchmarks, mutation campaigns, or the inline
CI self-check. Use `npm run check:not-run` and `ci.yml` to select those when a change needs them.
This workflow relaxes no gate threshold or corpus exclusion.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/):

```
feat(discovery): read typecheck.include from the resolved config
fix(fingerprint): count escaped brackets in a regex literal
docs: trim the README to what a new reader needs
```

Types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `chore`, `ci`. A `!` after the type, or a
`BREAKING CHANGE:` footer, marks an incompatible change.

Release notes are generated by GitHub from the merged pull requests, so a pull request's title is
what users read. Write it for them: say what changed and why it matters, not which file moved.

## Pull requests

Work on a branch and open a pull request against `main`. One concern per PR: a rule change and a
refactor in the same diff cannot be reviewed or reverted independently.

A PR description should say what changed, how it was verified, and what was deliberately left out.
If a measurement motivated the change, give the number.

CI must be green. The Rust coverage ratchet runs on pushes to `main` and on manual workflow
dispatches, not on pull requests, to avoid a second instrumented build on every one. Before merging, run `npm run verify:full` on the exact
reviewed PR head and record the full commit SHA and the Rust coverage result in the PR description.
The pre-push hook checks that proposed commit's coverage too, but hooks are opt-in, so their
existence alone is not evidence that the check ran. New commits invalidate the previous result and
need another check before merge. Verify the main coverage job after merging as well. Coverage
floors and scope stay unchanged.

## Fixtures

A unit test proves a function does what you think. A fixture proves the compiled binary reaches the
same verdict on real Vitest source. Rule and extraction changes need both.

Fixtures live in `fixtures/` and carry a machine-readable expectation block in the header comment:

```ts
// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L14 "adds two numbers"
// @vitest-dedup:participant L18 "sums one and one"
```

Four rules that are easy to get wrong:

1. The block is the test, not the filename. A fixture with no block, or one that fails to parse,
   is a gate failure rather than a silent pass. The `duplicates-*` and `not-duplicates-*` names are
   for humans.
2. Name the verdict, not just the rule. `redundant` (a copy can go), `consolidatable` (both cases
   are needed in one table), `similar-uncertain` (a review prompt), `distinct` (compared, found
   different), `not-compared` (never eligible). The last two are different claims, and conflating
   them lets a fixture pass because its tests were silently skipped.
3. A negative fixture must be one edit from firing. If it would stay silent however the analyzer
   changed, it is documentation. Write the edit that flips it into the header.
4. Vary the value and the kind where the kind is the point. Two literals of the same type prove
   nothing about a serializer that drops type information; `true` and `'true'` are the case that
   catches it.

Before adding an encoding or a normalization, ask what it now makes indistinguishable. That
question is what most fixtures here exist to answer.

## Releases

Tag-driven, from `main`. Run `npm run bump:version -- <version>` on a release-preparation branch,
review and merge that change, and create `v<version>` at the merged commit. The bump updates Cargo,
the npm workspaces and lockfile, the documented Action pin, and the Release workflow's default
version. `npm run check:versions` asserts those sites agree before a release can proceed.

Run the `Release` workflow manually against that exact tag. Its default `publish: false` rehearses
all platform builds and npm packages without creating a release. `publish: true` creates the GitHub
Release with checksums, Sigstore signature bundles and build provenance, then publishes to each
enabled registry. The `github-release`, `crates-io` and `npm` GitHub environments should allow
only `v*` tags and have no required reviewers, as in CukeDedup. Repository write access and the
workflow's exact-tag, merged-`main` ancestry, and version checks control dispatch; there is no
owner-only or manual reviewer gate. Set `PUBLISH_NPM=true` and, after crates.io setup,
`PUBLISH_CRATES=true` as repository
variables to enable those jobs.

If npm publication stops partway through, dispatch the workflow again on the same tag with
`publish: true` and `publish_npm_only: true`. This verifies the existing GitHub Release and its
live tag at the dispatched commit, rebuilds and checks the npm binaries against its archived binaries, then publishes
only missing package versions. It does not recreate the GitHub Release or publish the crate.

Check the individual GitHub Release, npm, and crate jobs and their registry pages before calling
the release complete; a skipped registry job did not publish anything. The npm website may cache
an older version even when the registry's `latest` tag has advanced.

The first crate version must be published manually by a crates.io owner before trusted publishing
can be configured. For v0.2.0, use a clean checkout of the existing, immutable tag, authenticate
with your own crates.io token using `cargo login`, and run:

```sh
git switch --detach v0.2.0
cargo publish --dry-run --locked
cargo publish --locked
```

The dry run has already passed on the v0.2.0 tag. After crates.io lists that version, configure a
trusted publisher for `figueiredoluiz/vitest-dedup`, workflow `release.yml`, and GitHub environment
`crates-io`. Create that tag-restricted GitHub environment, then enable `PUBLISH_CRATES=true`. Later release
dispatches use a short-lived crates.io credential from OIDC; no registry token belongs in GitHub
secrets. The already-published v0.2.0 tag cannot gain this new workflow; the automated crate job
applies to later tags.
