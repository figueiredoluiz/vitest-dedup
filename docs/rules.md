# Rules

| Rule | Default | Meaning |
| --- | --- | --- |
| `duplicate-test` | Error | Two tests have the same normalized body under an equivalent context. |
| `duplicate-test-name` | Warning | Two tests in one file share a full title. |
| `parameterization-candidate` | Warning | Two tests have the same structure and differ only in literal values. |
| `near-duplicate-test` | Warning | Two tests perform the same behavior under an equivalent context, but their normalized bodies differ. |
| `cross-file-duplicate` | Warning | The `duplicate-test` criteria without the single-file restriction. |
| `subsumed-test` | Off | One test's behavior is a strict ordered subsequence of another's. Off by default: its findings are frequently distinct tests rather than redundant ones. |
| `duplicate-suite` | Warning | A whole `describe` block is duplicated within one file, reported once instead of per test. |
| `duplicate-each-row` | Warning | Two rows of one parameterized test, benchmark, or suite carry the same case. |
| `redundant-hook` | Warning | A nested lifecycle hook repeats work an ancestor hook already performs. |

Each finding carries a verdict naming its remedy, because the remedies differ.

- Redundant: one copy can go. `duplicate-test`, `cross-file-duplicate`, `duplicate-suite`,
  `duplicate-each-row`, `redundant-hook`.
- Consolidatable: every case is needed, and they belong in one `test.each` table.
  `parameterization-candidate`. Deleting a row here would lose coverage.
- A review prompt: `near-duplicate-test` and `subsumed-test` flag tests that look equivalent
  without establishing that either adds no coverage. Review decides, and for
  `near-duplicate-test` the usual outcome is consolidation into one `test.each` table.

`cross-file-duplicate` reports at Warning rather than Error because duplication across unit and
integration layers is sometimes deliberate.

## Local classes and constructors

Constructor calls retain their callee, ordered arguments and generic arguments. Local class
declarations, class expressions and owned aliases also retain their implementation and nominal
identity: two distinct class objects do not become interchangeable merely because their methods
look the same. Fields, methods, accessors, private names, heritage and static initialization
participate in the comparison. Definition-time effects remain separate from construction.
References to separate local instances retain their declaration identity even when their
classes and construction arguments are identical. Consistent local renaming remains comparable.

Parentheses and erased TypeScript `as`, `satisfies`, non-null and angle-bracket assertions on these paths preserve the same constructor behavior.

Captured local values and readable `each` cells participate at the use site. Captures retain
which enclosing variable they use, even when initial values are equal; consistently renaming
those variables preserves comparison. Proven direct
assignments update a captured scalar or constructor alias; captured arrays and objects retain
their initializer and supported straight-line receiver mutations. Literal-only variations can
still produce a `parameterization-candidate`; that finding says to preserve both cases.

Unmodeled state makes the dependent body unresolved and excludes it from comparison. This
includes dynamic control-flow writes, escaping captured objects, unknown class-object mutations,
dynamic reflective overrides, source-text reflection, decorators and unowned factory results.
Known class names, intrinsic constructor names and observed stable callable names remain values.
Observed private method and callable-field names also remain values; explicit function names
survive private storage renaming. Replaced or escaping private callables remain unresolved,
while ordinary private object data properties named `name` or `toString` stay comparable.
Declared functions and expression/arrow closures share owned lexical capture environments;
ordinary helper renaming remains comparable until its name is observed. Callable aliases retain
the original function name. Callable metadata writes or escapes and a helper returning a captured
mutable object remain explicitly unresolved. Vitest mock captures and plain TypeScript parameter
properties are modeled. Mutable captured
objects retain a single ordered digest history across proven owned constructions and mutations.
Bounded class-summary and history caches degrade visibly when their budgets are exceeded. Unrelated
tests remain comparable; unresolved hooks and fixtures affect only their consumers.
Native and imported constructor bindings keep their existing syntactic comparison; their
conditional writes do not enter the owned local-class state model.

Behavior collection visits `for` expression/declaration initializers and ordinary template
interpolations, retaining nested calls, constructors and other modeled effects in source order
and deferred scope. Tagged-template call modeling remains separate.
