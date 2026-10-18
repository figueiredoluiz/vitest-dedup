# Look-alikes that are not Vitest tests


| Framework | Import shape | Distinguishing signal |
| --- | --- | --- |
| **Playwright Test** | `import { test, expect } from '@playwright/test'` (or a project fixture wrapper re-exporting `test`) | Import specifier is `@playwright/test` (or a local file that itself imports from it), not `'vitest'`. Callback receives Playwright fixtures (`page`, `context`, `browser`, `request`), recognizably different fixture vocabulary from Vitest's `TestContext`. `test.describe`, `test.beforeEach` exist in both frameworks with similar *shapes*, so import-specifier resolution (not name matching) is the only reliable static signal. |
| **Jest** | `test`/`it`/`describe` as ambient globals (Jest's own `globals`-like default), or `import { test, describe, expect } from '@jest/globals'` | No `'vitest'` import anywhere in the file/module graph; presence of `jest.config.*` instead of `vitest`/`vite` config at the resolved project root; use of `jest.fn()`/`jest.mock()` instead of `vi.fn()`/`vi.mock()` is a strong corroborating (but not exhaustive) signal since some codebases alias `vi` to `jest`-like helpers. |
| **Cypress** | `describe`/`it` as ambient Mocha-style globals injected by the Cypress test runner; `cy.*` commands in the body | No `'vitest'` import; presence of `cypress.config.*` at project root; body uses `cy.visit`, `cy.get`, etc., a distinct global command object with no Vitest analogue. |
| **`node:test`** | `import { test, describe } from 'node:test'` or `require('node:test')` | Import specifier is the `node:test` builtin, not `'vitest'`, so the analyzer deliberately extracts zero Vitest declarations from these bindings. `node:test` also lacks `.each`/`.for`/`.extend` in the same shape (it has its own, smaller modifier set) and typically pairs with `node:assert` rather than Vitest's `expect`. |
| **Mocha/Jasmine (via `globals: true`-like ambient `describe`/`it`)** | Ambient `describe`/`it`, `before`/`after`/`beforeEach`/`afterEach` (note: `before`/`after`, not `beforeAll`/`afterAll`) | The hook names `before`/`after` (rather than `beforeAll`/`afterAll`) are a Mocha-specific tell; no `'vitest'` import; presence of `.mocharc.*`. |

**How to tell them apart statically, in priority order:**
1. **Resolve the import specifier** of the binding used to call `test`/`describe`/etc. via
   `oxc_semantic`. If it traces to `'vitest'` (directly or through a namespace import), it's Vitest,
   full stop, this dominates every other heuristic.
2. If the binding is **not imported at all** (a bare global), it is only a Vitest test if the file's
   resolved project config has `globals: true` **and** no *other* globally-injected test framework
   is active for that project (Vitest and Jest/Mocha/Cypress globals are not normally combined in
   one project, but a monorepo could have different projects using different runners, hence
   resolving config **per project**, [projects and workspaces](discovery-globs-and-projects.md#projects-and-workspaces), matters here too).
3. If the binding traces to a **different** package specifier (`@playwright/test`, `@jest/globals`,
   `node:test`, a Cypress support-file global, etc.), it is definitively **not** a Vitest test,
   regardless of how structurally similar the call shape looks (`describe(str, fn)` is common to
   nearly every one of these frameworks).
4. As a last-resort corroborating signal only (never load-bearing on its own): co-occurring
   framework-specific calls in the same file (`cy.*`, `page.*` Playwright fixtures, `jest.*`), and
   the presence/absence of a `vitest`/`vite` config file at the resolved project root vs. a
   `playwright.config.*`/`jest.config.*`/`cypress.config.*`.

Back to the [Vitest surface index](../vitest-surface.md).
