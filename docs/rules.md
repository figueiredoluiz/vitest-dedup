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

Constructor behavior events retain callee identity, ordered arguments and nested effects for identifier/static-member callees rooted in imports or globals.
Other constructor shapes and depth-limited callbacks use legacy traversal in a separate behavior comparison scope.
Their existing events remain comparable to each other; mixed-scope near-duplicate and subsumption comparisons are declined.

Behavior collection visits `for` expression/declaration initializers and ordinary template
interpolations, retaining nested calls, constructors and other modeled effects in source order
and deferred scope. Tagged-template call modeling remains separate.
