# How describe, test and the rest become available

| Mode | Form | Notes |
| --- | --- | --- |
| Named import | `import { test, describe, it, expect, beforeEach } from 'vitest'` | The common case. Resolved with normal import-binding semantics (`oxc_semantic` symbol resolution), including renamed imports (`import { test as t } from 'vitest'`). |
| Namespace import | `import * as vitest from 'vitest'; vitest.test(...)` | Member access off a namespace-imported binding from the `'vitest'` specifier must be recognized, not just bare identifiers. |
| Globals | `globals: true` in config | No import at all. See [Globals](#globals). |

## Globals

With `globals: true`, `test`, `describe`, `it`, `suite`, `expect`, the hooks and the rest are
ambient globals, syntactically indistinguishable from any other free identifier of the same name.

- Recognizing them **requires resolving the effective Vitest config for the file's project**.
- Only when `globals: true` resolves for that file are the bare identifiers treated as Vitest globals.
- A bare identifier that merely *looks* like a test declarator is not one until the config says so.

### `globals` config specifics

From `/config/globals`:

- Default: **`false`**.
- Enable with `test: { globals: true }` in the resolved Vite/Vitest config, or the `--globals` CLI flag.
- For TypeScript ambient types, projects also add `"types": ["vitest/globals"]` to `tsconfig.json`.
  That is a type-checking concern only, not a runtime-behavior signal, so the extractor does not use
  it as an alternative detection source: a repo could have the types entry without `globals: true`,
  or vice versa via the CLI flag. It MAY be used as a corroborating hint when the config file itself
  is not resolvable.

### Where `globals: true` can be set

Any of:

- **Root `vitest.config.{ts,mts,cts,js,mjs,cjs}`**, looked up first.
- **Root `vite.config.{ts,mts,cts,js,mjs,cjs}`**, looked up second, if no `vitest.config` is found.
  The `test` field is nested under Vite's own config object, optionally gated by
  `process.env.VITEST` or Vite's `mode` to avoid polluting the plain Vite build config type.
- **Per project**, inside a `test.projects` entry: either an inline object
  (`{ test: { globals: true, ... } }`) or a referenced project config file matching
  `vitest.config*`, `vite.config*`, `vitest.<name>.config.*` or `vite.<name>.config.*`
  (`<name>` = letters, digits, `_` or `-`).
- **Legacy `vitest.workspace.{ts,js}` files**, deprecated since 3.2 in favor of `test.projects` but
  still functionally supported per the projects guide. Each entry can carry its own `globals` the
  same way a `projects` array entry can.

Per-project settings matter because projects can set `extends: false` to ignore the root config, so
`globals` is **not guaranteed uniform across a workspace**. The extractor must resolve it **per
project**, not once globally for the whole scan.

`[UNVERIFIED]` Whether `globals` is on the explicit "root config only" restricted-options list. The
config overview page mentions such a list exists, but the fetched excerpt did not confirm this
option's membership. Treat it as settable per project unless direct testing shows otherwise.

Config file lookup does **not** consider JSON files for the root config: `vitest.config.json` is not
in the documented lookup list. `UNVERIFIED` for legacy `vitest.workspace.json`.

Back to the [Vitest surface index](../vitest-surface.md).
