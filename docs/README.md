# VitestDedup documentation

| Page | Contents |
| --- | --- |
| [Configuration](configuration.md) | The `.vitest-dedup.json` keys, glob semantics, how Vitest configuration is read, runs that analyze no tests. |
| [Rules](rules.md) | Every rule, its default severity, and the verdict that names its remedy. |
| [Local classes and constructors](rules/local-classes.md) | How tests that construct local classes are compared and when they are left out. |
| [Reports](reports.md) | Index of the reporters (terminal, JSONL, JSON, HTML, SARIF) and of its pages: [terminal](reports/terminal.md), [findings](reports/findings.md), [diagnostic records](reports/diagnostic-records.md), [SARIF](reports/sarif.md), [report safety](reports/safety.md). |
| [Baselines and changed-file scoping](baselines.md) | Gate only on what a change adds. |
| [Incomplete runs and limits](incomplete-runs.md) | What `incomplete` means, `--fail-on-incomplete`, source, declaration and memory limits. |
| [Imported runners](imported-runners.md) | Which first-party helpers are expanded into tests, and what is refused. |
| [GitHub Action](github-action.md) | Inputs, outputs and how the binary is verified. |
| [Vitest surface](vitest-surface.md) | Index of the reference the extractor is written against. It links the twelve pages under `vitest-surface/`: declarations, `.each` tables, title formatting, hooks, fixtures, globals, in-source testing, discovery and projects, look-alikes, module mocking, ambient state and extraction hazards. |
| [Development](development.md) | Maintainer reference: verification, hooks, coverage before merge. |
| [Releasing](releasing.md) | Version bump, the Release workflow, registry checks. |

Contributors start at [CONTRIBUTING.md](../CONTRIBUTING.md).
