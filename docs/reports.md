# Reports

Choose a reporter by how the result will be consumed.

| Reporter | Destination | Best for |
| --- | --- | --- |
| `terminal` | stdout | Interactive use and readable CI logs. |
| `jsonl` | stdout | Streaming shell pipelines and AI agent workflows. |
| `json` | File | Structured integrations and complete run metadata. |
| `html` | File | Searchable human review with light and dark themes and per-finding source snippets. |
| `sarif` | File | GitHub code scanning and other SARIF 2.1.0 consumers. |

```sh
vitest-dedup .                                            # terminal
vitest-dedup . --reporters json,html --output reports/vd   # files only
vitest-dedup . --reporters sarif --output reports/vd       # for upload-sarif
vitest-dedup . --reporters jsonl | jq -c 'select(.type == "finding")'
```

`terminal` and `jsonl` both own stdout and cannot be selected together. `jsonl` combines freely
with the file reporters, which never touch stdout.

| Page | Contents |
| --- | --- |
| [Terminal](reports/terminal.md) | Layout, footer, detection time, color. |
| [Findings](reports/findings.md) | `verdict`, `fingerprint` and the structured fields machine formats carry. |
| [Diagnostic records](reports/diagnostic-records.md) | `--emit-test-fingerprints` and `--debug-memory`. |
| [SARIF](reports/sarif.md) | How findings and rules map to SARIF 2.1.0. |
| [Report safety](reports/safety.md) | Why report content is data, and how every reporter sanitizes it. |
| [Baselines and changed-file scoping](baselines.md) | `--baseline`, `--update-baseline`, `--changed-since`, `--baseline-from-ref`. |
| [Incomplete runs and limits](incomplete-runs.md) | What `incomplete` means, `--fail-on-incomplete`, work and memory limits. |
| [Imported runners](imported-runners.md) | Which first-party helper functions are expanded into tests, and what is refused. |

## Writing files

`--output` is a filename stem, not a directory: `--output reports/vd` writes `reports/vd.json`.
Without `--output`, file reporters use the stem `reports/vitest-dedup`. Parent directories are
created as needed.

- Each report is buffered into a temporary file in its output directory, then atomically replaces
  the destination after successful serialization and flush.
- A final-component symlink is replaced without writing to its target. Output directories may
  contain symlinks.
- If writing fails, the previous report remains intact and the run fails.
- Replacement uses temporary-file permissions and does not preserve the previous inode or its
  metadata.
- Several selected reports are replaced independently.

After every selected file report has been written successfully, file-only runs print
`Reports written to:` followed by indented, sanitized paths. The path spelling and filename-stem
behavior are preserved. Runs with `terminal` or `jsonl` omit this announcement, leaving stdout to
the selected stream reporter.

## Diagnostics

Diagnostics stay on stderr: early operational errors, globals detection, incomplete coverage,
baseline and changed-file qualifications, and the opt-in cell census. Disabled globals and absent
configuration remain silent. Globals detection markers retain their existing wording for corpus
tooling. Diagnostic text is plain and sanitized; writer failures are operational errors.

The terminal caps its file lists, but the `json` and `jsonl` reports list every excluded file with a
reason code; see [Excluded files](incomplete-runs.md#excluded-files).

Lists are bounded with explicit omission counts:

- at most 20 immediate read failures
- at most 20 deferred diagnostic messages
- at most 10 wholly excluded files
- at most 10 files containing only statically empty tables

Counts describe all affected messages and files, including omitted entries; display limits do not
change analyzed scope, findings or exit policy. Census categories retain their own counts.

## `verdict` and `fingerprint`

Machine formats carry both. `verdict` names what a finding claims (`redundant`, `consolidatable`,
`name-collision` or `similar-uncertain`), and `fingerprint` is its stable identity across runs.
Details are on the [Findings](reports/findings.md) page.

## Determinism

The same admitted input produces byte-identical output, with two exceptions: terminal output
includes the detection time unless `--no-metrics` is set, and the `--debug-memory` record carries
process-dependent figures such as peak RSS. A memory-limited run may admit a different
file prefix as process residency changes. Findings are ordered by their participants' locations
(root-relative path, then position), then by rule, then by message. Paths are compared component by
component rather than as strings, so the order does not depend on the platform's separator.
