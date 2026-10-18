# Fixtures and test.extend


```ts
const myTest = test.extend<{ db: Database }>({
  db: async ({}, use) => {
    const db = await connect()
    await use(db)
    await db.close()
  },
})

myTest('reads a row', async ({ db }) => {
  expect(await db.get('x')).toBeDefined()
})
```

- Fixture keys are the **object's property names** (object-syntax) or the **first string argument**
  to `.extend('name', fn)` (builder-syntax, newer). Either way, the fixture's name is statically the
  literal key/string, no computed keys expected in idiomatic code, but a `[computed]: fn` key would
  defeat static naming and should be flagged unresolved.
- A fixture function's signature is `(dependencies, use) => ...` (object-syntax) or
  `(dependencies, { onCleanup }) => value` (builder-syntax). `dependencies` is destructured from
  previously-defined fixtures (including base ones), this is itself a fixture dependency graph the
  extractor can build from the destructuring pattern's property names, matched against sibling keys
  in the same `.extend()` call and any upstream `.extend()` in the chain.
- **Auto vs. lazy initialization**: Vitest decides whether to instantiate a fixture based on whether
  the test callback (or a downstream fixture) actually uses it, i.e., a **runtime**
  usage-detection mechanism, not purely a static one.
  - This is **not** arity/reflection on the function's declared params. Vitest's own docs describe
    it as "smartly initialize your fixtures ... based on usage" and separately instruct that "you
    should always use the object destructuring pattern `{ database }` to access context both in
    fixture function and test function".
  - So the detector is understood to work reliably on the destructured-name shape. Whole-object dot
    access to a *custom fixture* (`(ctx) => ctx.db`, as opposed to a built-in `TestContext` member
    like `ctx.expect`) is a discouraged, not-guaranteed-detected pattern, not merely a stylistic
    alternative.
  - `{ auto: true }` forces initialization unconditionally, sidestepping the detector entirely.
  - **What a static analyzer can know**: the full fixture graph and its declared dependencies (from
    source).
  - **What it cannot fully know**: whether a given test transitively pulls in a given fixture's side
    effects without also statically re-deriving Vitest's own usage-detection heuristic against that
    specific callback's access pattern. The extractor *can* approximate the destructured shape (a
    syntactic pattern-match on the callback's parameter list) but should treat whole-object access
    to a fixture name as a weaker, best-effort signal than a destructured one, not an equally
    reliable mirror of runtime behavior.
- **`scope`**: `'test'` (default, per-test) / `'file'` (shared per file) / `'worker'` (shared per
  worker process), changes whether two tests sharing a fixture are "isolated" for context-equivalence
  purposes; a `file`/`worker`-scoped fixture means state genuinely carries over between tests, which
  the context model should represent (two tests using the same file-scoped fixture are *not*
  automatically context-independent the way two `test`-scoped-fixture tests are).
- **`injected: true`**: value supplied externally via project `provide` config; statically the
  fixture's *value* is unknowable (comes from config, not source), record as "external/opaque
  fixture value."
- **`test.scoped(...)`** (deprecated 4.1+) / **`test.override(...)`** (4.1+, current): both mutate
  fixture values for the current suite (and nested suites) rather than defining new fixtures; must
  be called at a `describe` block's top level, a call site inside a test body or nested callback is
  invalid and (per docs) not the supported form, so the extractor should anchor these calls to
  direct children of a `describe` block's statement list.
- Reaching the callback: extended fixtures are merged into the same `TestContext` object ordinary
  hooks/tests receive, from the test body's perspective there is no syntactic difference between a
  built-in context property (`expect`, `task`, `skip`, ...) and a user-defined fixture; both are
  just destructured names. Disambiguating "known Vitest context member" vs. "user fixture" requires
  cross-referencing the destructured name against (a) the built-in `TestContext` member list and (b)
  the fixture keys visible from the `test.extend` chain that produced the local `test`/`it` binding.

Back to the [Vitest surface index](../vitest-surface.md).
