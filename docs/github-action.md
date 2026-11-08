# GitHub Action

```yaml
- uses: figueiredoluiz/vitest-dedup@vX.Y.Z
  with:
    path: .
    version: X.Y.Z
    threshold: "5"
```

Replace `X.Y.Z` with a released version; the [README](../README.md#github-action) carries the
current pin.

## What the Action does

The Action downloads the release binary for the runner's platform, verifies its SHA-256 against the
published checksum and, with `gh`, that its build provenance attestation comes from this
repository's release workflow at the version's tag (v0.1.0, built from `main`, is checked against
the workflow only), and only then runs it.

- `version` is required and exact, so the workflow pins the code it runs.
- `token` (default `github.token`) is passed to `gh` alone.
- Install `gh` and `tar` on the runner's PATH outside the checked-out repository.
- When `gh` is missing, or has no credentials at all, the attestation check is skipped with a
  warning. A failed verification, including a rejected token, always fails the step.

## Inputs

The inputs are `path`, `config`, `threshold`, `reporters`, `output`, `rule`, `globals`,
`no-metrics`, `fail-on-incomplete`, `baseline`, `fail-on-new`, `changed-since`, `max-memory`,
`no-memory-cap`, `version`, and `token`.

- `path` is the positional argument, passed last after `--`, so a value such as `-dir` or
  `check` names a directory. `version` selects the release binary, and `token` authenticates the
  attestation check.
- Each of the others maps to the flag of the same name.
- `globals` accepts `true`, `false`, or empty (including omission). Only `true` forces globals;
  the other accepted values preserve config detection. Other values fail before installation or analysis.
- `fail-on-incomplete` accepts `false` or empty to leave strict failure disabled, `true` for
  `--fail-on-incomplete`, or `any`, `reads`, and `limits` for the corresponding `=mode` flag.
  Other values fail before installation or analysis. See [incomplete runs](incomplete-runs.md)
  for what each mode rejects.
- `rule` accepts several overrides separated by newlines or commas.
- The JSON reporter is always enabled so the outputs below can be read.
- `output` defaults to `reports/vitest-dedup`. The Action resolves the filename stem against its
  working directory before cleanup and analysis. Destinations under either its lexical path or
  its canonical path count as inside the working directory. For those destinations, every
  existing parent must resolve inside it too; outward, dangling or cyclic directory links fail
  before cleanup or installation. Inward links and missing directories are supported. Explicit
  output paths outside the working directory remain supported and are the workflow author's
  responsibility. This check assumes the checkout is not concurrently modified.
- `--update-baseline` is deliberately not an input: rewriting a committed record of accepted debt is
  not a CI gate operation.
- `changed-since` needs `fetch-depth: 0` on the checkout.

## Outputs

| Output | Description |
| --- | --- |
| `exit-code` | Native exit code (0, 1, or 2). Always set; `2` when the install failed or the report was unreadable (`json-report` still names an unreadable file) |
| `findings` | Total findings, baselined or not |
| `errors` | Error-severity findings |
| `warnings` | Warning-severity findings |
| `tests-analyzed` | Tests extracted |
| `files-analyzed` | Files parsed |
| `incomplete` | `"true"` when the run could not fully cover the corpus |
| `baselined` | Findings the baseline accounted for |
| `new-findings` | Findings absent from the baseline |
| `error-cases` | Distinct runtime cases in a non-baselined error-severity finding: the threshold's numerator |
| `eligible-cases` | Runtime cases the threshold counts: every test but benchmarks, wrapper placeholders and bodyless todos |
| `duplication-percentage` | `error-cases / eligible-cases` as a percentage; empty when no case is eligible |
| `json-report` | Path to the JSON report |
| `html-report` | Path to the HTML report, when the html reporter was selected |
| `attestation` | `verified`, or `unavailable` when only the checksum could be checked |

Before analysis, the Action removes stale JSON and HTML reports at the selected output stem.
A missing JSON report makes the step fail: `exit-code` is `2` if the analyzer exited successfully,
and otherwise preserves the analyzer's nonzero exit code. An unreadable or malformed JSON report
sets `exit-code` to `2`. An earlier report cannot supply the current run's outputs.

Back to the [documentation index](README.md).
