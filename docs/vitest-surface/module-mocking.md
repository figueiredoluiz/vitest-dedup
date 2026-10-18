# 12. The `vi` module-mocking family

The mocking surface is larger than `vi.mock`, and two pairs in it differ from `vi.mock` exactly
the way `vi.doMock` does. The distinction is easy to miss because the call shape
(`(specifier, factory?)`) is identical across all of them.

| Member | Hoisted? | Handling |
| --- | --- | --- |
| `vi.mock(path, factory?)` | Yes | Modelled |
| `vi.doMock(path, factory?)` | No | Taints the file |
| `vi.unmock(path)` | Yes | Modelled |
| `vi.doUnmock(path)` | No | Taints the file |
| `vi.resetModules()` | N/A | Not a registration |
| `vi.mocked(value, options?)` | N/A | Not a registration |
| `vi.importActual<T>(path)` | N/A | Not a registration |
| `vi.importMock<T>(path)` | N/A | Not a registration |
| `vi.mockObject(value, options?)` | N/A | Not a registration |
| `vi.when(spy, options?)` | N/A | Not a registration |

## Members

### `vi.mock(path, factory?)`

- **Hoisted:** **Yes**, moved to top of file, before all imports
- **Scope:** File-wide; affects every `import` of `path` in the file
- **Notes:** The primitive, covered by `fixtures/edge/vi-mock-hoisting.test.ts`. Factory runs once, cached. `{ spy: true }` automocks while preserving original implementations. **Modelled** when written as a module-top-level statement with a string-literal specifier: it becomes a registration event in the fold described below. Written anywhere else, inside a function, a hook, an `if`, it taints the file instead; see the fold's third bullet.

### `vi.doMock(path, factory?)`

- **Hoisted:** **No**
- **Scope:** Scoped, affects only dynamic `import()`s evaluated *after* this call runs, in whichever test/hook calls it; static imports and earlier dynamic imports are unaffected
- **Notes:** **Handled conservatively** (`src/frontend/extract.rs`): a `doMock` call cannot be soundly attributed to one test by static analysis, so this extractor marks the **whole file's** mocks unresolved whenever any `vi.doMock` call appears anywhere in it. The alternatives are both false-duplicate hazards:
  - treating it as file-wide like `vi.mock`
  - silently ignoring it

  The tradeoff is recall, not precision: real duplicates elsewhere in a `doMock`-using file simply stop being compared. See `fixtures/edge/vi-domock-scoped.test.ts`.

### `vi.unmock(path)`

- **Hoisted:** **Yes**, same hoisting class as `vi.mock`, and hoisted calls keep their relative source order
- **Scope:** File-wide
- **Notes:** Removes `path` from the mock registry; subsequent imports (anywhere in the file, due to hoisting) get the real module. **Modelled**, on the same terms as `vi.mock` above: a module-top-level call with a string-literal specifier is a registration event, and the file's mock set is the fold of all such events, last-in-source-order wins per specifier. For example:
  - `vi.mock(p, f); vi.unmock(p)` nets to unmocked.
  - `vi.unmock(p); vi.mock(p, f)` nets to mocked.
  - `vi.unmock(p)` alone (nothing ever mocked `p`) nets to the same empty set as a file that mocks nothing.

  See `fixtures/edge/vi-unmock-file-wide.test.ts` and the `fixtures/duplicates/not-duplicates-mock-then-unmock*` pair.

### `vi.doUnmock(path)`

- **Hoisted:** **No**
- **Scope:** Scoped, only the next dynamic import(s) after this call; does not reevaluate an already-imported module
- **Notes:** Non-hoisted counterpart of `vi.unmock`, symmetric to how `vi.doMock` is the non-hoisted counterpart of `vi.mock`. **Handled conservatively**, the same way as `vi.doMock`: the call marks the file unresolved rather than being ignored or folded in as if it were hoisted. See `fixtures/edge/vi-dounmock-scoped.test.ts`.

### `vi.resetModules()`

- **Hoisted:** N/A (a call, not a registration)
- **Scope:** Per-call; commonly invoked in a `beforeEach`
- **Notes:** Clears the module cache so a subsequent dynamic `import()`/`require()` reevaluates the module; does **not** reset the mock registry itself (a module mocked before `resetModules()` is still mocked after). Top-level static imports already evaluated cannot be reevaluated by this call. Not currently modeled by the extractor as anything other than an opaque body statement; low risk since it doesn't change `mocked_modules` membership, only cache freshness.

