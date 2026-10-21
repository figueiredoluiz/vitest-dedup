# Imported runners

A runner is a first-party function that registers tests when called. VitestDedup expands a call to
one as if its tests were written at the call site, so wrapper helpers do not hide duplicates.

## What is extracted

The frontend extracts static declarations from called first-party function exports, including named
re-export chains.

- Source locations and fingerprints come from the helper.
- Registration ownership, governing globals and configuration, and surrounding hooks come from the
  caller.
- Repeated calls count as registrations but retain their shared declaration identity.
- Type-only imports, interfaces and type aliases are erased. They neither add module effects nor
  supply runtime runner or Vitest bindings, and type-only re-exports cannot expose a runtime runner.

## Accepted shapes

A runner is one of:

- a synchronous function declaration
- a `const` arrow or function-expression export, with a block or concise body
- a default export of a function or arrow
- an `export { local as name }` alias of those

It has plain positional parameters, static `test` and `describe` titles, and inline callbacks.
Suite callbacks must be synchronous with no parameters. Extra inert arguments at the call site are
ignored.

### Titles

A title may read a parameter when the call site's argument is one of:

- a literal or constant expression
- a loop variable of an unrolled `for...of`
- a value forwarded from a local helper

Each call renders its own title.

### Hooks

Hooks the runner registers (`beforeAll`, `beforeEach`, `afterEach`, `afterAll`, with an inline
callback and at most a literal timeout) attach to the calling scope as if written at the call site.
They keep the runner's source location and fingerprint, and apply to tests declared before or after
the call. A hook inside the runner's own `describe` stays in that suite.

### Fixtures

A `const t = test.extend({...})` at the top level of the runner, with static fixture values,
supplies fixtures to `t(...)` declarations. It accepts scoped `t.beforeEach(...)`-style hooks like
the bare ones. One declared inside a nested suite callback is refused.

### Nested calls

A runner may call another first-party runner or a function declared in its own file. The nested call
is expanded the same way, under the same depth, admission and source budgets, and a refusal inside
it refuses the outer runner. Recursion through modules or local helpers is refused.

## Refused

- A missing, spread or unprovable argument that a title reads.
- Default, rest or destructured parameters, and a parameter the runner writes.
- Registration modifiers, control flow and registration options.
- `let` or `var` exports, reassigned exports, async and generator functions, opaque initializers,
  and bodyless or overload signatures.
- Module setup effects.
- Called opaque imports at registration time. Uncalled imports and calls inside test callbacks are
  not registration candidates.
- Named or default object method calls as runner candidates, and function `.call()` / `.apply()`
  invocations.
- Mocked runners and mocked re-export hops: they cannot contribute their real bodies.
- Installed-package bodies.

Proved ordinary utilities and shadowed inert callbacks contribute no declarations.

### Module dependencies

Module dependencies must be Vitest, the validated named re-export chain, or first-party modules that
are themselves free of setup effects: importing a module runs it, called or not. Unknown dependency
or setup effects, and helper mocking effects, mark the entire calling file unresolved.

### What a refusal does

A refusal marks the file incomplete and the affected calling scope unresolved, including earlier
tests and its suite record. Those records are excluded from comparison. The `json` and `jsonl`
reports list the file with a reason code naming the refusal; see
[Excluded files](incomplete-runs.md#excluded-files).

## Budgets

- Local and imported expansion share a depth of 8 and 2,000 body admissions across both passes.
- Helper files share the source byte limit (8 MiB), the bracket nesting limit (256) and the
  export-hop limit (8).
- Exact helper sources retained for one caller share an additional 8 MiB budget; repeated calls
  retain each source once.
- Refusals produce at most one helper diagnostic per calling file.

Helper source bytes also serve inline suppressions and HTML snippets.

## Baselines

`--baseline-from-ref` currently refuses imported runners: historical helper blobs are unavailable,
so using present-day bodies would produce a false baseline. The base analysis is reported as
partial. File baselines from complete current analyses retain imported source and caller identity
normally.

Back to the [documentation index](README.md).
