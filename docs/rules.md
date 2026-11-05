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
| `duplicate-each-row` | Warning | Two rows of one parameterized test, benchmark, or suite carry the same case. |
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

Behavior comparisons retain destructuring evaluation,
including unused properties and iterator steps. A single-property binding can still match
a direct member read when its sole use safely accounts for the whole pattern. Destructuring
alone does not make a body eligible for a behavior finding, and an extra binding alone does
not establish strict subsumption. These comparisons remain review prompts.

Parameterization candidates retain the identity of shared helpers. Unknown computed dependencies
withhold consolidation. Numeric data indexes whose immutable value is proved primitive can become
table inputs when their consumers are proved primitive expressions or non-invoking comparisons.
Unknown calls, selected operations and escaping values remain significant, including through a
local alias. Source-sensitive assertions and uncertain module identities also withhold consolidation.

## Parameterized rows

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
ancestry, including nested suite and test tables. Expansion is bounded to 2,048 enclosing suite
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
eligible. This conservative check also applies to imported setup hooks; it does not exclude
the tests from unrelated comparisons.

## Local classes and constructors

Tests that construct local classes are compared by what the classes do rather than by what they
are called, except that names a test observes still count. The rules for what counts as the same, and when a test is left out of comparison, are on
[Local classes and constructors](rules/local-classes.md).

Back to the [documentation index](README.md).
