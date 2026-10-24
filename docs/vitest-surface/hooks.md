# Hooks


| Hook | Signature | Scope | Notes |
| --- | --- | --- | --- |
| `beforeAll(fn, timeout?)` | `fn: (ctx: ModuleContext) => unknown` | File (top-level) or enclosing `describe` | Runs once before all tests in scope. |
| `afterAll(fn, timeout?)` | same | same | Runs once after all tests in scope. |
| `beforeEach(fn, timeout?)` | `fn: (ctx: TestContext) => unknown` | File or enclosing `describe` | Runs before every test in scope; may **return a cleanup function**, which runs after all `afterEach` hooks in that scope (i.e. later than ordinary `afterEach`). |
| `afterEach(fn, timeout?)` | same | same | Runs after every test in scope. |
| `onTestFinished(fn)` | `fn: (ctx: TestContext) => unknown`, called from inside a running test/hook | Single test instance | Runs after that test's `afterEach` chain; multiple registrations run in **reverse** registration order; not affected by `sequence.hooks` config. |
| `onTestFailed(fn)` | same shape | Single test instance | Only fires if the test failed; also runs after `afterEach`. |

## `onTestFinished` and `onTestFailed` have two call forms

Both are destructurable off
`TestContext` (as shown in [signature variants](declarations.md#signature-variants)) *and* importable directly from `'vitest'` as named exports, the same
shape as `beforeEach`/`afterEach`. Both forms throw if called outside a running test body. The
docs explicitly warn the global/imported form is unsound under `.concurrent`: "If you are running
tests concurrently, you should always use the `onTestFailed`/`onTestFinished` hook from the test
context since Vitest doesn't track concurrent tests in global hooks", i.e. the imported form
resolves "the current test" through shared module state that a single sequential runner keeps
consistent but that concurrent execution does not. An extractor doing name-based import resolution
for `'vitest'` exports should recognize `onTestFinished`/`onTestFailed` as legal top-level imports,
not just as context members.

## Nesting and ordering

- Hooks registered at file top-level apply to every test in the file; hooks registered inside a
  `describe` apply only to tests (and nested describes) within that block.
- Default execution order is a "stack": `beforeAll`/`beforeEach` run outer-to-inner (parent before
  child), `afterAll`/`afterEach` run inner-to-outer (child before parent), i.e. `afterEach` order
  is the reverse of `beforeEach` registration order, configurable globally via `sequence.hooks`.
- `test.beforeEach`/`test.afterEach`/`test.beforeAll`/`test.afterAll` (hooks hung off a
  `test.extend()`-produced object) are the same hooks, scoped to whatever suite they're declared
  in, just spelled through the extended-test namespace so they can see fixture types.
- `test.aroundEach`/`test.aroundAll` (v4.1+) combine setup+teardown in one wrapper; exact signature
  UNVERIFIED, treat as an opaque hook-registering call for extraction purposes (record its
  presence and scope; don't try to model control flow inside it yet).

## The effective hook chain

For context-compatibility purposes (the `redundant-hook` and context-equivalence rules), a
`TestCase`'s effective hook chain is: every
`beforeAll`/`beforeEach` from the root of the file down through each enclosing `describe`, in
declaration order, **plus** the corresponding `afterEach`/`afterAll` from each level in reverse,
this is exactly the ordered set the analyzer needs to compare for "equivalent context."

A registration written under control flow -- inside an `if` branch, a loop body, a
`try`/`catch`/`finally` arm or a `switch` case (including fallthrough between arms), or after a
`return`/`throw`/`break`/`continue` among direct siblings of its block -- carries that position
as part of its identity. The guard is recorded structurally (which position)
plus, where one exists, the controlling expression's own source text; it is never an evaluation
of which branch runs. Different guards can both be true for the same tests, and a matching
`switch` case can fall through to later cases -- the identity is structural, never a claim of
exclusive execution. Because the runtime registration depends on that unevaluated control flow
and on the values of the conditions governing it, a scope holding any guarded registration is
marked unresolved: its tests are excluded from comparison rather than compared on an identity
that cannot know whether the hook runs.

Back to the [Vitest surface index](../vitest-surface.md).
