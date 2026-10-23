# VitestDedup

[![npm version](https://img.shields.io/npm/v/vitest-dedup)](https://www.npmjs.com/package/vitest-dedup)
[![crates.io version](https://img.shields.io/crates/v/vitest-dedup)](https://crates.io/crates/vitest-dedup)
[![License](https://img.shields.io/github/license/figueiredoluiz/vitest-dedup)](LICENSE)

A static analyzer that finds duplicate and near-duplicate Vitest tests.

Website: [vitestdedup.dev](https://vitestdedup.dev)

It reads project configuration and test files without importing or executing either one. A test's
identity is its body: two tests that do the same thing under different titles are duplicates, and
two textually identical bodies that run under different hooks, mocks, or fixtures are not.

## Install

```sh
npm install --save-dev vitest-dedup   # project-local
npx vitest-dedup .                    # one-off, no install
```

Installation pulls one prebuilt binary for your platform through `optionalDependencies`. There is
no `postinstall` script, so nothing is downloaded, compiled, or executed at install time. The
`vitest-dedup` command is a launcher that execs the binary and forwards its exit code unchanged. The
launcher needs Node 20 or later.

Binaries are published for macOS (arm64, x64), Linux (arm64, x64; glibc and musl), and Windows
(arm64, x64). Elsewhere, build from source:

```sh
cargo install --git https://github.com/figueiredoluiz/vitest-dedup
```

`VITEST_DEDUP_BINARY` points the launcher at a specific executable, such as a local build.

To give an AI agent the workflow instead, install the [agent skill](#agent-skill).

Packages are published from this repository's release workflow with npm trusted publishing, so
`npm audit signatures` verifies them. Release archives carry SHA-256 checksums, keyless Sigstore
signature bundles, and a build provenance attestation.

## Usage

```sh
vitest-dedup .                                              # analyze, default gate
vitest-dedup . --threshold 5                                # allow a 5% error-severity rate
vitest-dedup . --rule duplicate-test=warning                # change a rule's severity
vitest-dedup . --reporters json,html --output reports/dedup  # write report files
vitest-dedup . --reporters jsonl | jq -c 'select(.type == "finding")'
vitest-dedup . --baseline dedup-baseline.json               # gate only on findings the baseline lacks
vitest-dedup . --changed-since origin/main                  # only findings touching changed files
vitest-dedup . --max-memory 4096                            # raise the 2048 MiB memory budget
```

`vitest-dedup check <path>` is the explicit form of `vitest-dedup <path>`. `--help` lists every
flag.

Exit codes are distinct so CI can tell a broken analyzer from duplicated tests:

| Code | Meaning |
| ---: | --- |
| 0 | The error-severity duplication rate is within `threshold` (the default of 0 means no error-severity findings at all) |
| 1 | Quality gate failed: the threshold was exceeded, or more findings than `--fail-on-new` allows are absent from the baseline |
| 2 | Operational error: bad configuration, unreadable path, no tests analyzed without a quality-gate failure, unusable baseline or `--changed-since` revision, or `--fail-on-incomplete` on another degraded run |

## Rules

Each rule's default severity, its meaning, and the remedy its verdict names are in
[docs/rules.md](docs/rules.md).

## Reports

| Reporter | Output |
| --- | --- |
| `terminal` | stdout, for reading a run directly |
| `jsonl` | stdout, one record per line, for `jq` or an agent |
| `json` | file, for machine consumption and diffing between runs |
| `html` | file, a browsable report |
| `sarif` | file, for GitHub code scanning |

`terminal` and `jsonl` both write to stdout and cannot be combined. `--output` is a filename stem:
`--output reports/dedup` writes `reports/dedup.json`, and file reporters default to the stem
`reports/vitest-dedup`. An explicit `--output` requires a file reporter and a filename prefix;
directory paths (including a trailing slash) are rejected. A closed stdout pipe does not change
the analysis exit code or prevent file reporters from completing.

Two diagnostic streams are available on request. `--emit-test-fingerprints` adds the per-test
comparison keys to the `jsonl` stream, and `--debug-memory` adds a run-level record of retained
bytes and their per-test average. See [docs/reports.md](docs/reports.md) for the record shapes.

## Configuration

A `.vitest-dedup.json` file is found by walking upward from the analyzed directory, or named with
`--config`. Command-line flags override it.

The keys, glob and threshold semantics, and how Vitest configs are read are in
[docs/configuration.md](docs/configuration.md).

## Adopting it on an existing codebase

A codebase with existing duplication cannot turn the gate on at once, and a threshold high enough
to pass gates nothing. Record what is there, then gate on what is added.

```sh
vitest-dedup . --baseline dedup-baseline.json --update-baseline   # record today's findings
vitest-dedup . --baseline dedup-baseline.json                     # gate only on what is new
```

Baselined findings are still reported, marked `baselined`, and excluded from the gate.
`--fail-on-new N` fails the gate when more than N findings are absent from the baseline.
`--update-baseline` is refused on an incomplete run, because the baseline would accept whatever the
run could not see, and it cannot be combined with `--changed-since`.

`--changed-since <rev>` reports only findings with a participant in a file changed since that
revision. `--baseline-from-ref <rev>` computes the baseline from the base revision's own findings,
read through git objects and never checked out, so a pull request fails only for what the branch
adds. Both say so in their output, because a run that examined a subset is not a verdict on the
whole repository. The options are described on [docs/baselines.md](docs/baselines.md).

A finding that is not debt, such as a deliberately parallel pair, can be answered next to the code
instead of in a baseline file:

```ts
// vitest-dedup-disable-next-line these two are deliberately parallel examples
test('adds numbers', () => { expect(add(1, 1)).toBe(2); });
```

The directive suppresses findings with a participant on the next line. It outranks baselines: a
suppressed finding never consumes baseline allowance and is never recorded by `--update-baseline`,
because a baseline records accepted debt and the directive claims there is none. Every report
states how many findings were suppressed this way.

## GitHub Action

```yaml
- uses: figueiredoluiz/vitest-dedup@v0.7.0
  with:
    path: .
    version: 0.7.0
    threshold: "5"
```

The action downloads the release binary for the runner's platform and verifies its SHA-256 and,
with `gh`, its build provenance attestation before running it. Inputs, outputs and the verification
details are in [docs/github-action.md](docs/github-action.md).

## Agent skill

An Agent Skill for Claude Code is published in [`skills/vitest-dedup`](skills/vitest-dedup). It
teaches an agent to run the analyzer, read the `jsonl` stream, and act on findings by rule rather
than treating every one as a deletion.

Install it into a project with the [`skills`](https://www.npmjs.com/package/skills) CLI:

```sh
npx skills add https://github.com/figueiredoluiz/vitest-dedup --skill vitest-dedup
```

This installs only the skill, which tells the agent how to run the analyzer; install the analyzer
itself as described under [Install](#install).

## Incomplete analysis

Some tests cannot be compared: an unreadable config, a dynamic title, a table the analyzer cannot
resolve statically. The run is then marked incomplete and says why, because the absence of a
finding is no longer evidence of its absence. `--fail-on-incomplete` turns that into a gate, and
`=reads` or `=limits` narrows which causes count. The `json` and `jsonl` reports list the excluded
files, up to 10,000, with a reason code. See
[docs/incomplete-runs.md](docs/incomplete-runs.md).

## Limitations

- A call to an imported helper is compared by the helper's resolved body when the import resolves
  to a readable ES-module file with a plain parameter list. Unresolved imports, defaulted or rest
  parameters, and helpers that call helpers fall back to comparing by name, as same-file helpers
  always do.
- Type tests (`*.test-d.ts`) are analyzed only when the config enables the typecheck channel, which
  matches when Vitest would run them.
- Dynamic test generation that no static reader can follow is reported as unresolved rather than
  guessed at.

## Documentation

The website is at [vitestdedup.dev](https://vitestdedup.dev). Every documentation page is listed in
[docs/README.md](docs/README.md). To contribute, start at [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
