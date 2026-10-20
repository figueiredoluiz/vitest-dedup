# Default file globs and projects

## Default file globs

| Option | Default | Purpose |
| --- | --- | --- |
| `include` | `['**/*.{test,spec}.?(c\|m)[jt]s?(x)']` | Which files are "test files" and get collected at all. Resolved relative to `root`. |
| `exclude` | `['**/node_modules/**', '**/.git/**']` | Files never scanned regardless of `include`. |
| `includeSource` | `[]` | Additional non-test source files scanned **only** for `import.meta.vitest` blocks (see [in-source testing](in-source-testing.md)). Empty by default, in-source testing is entirely opt-in. |
| `benchmark.include` | `['**/*.{bench,benchmark}.?(c\|m)[jt]s?(x)']` | Legacy benchmark files collected independently of `include`. |
| `benchmark.exclude` | `['node_modules', 'dist', '.idea', '.git', '.cache']` | Benchmark-file exclusions, independent of `exclude`. |
| `benchmark.includeSource` | `[]` | Additional source files containing benchmarks; opt-in and independent of `includeSource`. |
| `typecheck.enabled` | `false` | The gate for the whole type-test channel: while off, the files `typecheck.include` names are not test files at all (type tests run only under this switch or the `--typecheck` CLI flag). |
| `typecheck.include` | `['**/*.{test,spec}-d.?(c\|m)[jt]s?(x)']` | Type-test files, consulted only while `typecheck.enabled` is on. A separate list from `include`: `.test-d.ts` matches the default `include` never. |
| `typecheck.exclude` | Vitest's `configDefaults.exclude` | Type-test exclusions, independent of `exclude`. |

Discovery in the analyzer therefore has five independent admission channels per project: ordinary
test files (`include` minus `exclude`), benchmark files (`benchmark.include` minus
`benchmark.exclude`), test in-source files (`includeSource` minus `exclude`), benchmark
in-source files (`benchmark.includeSource` minus `benchmark.exclude`), and, gated behind
`typecheck.enabled`, type-test files (`typecheck.include` minus `typecheck.exclude`). A match in
any channel admits the file once. Custom test globs do not replace benchmark defaults, custom
benchmark globs do not replace test defaults, and the type-test defaults apply only when
`typecheck.include` is not written.

## Projects and workspaces

- Current mechanism: root `test.projects: (string | InlineProjectConfig)[]`. Entries can be:
  - glob strings matching directories containing a project config (`'packages/*'`), including
    negation (`'!packages/excluded'`) and extended-glob exclusion (`'packages/!(business)'`),
  - explicit config-file globs (`'packages/*/vitest.config.{e2e,unit}.ts'`),
  - inline objects (`{ test: { name, include, environment, ... } }`).
- A referenced project config file must be named `vitest.config.*` / `vite.config.*` /
  `vitest.<name>.config.*` / `vite.<name>.config.*`. The analyzer additionally requires it, and any
  other path the configuration names, to lie inside the analyzed directory
  ([configuration](../configuration.md#monorepos)).
- Inline projects **inherit the root config by default** in v5 (`extends: true` implicit)
  `[v5 CHANGE, pre-v5 default was not to inherit]`; `extends: false` opts a project out entirely.
  Some options concatenate across the inheritance chain (e.g. `setupFiles`) rather than override;
  `tags` replaces rather than merges. Certain options (`coverage`, `reporters`,
  `resolveSnapshotPath`, `attachmentsDir`, and reportedly more per the "not supported inside a
  project" marker on the config reference pages) are **root-only** and cannot vary per project,
  `UNVERIFIED` whether `globals` is on that restricted list; treat as project-overridable per [how the API becomes available](availability-and-globals.md#globals).
- Legacy `vitest.workspace.{ts,js}` files are deprecated (since 3.2) but documented as still
  functionally equivalent to a `projects` array, the extractor should treat a discovered
  `vitest.workspace.*` the same as a root `projects` list.
- **Implication for corpus boundaries**: a project (in the `test.projects` sense) is the right unit
  for resolving `globals`, `include`/`exclude`/`includeSource`, and `setupFiles`, the analyzer
  should build one "resolved config" per project and attribute each discovered test file to exactly
  one (or, if globs overlap, potentially more than one, `UNVERIFIED` whether Vitest allows/dedupes
  a file matched by two projects) project, rather than assuming one global config for the whole
  repository.

Back to the [Vitest surface index](../vitest-surface.md).
