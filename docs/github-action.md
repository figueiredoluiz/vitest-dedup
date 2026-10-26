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

- `path` is the positional argument, `version` selects the release binary, and `token`
  authenticates the attestation check.
- Each of the others maps to the flag of the same name.
- `globals` accepts `true`, `false`, or empty (including omission). Only `true` forces globals;
  the other accepted values preserve config detection. Other values fail before installation or analysis.
- `rule` accepts several overrides separated by newlines or commas.
- The JSON reporter is always enabled so the outputs below can be read.
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
| `json-report` | Path to the JSON report |
| `html-report` | Path to the HTML report, when the html reporter was selected |
| `attestation` | `verified`, or `unavailable` when only the checksum could be checked |

Back to the [documentation index](README.md).
