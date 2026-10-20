# Security policy

## Reporting a vulnerability

Do not open a public issue. Use
[GitHub private vulnerability reporting](https://github.com/figueiredoluiz/vitest-dedup/security/advisories/new)
with the affected version, impact, and reproduction steps. Strip credentials, proprietary source,
and unrelated personal data from the report.

You should get an acknowledgement within seven days. A fix and disclosure are coordinated with the
reporter, and an advisory is published when users can act on it. Please allow a reasonable
remediation period before disclosing publicly.

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | Yes |
| Older | No |

## Analyzed code is never executed

VitestDedup parses configuration and test source and never imports, requires, transpiles and runs,
or otherwise executes either one. This holds for test files and for the Vitest and Vite config
itself. Running it against a hostile repository does not run that repository's code.

Configuration that only a running JavaScript engine could resolve, such as a project list built by
a function call or a dynamic import, is reported as undetermined rather than evaluated.

## Analyzed content is untrusted input

Test titles and file paths come from the analyzed repository and reach reports verbatim. Each output
format escapes them for its own needs:

- HTML escapes the five HTML-special characters, encodes `<` and `>` inside the JSON island so the
  browser's tokenizer cannot see `</script`, and strips invisible bidirectional-override characters
  that would otherwise reorder a rendered line to point at a different file.
- JSON, JSONL, and SARIF emit valid encodings of the bytes they were given, with the same bidi,
  format-character, line-separator and control-character stripping.
- The terminal reporter strips ANSI escapes and control characters so a title cannot move the
  cursor or recolour later output.

Reports may contain source-derived text. Treat a generated report with the same care as the
repository it describes.

## The GitHub Action downloads and executes a release binary

The action fetches the release asset for the runner's platform and, before executing it, verifies
its SHA-256 against the published checksum and, with `gh`, that its build provenance attestation was
signed by this repository's release workflow building the version's tag (v0.1.0, built from
`main`, is checked against the workflow only). The `token` input (default `github.token`) is given
to `gh` only; the analyzer receives no token or runner credential. When `gh` is
missing, or has no credentials at all, the attestation check is skipped with a warning; a failed
verification, including a rejected token, always fails.
`version` is required and exact, so the workflow pins the code it runs.

## Resource bounds

Hostile or merely enormous input is bounded rather than trusted: per-file size limits, a cap on
analysis work, and a memory budget. Exceeding any of them marks the run incomplete and says so,
rather than reporting a partial result as a clean one.

## Compromised releases

npm packages are published from this repository's release workflow using trusted publishing, so
`npm audit signatures` verifies them. Release archives carry SHA-256 checksums and a build provenance
attestation. If you believe a published artifact does not match this repository, report it through
the private channel above.
