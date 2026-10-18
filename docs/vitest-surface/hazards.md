# Extraction hazards, ranked

1. **`bench` changed from a top-level import (v2-v4) to a `TestContext` fixture (v5)**, the single
   test-declaring callable whose entire declaration *shape* differs across the majors the
   extractor will encounter in real repos. Getting this wrong either misses all v5 benchmarks or
   crashes on v4 ones.
2. **`.sequential` was removed in v5** (`test.sequential`, `describe.sequential`, and the
   `sequential` config option are all gone) but is common in v2-v4 corpora, treating it as an
   unknown/ignored member on a v5-only extractor silently drops a real modifier from older code.
3. **`globals: true` resolution is per-project, not per-repo**, `test.projects` entries can set
   `extends: false` and diverge on `globals`, so a bare `test(...)` identifier's meaning depends on
   which project's resolved config governs that file. A single "did this repo set globals" boolean
   is wrong for any repo using projects/workspace with mixed configs.
4. **Look-alikes share call shape, not identity**, Playwright, Jest, Cypress, node:test, and Mocha
   all expose a `describe(string, fn)` / `it(string, fn)` shape nearly indistinguishable from
   Vitest's by name alone. Only import-specifier resolution (or, for globals, resolved per-project
   config plus absence of any other test-framework import) is sound; name-based matching produces
   false positives.
5. **`.each`/`.for` title strings are only reproducible when the table is a literal**, a
   non-literal table argument (`test.each(getCases())`) means row values, row count, and therefore
   concrete titles are unknowable statically; the extractor must degrade to "unresolved" rather than
   guess, and must not silently treat it as zero tests.
6. **Title-rendering itself changed in v5** (`pretty-format`, no quote-wrapping of interpolated
   strings, new truncation option), byte-exact title reproduction is version-dependent, not a
   single fixed algorithm.
7. **Options-object vs. bare-function 2nd argument is a type disambiguation, not a positional one**:
   `test(name, fn)` vs `test(name, options, fn)` requires checking whether the 2nd arg is a
   `Function` or an `Object` literal; modifier flags can also arrive via the options object
   (`{ skip: true }`) and must be unioned with chain-based flags (`test.skip`) rather than treated
   as mutually exclusive sources.
8. **Fixture "reaches the callback" the same way built-in context members do**, a destructured
   name in a test callback is ambiguous between a Vitest built-in (`expect`, `task`, `skip`, `bench`
   in v5) and a user `test.extend` fixture unless cross-referenced against the specific `test`
   binding's `.extend` chain; a naive allowlist/denylist of "known" context names breaks the moment
   a fixture happens to reuse a common name.
9. **In-source tests bind `test`/`it`/`expect` off `import.meta.vitest`**, not off a `'vitest'`
   import specifier and not off ambient globals, a resolver that only checks import specifiers or
   `globals: true` will miss every in-source test, and `includeSource` (default `[]`) gates which
   files even get scanned for the `import.meta.vitest` guard, independently of the ordinary
   `include` glob.
10. **A local variable can be named `test`/`describe`/`it` without being Vitest's** (shadowing an
    import, or a plain helper function), name matching without binding resolution produces false
    positives even inside an otherwise-legitimate Vitest file.
11. **The mocking family is modelled only where hoisting makes a file-wide answer sound**:
    `collect_mocked_modules` folds module-top-level `vi.mock`/`vi.unmock` with string-literal
    specifiers into one mock set (last in source order wins per specifier, verified against a real
    Vitest 5 run, see [§12](module-mocking.md)), and taints the whole file for everything else: the non-hoisted
    `vi.doMock`/`vi.doUnmock`, an unreadable specifier or factory, and **any** of the four called
    outside module top level. A tainted file's tests are excluded from comparison entirely, which
    trades a possible false duplicate for a certain false negative, the same direction throughout.
    The top-level restriction matters because the scan is a whole-program AST walk, and a
    `vi.mock` found inside a `describe`, a test body, or an `if` must not be recorded as file-wide.
    Vitest 5 refuses to run such a file at all (`vitest:mocks`: "defined outside of the module's
    top level scope"), so there is no runtime context there to model, including in the
    conditional case, where the call is hoisted out of the branch. See [§12](module-mocking.md) for the full table and
    `fixtures/edge/vi-unmock-file-wide.test.ts` / `vi-dounmock-scoped.test.ts`, which demonstrate
    the modelled and tainted shapes respectively, and the
    `fixtures/duplicates/not-duplicates-mock-then-unmock*` pair, which asserts the mocked and
    mocked-then-unmocked contexts are compared and found different rather than excluded.
    `[status: current]` as implemented by `collect_mocked_modules` in `src/frontend/extract.rs`.

Back to the [Vitest surface index](../vitest-surface.md).
