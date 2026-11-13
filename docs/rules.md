# Rules

Set a rule to `off`, `warning` or `error` in [configuration](configuration.md) or with
`--rule <id>=<severity>`.

| Rule | Default | Meaning |
| --- | --- | --- |
| `duplicate-test` | Error | Two tests have the same normalized body under an equivalent context. |
| `duplicate-test-name` | Warning | Two tests in one file share a full title. |
| `parameterization-candidate` | Warning | Two tests have the same structure and differ only in literal values. |
| `near-duplicate-test` | Warning | Two tests perform the same behavior under an equivalent context, but their normalized bodies differ. |
| `cross-file-duplicate` | Warning | The `duplicate-test` criteria without the single-file restriction. |
| `subsumed-test` | Off | One test's behavior is a strict ordered subsequence of another's. Off by default: its findings are frequently distinct tests rather than redundant ones. |
| `duplicate-suite` | Warning | A whole `describe` block is duplicated within one file, reported once instead of per test. |
| `duplicate-each-row` | Warning | Two rows of one parameterized test or suite carry the same case. |
| `redundant-hook` | Warning | A lifecycle hook repeats work another hook in its execution chain already performs. |

## Verdicts

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

Body comparisons track `repeats` on suite and test registrations. An explicit `repeats: 0`
runs the body once even when its suite or project repeats tests. The project default stays
symbolic, so an omitted count can compare with another use of the default. An option that
might supply an unknown count leaves the test in the inventory and its body unresolved.
A test inside a loop over an empty array literal is never registered and is never compared. A
loop whose iterations are not counted leaves its test unresolved
([configuration](configuration.md#quality-gate) lists the loops that are counted).

Behavior comparisons retain destructuring evaluation,
including unused properties and iterator steps. A single-property binding can still match
a direct member read when its sole use safely accounts for the whole pattern. Destructuring
alone does not make a body eligible for a behavior finding, and an extra binding alone does
not establish strict subsumption. These comparisons remain review prompts.

`subsumed-test` claims a pair only when both tests are known to expect to pass and the larger one
runs. An expected failure (`.fails`, `{ fails: true }`, a failing suite) and options that could
set one without spelling it (`{ fails: flag }`, `{ ...options }`, options passed as a variable)
withhold the claim. So does a larger test that is, or may be, skipped or a todo: by its own chain
or options or an enclosing suite's, including an option read from a value (`{ skip: flag }`).

Parameterization candidates retain the identity of shared helpers. Unknown computed dependencies
withhold consolidation. Numeric data indexes whose immutable value is proved primitive can become
table inputs when their consumers are proved primitive expressions or non-invoking comparisons.
Unknown calls, selected operations and escaping values remain significant, including through a
local alias. Source-sensitive assertions and uncertain module identities also withhold consolidation.

## Benchmarks

A top-level `bench` is compared on the same terms as a test, by every rule, but only with other
benchmarks: a benchmark and a test never pair, even with equal bodies or titles. Options such as
`iterations` are part of what a benchmark measures, so they keep two benchmarks apart. Vitest runs
no hook around a benchmark, so a `beforeEach` that resets shared state does not make it safe for
benchmarks to read. A benchmark is compared only when its options are plain data, literal values under written keys:
options can hold `setup` and `teardown` callbacks that run around it, and what those run is not
read. A `parameterization-candidate` between benchmarks advises one loop of
`bench(...)` calls over the varying values, since there is no `bench.each` and a `test.each`
table would not be measured.

A benchmark is compared only where it runs: in a file `benchmark.include` collects, in a project
whose installed Vitest, or the `vitest` version its nearest `package.json` declares, is 4. Vitest 5
has no top-level `bench`, a `bench` in a test file fails outside benchmark mode, and any member
Vitest 4's `bench` lacks (it has `skip`, `only`, `todo`, `skipIf` and `runIf`; not `each`, `for`
or `concurrent`) makes it fail the whole file, so those, and any benchmark whose
Vitest version is not known, are counted as `unsupported-benchmark` and not compared. Benchmarks are never quality-gate cases.

## Parameterized rows

String values and property keys retain their JavaScript UTF-16 code units, including lone
surrogates. A lone `\uD800` differs from the literal text `\uFFFDd800`; valid surrogate pairs
equal the corresponding Unicode character. This identity also applies to static test titles.

Repeated-row claims require proven input values, equal rendered titles and equivalent callback
and setup context within one declaration. Primitive proofs preserve JavaScript types, negative
zero, bigint and non-finite numbers. Immutable scalar constants can cross explicit named exports
and re-exports; mutable bindings, erased declarations, cyclic value reads and dependencies with
mocking capability remain unknown. Namespace or dynamic-import capability also withholds imported
value proofs because Vitest module objects can be rewritten. A local literal object's member is readable only while the
object has no writes, aliases or escaping uses. The same proof reads present indices and `length`
from local literal arrays of primitive values; holes, spreads, missing indices and unproved cells
remain unknown.

Ordinary function callbacks keep table inputs significant when they read `this` or `arguments`,
including callbacks without named parameters or with empty destructuring patterns. Arrow callbacks
inherit these values from their surrounding scope.

Rectangular tagged tables with unique static headers followed by a newline support primitive cells and flat
destructuring; effectful cells or unsupported bindings leave the row unresolved. Tagged callbacks
that read `this` or `arguments` also remain unresolved. Suite tables
expand their children with the row's primitive bindings and title
ancestry, including nested suite and test tables. A child's title may read those bindings. Any
test title may also read a `let` its declaring body assigns only through plain `=` statements,
under `===`/`!==` branches over proved values: a registration in that body reads the value at that
point, and one in a nested suite callback the value the body leaves, unless the body may return
early, await or yield. Expansion is bounded to 2,048 enclosing suite
invocations per declaration; unsupported inputs produce unresolved cases. Readonly literal arrays
can retain child comparisons through their own scalar indices or `length`, with each invocation
kept separate. They do not establish repeated-row input equality. Whole-suite table
equivalence and arbitrary collection-time execution are outside this row proof. Removable
suite-row claims require block callbacks, including nested suites, and inert owning options; concise callbacks still
contribute children but do not establish an inert collection body.

## Hook scheduling

Identical lexical writes alone do not prove a hook removable: Vitest can yield between hooks,
so removing one may change when queued work becomes observable. The analyzer withholds
`redundant-hook` claims when callbacks in the affected lifecycle scope contain potentially
deferred or opaque work, including async callbacks, nested callbacks, unknown calls or
non-assertion property access. Ordinary synchronous assertions and proven lexical writes remain
eligible; property writes stay refused — a file-local scan cannot prove the receiver stays plain
against exports, setup files and loaded modules.

A hook inside a suite table whose rows are proven is an ordinary lifecycle hook: the table's
callback runs once per generated suite, so its registrations form the same chain a suite body
does, and a pair is reported once by declaration location. Only primitive-literal rows qualify —
title formatting reads richer rows again, and an accessor among them can run code. Unread or
empty table rows keep the tests' usual treatment instead (unresolved or unregistered).

A nested `beforeAll` pairs with an identical one in an enclosing scope when the cells it writes
cannot change between the two runs -- every module-wide write to each cell assigns the same
primitive literal. Per-test phases pair across scopes as before; `afterAll` does not, because
the outer hook runs last and the nested write may be the first establishment of that state.

## Local classes and constructors

Tests that construct local classes are compared by what the classes do rather than by what they
are called, except that names a test observes still count. The rules for what counts as the same, and when a test is left out of comparison, are on
[Local classes and constructors](rules/local-classes.md).

Back to the [documentation index](README.md).
