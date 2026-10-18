# VitestDedup documentation

| Page | Contents |
| --- | --- |
| [Configuration](configuration.md) | The `.vitest-dedup.json` keys, glob semantics, how Vitest configuration is read, runs that analyze no tests. |
| [Rules](rules.md) | Every rule, its default severity, and the verdict that names its remedy. |
| [Local classes and constructors](rules/local-classes.md) | How tests that construct local classes are compared and when they are left out. |
| [Reports](reports.md) | Index of the reporters: terminal, JSONL, JSON, HTML and SARIF. |
| [Baselines and changed-file scoping](baselines.md) | Gate only on what a change adds. |
| [Incomplete runs and limits](incomplete-runs.md) | What `incomplete` means, `--fail-on-incomplete`, source, declaration and memory limits. |
| [Imported runners](imported-runners.md) | Which first-party helpers are expanded into tests, and what is refused. |
| [GitHub Action](github-action.md) | Inputs, outputs and how the binary is verified. |
| [Vitest surface](vitest-surface.md) | Index of the reference the extractor is written against: declarations, tables, hooks, fixtures, globals, mocking. |
| [Development](development.md) | Maintainer reference: verification, hooks, coverage before merge. |
| [Releasing](releasing.md) | Version bump, the Release workflow, registry checks. |

Contributors start at [CONTRIBUTING.md](../CONTRIBUTING.md).