### `vi.mocked(value, options?)`

TypeScript-only type helper, returns its argument unchanged at runtime. `options: { partial?, deep? }` only narrows the returned *type*. Not a registration; purely a body-level call the generic body fingerprint already captures.

### `vi.importActual<T>(path)`

Imports the real module, bypassing mock substitution, the standard way to build a partial mock inside a `vi.mock` factory.

### `vi.importMock<T>(path)`

Imports a module with every export automocked, following the same automocking algorithm `vi.mock()` uses with no factory.

### `vi.mockObject(value, options?)`

**New since 3.2.0.** Deep-mocks an arbitrary object's properties/methods the same way `vi.mock()` mocks a module's exports, without going through the module registry at all. `{ spy: true }` preserves original implementations while still tracking calls.

### `vi.when(spy, options?)`

**New since 5.0.0.** Argument-matched stubbing on a spy: `.calledWith(...args).thenReturn(...)`/`.thenThrow(...)`/`.thenResolve(...)`/etc. A body-level API for shaping mock behavior per call signature, not a module-registration primitive, same low extraction risk as `vi.mocked`/`vi.importActual` above.


## Why the pairs matter

The `mock`/`doMock` and `unmock`/`doUnmock` pairs matter for this analyzer specifically: a
test's effective `Context` includes which modules are mocked and with what factory (the
`duplicates/not-duplicates-different-mocks-*` pair asserts this for `vi.mock`). Anything that changes *whether* a specifier is mocked, including un-mocking it,
is therefore part of that context, and any registration primitive the extractor doesn't recognize
is a silent hole in context-equivalence: two files can look identically mocked while one is
genuinely unmocked at runtime, which is exactly the shape of a false `duplicate-test` finding.

## What is modelled, and what still taints

All four primitives are recognized.
`collect_mocked_modules` (`src/frontend/extract.rs`) models the hoisted pair, `vi.mock` with its
factory and `vi.unmock`, as an ordered event list read from the module's top-level statements and
folded per specifier, last in source order winning. The file's `Context` then carries exactly the
specifiers Vitest's registry would hold.

That yields **two** distinct contexts from three source shapes, not three. "Mocks `./m`" stands
alone; "mocks then unmocks `./m`" and "never mentions `./m`" fold to the same empty
`mocked_modules` and therefore hash identically, which is correct: at runtime both resolve the
real module and the tests are comparable.

The ordering was measured under Vitest 5 with a module exporting `value = 99` and a factory
returning `value: 1`: `vi.mock(p, f)` then `vi.unmock(p)` yields **99** (the real module), while
`vi.unmock(p)` then `vi.mock(p, f)` yields **1** (the factory). "Unmock wins" would be right only
for the first ordering.

### What marks the whole file unresolved

Three things mark the whole file unresolved, excluding its tests from comparison rather than
recording a mock state that may be wrong:

- **`vi.doMock` / `vi.doUnmock`**, because they are not hoisted: they take effect where they run,
  for dynamic imports evaluated after that point, so there is no file-wide answer to fold.
- **A specifier that is not a readable string literal**, or a `vi.mock` factory that is present but
  not an inline arrow/function, recording nothing would be indistinguishable from no mock at all.
- **`vi.mock` / `vi.unmock` called outside module top level**, in a `describe`, a test body, an
  `if` (the non-hoisted `vi.doMock` / `vi.doUnmock` are covered by the first bullet wherever they
  appear).
  Vitest 5 does not run such a file at all: the `vitest:mocks` plugin fails the transform with
  *"N call(s) in `<file>` was defined outside of the module's top level scope … Although it appears
  nested, it will be hoisted and executed before anything in this file"*. That was confirmed by
  running the three shapes (inside `describe`, inside `test`, inside a top-level `if`) under
  Vitest 5.0.0. The error is also the answer to the conditional case: the call is hoisted out of
  the branch, so `if (flag) vi.mock(p)` is not conditional at all, and Vitest refuses it rather
  than let the nesting read as if it were. A file the runner rejects has no runtime context to
  model, and on older Vitest it has a file-wide but unconditional one, so "unknown" is the answer
  either way.

Back to the [Vitest surface index](../vitest-surface.md).
