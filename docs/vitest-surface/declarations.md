# Declarations: callables, modifiers and signatures

## Test-declaring callables

| Callable | Alias of | Import | Notes |
| --- | --- | --- | --- |
| `test` | none | `vitest` | Primary test declarator. |
| `it` | `test` | `vitest` | Full alias: the same object, with the same modifiers, and interchangeable everywhere including `.each` and `.extend` chains. |
| `describe` | none | `vitest` | Suite declarator. |
| `suite` | `describe` | `vitest` | Full alias of `describe`. See [`suite` in v5](#suite-in-v5). |
| `bench` | none | v2-v4: `vitest`; v5: not an export | Declared differently in each major. See [`bench` in v5](#bench-in-v5). |

### `suite` in v5

`[v5 CHANGE]` The `vitest/suite` *subpath* entry point was removed in v5. The named export `suite`
from the `vitest` package root is unaffected, so the extractor treats `suite(...)` exactly as
`describe(...)`.

### `bench` in v5

`[v5 CHANGE]` "Benchmarking API Rewrite":

- **v5:** `bench` is no longer `import { bench } from 'vitest'`. It is a **test-context fixture**,
  destructured from the callback argument of a regular `test`:
  `test('name', ({ bench }) => { bench(...) })`. `bench.skip`, `.only` and `.todo` are removed; use
  `test.skip`, `.only` and `.todo` on the enclosing test instead.
- **v2-v4:** `bench` is a top-level import with its own `.skip`, `.only`, `.todo` and `.each`,
  structurally parallel to `test`.

The extractor supports **both shapes**, because the corpus contains both major versions.

### Interchangeability

`test` and `it` are the same function object, and so are `describe` and `suite`. Every modifier and
chain form below applies identically to both members of a pair. The pairs are not interchangeable
with each other: `describe.only` and `test.only` are unrelated.

## Modifiers and chained forms

| Modifier | Applies to | Effect | Chainable with |
| --- | --- | --- | --- |
| `.skip` | test, describe (, bench in ≤v4) | Never runs; still listed/reported | `.concurrent`, `.each`, `.for` |
| `.only` | test, describe (, bench ≤v4) | Only `.only`-marked items run in the file/run | `.concurrent`, `.each`, `.for` |
| `.todo` | test, describe (, bench ≤v4) | Placeholder, not executed, reported as todo | generally terminal (no body required) |
| `.fails` | test only | Inverts pass/fail, test **must** throw/fail or the run fails | `.each`, `.for` |
| `.concurrent` | test, describe | Runs in parallel with sibling concurrent tests | `.skip`, `.only`, `.todo`, `.each`, `.for` |
| `.sequential` | test, describe | `[v5 CHANGE]` **Removed in v5** ("Removed `test.sequential`, `describe.sequential`, and `sequential` Options"). Present and meaningful in v2-v4 as the inverse of `.concurrent` (forces sequential execution inside an otherwise-concurrent suite). Extractor must still recognize it as a legal chain member for pre-v5 corpora. | UNVERIFIED |
| `.skipIf(cond)` | test, describe | Skip when `cond` truthy, evaluated at collection time | usually leaf of chain, precedes `.each`/`.for`? UNVERIFIED ordering |
| `.runIf(cond)` | test, describe | Inverse of `.skipIf` | same |
| `.each(table)` | test, describe (, bench ≤v4) | Expands one declaration into N declarations, one per table row | combines with `.skip/.only/.todo/.concurrent` |
| `.for(table)` | test, describe | Like `.each` but does not spread array rows into positional args, and gives the callback `TestContext` as its 2nd param | same combinability as `.each` |
| `.extend(fixtures)` | test only | Returns a **new** `test`-like function bound to merged fixtures; not a modifier on an individual test, it produces a new base object that itself has `.skip/.only/.each/...` | Chains from the new object, not from `test` directly |
| `.scoped(...)` | result of `.extend` | Deprecated in v4.1 in favor of `.override`; sets/overrides fixture values for the current suite and nested suites | Must be called at top level of a `describe` block |
| `.shuffle` | describe (config-level `sequence.shuffle` also exists) | Randomizes execution order of tests within the suite; inherited by nested suites unless overridden with `{ shuffle: false }` | UNVERIFIED whether `test.shuffle` exists (docs only show `describe.shuffle`) |

Newer, v4.1+/v5-only members (worth knowing exist even if a v1 extractor treats them as
out-of-scope, so they don't get misparsed as unknown-and-ignored when they actually carry
semantics):

| Form | Since | Meaning |
| --- | --- | --- |
| `test.override(fixtures)` | 4.1 | Overrides fixture values for all tests in current + nested suites; supersedes `.scoped`. Must be called at the describe-block top level, so its call site (not just its existence) is part of the suite structure. |
| `test.beforeEach` / `.afterEach` / `.beforeAll` / `.afterAll` | scoped-hook sugar | Hooks bound to a `test.extend`-produced object, typed with its fixtures, behaviorally identical to the free `beforeEach`/etc. but namespaced under the extended `test`. |
| `test.aroundEach` / `.aroundAll` | 4.1 | New hook forms combining before+after in one wrapper function. UNVERIFIED exact signature. |
| `test.describe` / `test.suite` | 4.1 | "Scoped describe", a `describe` bound to the extended fixtures, same semantics as top-level `describe` otherwise. |

### Legal chain orders

Confirmed from docs (both orders shown as valid, i.e. **order-independent** for
skip/only/todo × concurrent):

```
test.concurrent(...)
test.skip.concurrent(...)   // or test.concurrent.skip(...)
test.only.concurrent(...)   // or test.concurrent.only(...)
test.todo.concurrent(...)   // or test.concurrent.todo(...)
describe.skip.concurrent()  // or describe.concurrent.skip()
describe.only.concurrent()  // or describe.concurrent.only()
describe.todo.concurrent()  // or describe.concurrent.todo()
```

`.each`/`.for` appear to always be the **last** link before the call, e.g.
`test.concurrent.skip.each(table)(title, fn)`, `describe.concurrent.each(table)(title, fn)`,
`test.skipIf(cond).each(table)(...)`. The extractor should treat the modifier chain as an
**unordered set** of flags (skip/only/todo/concurrent/fails) collected by walking `MemberExpression`
links, with `.each`/`.for`/`.skipIf(...)`/`.runIf(...)` as the (at most one) chain-terminal,
call-taking members, rather than hard-coding a specific order, the docs demonstrate both orders
are accepted and there is no indication the parser cares about order for the boolean flags.

## Signature variants

```ts
// title forms
test('a plain string title', fn)
test(`a template literal title ${maybeDynamic}`, fn)   // only statically resolvable if template has no expressions, or all expressions are literals/const-foldable
test(namedFunction, fn?)                                // if 1st arg is a Function, its .name becomes the title WITHOUT calling it

// options object: 2nd OR 3rd positional argument
test('name', fn, timeoutMs)                             // legacy 3-arg numeric-timeout form
test('name', { timeout, retry, repeats, tags, meta, concurrent, skip, only, todo, fails }, fn)  // options as 2nd arg, fn as 3rd
describe('name', { ... }, fn)                            // same options-object pattern for suites

// callback receiving TestContext
test('name', (ctx) => { ctx.expect(1).toBe(1) })
test('name', ({ expect, skip, task, annotate, signal, onTestFailed, onTestFinished }) => {})
```

- **Title as string**: the common case, statically a `StringLiteral`.
- **Title as template literal**: statically resolvable only when it is a `NoSubstitutionTemplateLiteral`, or every substitution expression const-folds (e.g. `${1+1}`); an extractor must fall back to "dynamic title" (store cooked template + placeholder positions) when substitutions reference runtime identifiers.
- **Title as function name**: Vitest uses `someFn.name` for `test(someFn, handler)` without calling `someFn`. Only the later handler is the test body; `test(someFn)` has no handler and becomes a todo. Options and `.each`/`.for` rows apply normally. VitestDedup resolves an inline named function expression as a test title; anonymous functions and names inferred through bindings remain unresolved.
- **Options object position**: can be the **2nd** argument (`test(name, options, fn)`) or the value at that position can instead be the **function itself with no options** (`test(name, fn)`), extractor must disambiguate by argument type (`Function` vs plain `Object`), not position alone. A bare number in the "3rd" position is the legacy timeout shorthand, not an options object.
- **`options.timeout`**, **`options.retry`** (`number | { count?, delay?, condition? }`), **`options.repeats`**, **`options.tags`** (`string[]`), **`options.meta`** (arbitrary object), plus the boolean modifier fields (`concurrent`, `skip`, `only`, `todo`, `fails`) which are **the object-literal equivalent of the chained forms**, `test('x', { skip: true }, fn)` is equivalent to `test.skip('x', fn)`. The extractor must union modifier flags from both the member-expression chain and the options object.
- **`TestContext` callback param**: destructured or whole-object; whole-object access (`(ctx) => ctx.skip()`) vs destructured (`({ skip }) => skip()`) are both legal and must both be recognized when detecting in-body use of `skip`/`expect`/`onTestFinished`/`onTestFailed`/`bench`(v5)/`task`/`annotate`/`signal`.
- **`onTestFinished`/`onTestFailed` are also importable directly from `'vitest'`**, exactly like
  `beforeEach`/`afterEach`, not only reachable through `TestContext` destructuring. The imported form
  is unsound under `.concurrent`; the context-bound form is the only one Vitest recommends there.
  See [Hooks](hooks.md) for the hook-table entry and the full caveat.
- **Unreachable declarations**: a declaration after a `return`, `throw`, `break` or `continue` among
  its direct siblings, under `if (false)` or in the `else` of `if (true)`, never runs and is not
  read; a `finally` that jumps (`finally { break out }`) cancels the exit it follows. `return test(...)` and `return describe(...)` register like the call written as a
  statement.
- **Declarations in callbacks**: a call the analyzer does not read (`[1, 2].forEach(...)`, a method
  on an imported value) is not entered. When an argument is an inline function or arrow that may
  register, the scope the call stands in is refused (`helper-effects-unproved`). The callback's
  parameters hold whatever the callee passes, so a callback that uses them is refused too. A
  callback passed by name is not judged, so a registration inside it is not seen.

Back to the [Vitest surface index](../vitest-surface.md).
