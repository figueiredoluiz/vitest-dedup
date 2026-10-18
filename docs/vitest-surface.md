# Vitest test-declaration surface

What VitestDedup recognizes, and why. This is the reference the extractor is written against:
anything Vitest can express that the analyzer must either read or refuse.

Verified against the Vitest API and config documentation at version **5.0.0**. Facts that are v5
breaking changes relative to v2-v4, the versions most repositories still run, are flagged
`[v5 CHANGE]`. Anything not confirmed by the published docs is marked `UNVERIFIED`.

**Start with [Extraction hazards, ranked](vitest-surface/hazards.md)** if you are reading this to
understand where static analysis of Vitest gets hard. The sections in the table are the surface
itself. Code comments and fixtures cite them by the number in the first column; the pages
themselves are named, not numbered.

| # | Section | Page |
| --- | --- | --- |
| 1 | Test-declaring callables: `test`, `it`, `describe`, `suite`, `bench` | [Declarations](vitest-surface/declarations.md) |
| 2 | Modifiers and chained forms: `.skip`, `.only`, `.todo`, `.each`, chaining order | [Declarations](vitest-surface/declarations.md) |
| 3 | Signature variants: options object vs bare function, legacy timeout | [Declarations](vitest-surface/declarations.md) |
| 4 | `.each` / `.for` table forms | [`.each` and `.for` tables](vitest-surface/each-tables.md) |
| 4b | Title directives, numeric coercion, truncation | [Title formatting](vitest-surface/title-formatting.md) |
| 5 | Lifecycle hooks and their scoping | [Hooks](vitest-surface/hooks.md) |
| 6 | `test.extend` and fixture resolution | [Fixtures](vitest-surface/test-extend-fixtures.md) |
| 7 | Imports vs `globals: true` | [How the API becomes available](vitest-surface/availability-and-globals.md) |
| 8 | `import.meta.vitest` | [In-source testing](vitest-surface/in-source-testing.md) |
| 9 | `include`, `exclude`, benchmark and typecheck channels | [Default file globs and projects](vitest-surface/discovery-globs-and-projects.md) |
| 10 | Per-project resolution | [Default file globs and projects](vitest-surface/discovery-globs-and-projects.md) |
| 11 | Playwright, Jest, Cypress, Mocha, `node:test` | [Look-alikes](vitest-surface/look-alikes.md) |
| 12 | `mock`, `doMock`, `hoisted` and the rest of `vi` | [Module mocking](vitest-surface/module-mocking.md) |
| 12b | Module-scope ambient state: timers, env, spies, `expect.extend` | [Ambient state](vitest-surface/ambient-state.md) |
| | Ranked list of what makes extraction hard | [Extraction hazards](vitest-surface/hazards.md) |
