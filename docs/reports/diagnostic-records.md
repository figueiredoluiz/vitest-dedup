# Diagnostic records

Two opt-in flags add records to the `jsonl` stream. Both require `--reporters jsonl`.

## `--emit-test-fingerprints`

Adds one record per test and per hook: the comparison keys themselves, for debugging why two tests
did or did not match.

```jsonl
{"type":"test","schemaVersion":"0.1","path":"cart.test.ts","line":13,"column":1,"title":"totals a two-line cart","alpha":"24ca2dec36cb546b","structural":"61d3c3582d21bc51","behavior":"6ace5d410965ead9","trivial":false,"comparable":true,"contextKey":"9b9573371b170f09","modifierKey":"09612b07b5ecb5a5","truncatedFields":[]}
```

Comparable tests from distinct declarations registered in one file are a `duplicate-test` when
`alpha`, `contextKey`, and `modifierKey` all match. If a pair you expected to match did not, diff
those three to see which one disagrees.

### `"test"` records

| Field | Meaning |
| --- | --- |
| `registrationFile` | Optional caller path when different from the declaration's `path`; otherwise `path` also identifies the registering file. |
| `alpha` | Identifiers normalized to their resolved symbol; literal values preserved, except inert literal-data properties that a `const` object destructure does not select from its own object-literal initializer. |
| `structural` | `alpha` with literal values collapsed to their kind. |
| `behavior` | The ordered behavior events: callee paths, assertions, awaits, control flow. |
| `trivial` | The body is empty or a recognized stub, and is excluded from comparison whatever the hashes say. |
| `comparable` | The context resolved well enough to participate in comparison. `false` means the test was not compared, not that it was verified distinct. |
| `contextKey` | The ambient facts that gate comparison: hook chain, `vi.mock` registrations, `test.extend` fixtures, imports, module-scope setup, per-directory config scope. |
| `modifierKey` | Modifiers that change what the test does (`.each` data, `.skipIf`/`.runIf`, `.fails`), never ones that only change whether or when it runs. |

### `"hook"` records

Hook records carry `kind`, `alpha`, `structural`, `behavior`, and `trivial`. `redundant-hook` only
compares hooks of matching `kind`.

### Owned-core records

With the experimental `VITEST_DEDUP_ENGINE=next` selection, test and hook records use
`schemaVersion: "0.2"` and `engine: "owned"`. Other record kinds keep their existing versions.
The legacy five fingerprint fields are not synthesized from combined owned comparison keys.
Consumers must dispatch on the record's version and engine rather than compare hashes across
engines or runs built with different toolchains.

Owned test records carry declaration coordinates, an optional `registrationFile`, a rendered title
when known, a zero-based `row` when applicable, `comparable`, `exclusion`, `traceComplete` and
`ordinaryEvents`. `comparisonKeys` contains the actual owned keys:

| Key | Comparison evidence |
| --- | --- |
| `duplicate` | Same-file exact grouping, including body, context, modifiers and unresolved row inputs. |
| `crossFile` | Exact grouping with module context. |
| `parameterization` | Supported literal-shaped consolidation under exact context. |
| `similarity`, `eventSet` | Near-duplicate context and sorted behavior-event multiset. |
| `subsumption` | Context admitted for directional trace comparison. |
| `name` | Proven full title segments. |
| `rowClass` | Proven row identity used with name and exact body/context keys. |

Keys without proof are `null`. Excluded tests still receive a record, preserving known suite
prefixes and supported test-row title rendering, but carry no body-comparison proof. Rule eligibility also depends on the recorded trace facts, registration/declaration identity,
and rule-specific requirements. Equal keys alone do not promise a finding: grouping, lifecycle
barriers, suppressions, budgets and other rule conditions can still prevent one.

`views` contains independent owned `body`, `shape`, `trace`, `context` and `modifiers` digests for
checking a fixture's isolation claim. The context view includes both inherited/local context and
module-load effects. An unavailable view is `null`; a refused test may have no
views at all. These are diagnostic dimensions, not an alternative rule-eligibility oracle.

Owned hook records enumerate known hook declarations, including a single hook that cannot form a
redundancy pair. They carry `kind`, body/shape/trace `views` when available, and
`comparisonKeys.effects`. The effect key is `null` unless the owned hook planner retained a proof
in a comparison chain. A body digest does not establish such a proof.

Hooks precede tests; both precede findings. Registration `site` records remain available alongside
them for the collection oracle. Sites count declarations rather than expanded tests, so their
population can differ from `testsAnalyzed`. With the flag off, none of these optional records are
emitted. Optional retained evidence is charged to the existing memory budget; hitting that budget remains
a visible refusal. The old engine's `0.1` record contract is unchanged.

## `--debug-memory`

Adds one run-level `memory` record: the bytes retained after extraction, split by the field holding
them, with the number of tests and `bytesPerTest`, which is their average. The buckets are disjoint
and sum to the reported total, so a consumer can both trust the total and attribute it.

The owned engine attributes its retained capacity to `fields.engine`, with a disjoint
`engineFields` breakdown: cases, suites, hooks, sites, blind files, modules, exports and scalars.
Those buckets sum to `fields.engine`; they are not extra bytes to add to `fields`. The legacy
field buckets are zero in this record. `engineMemoryNotice` states the estimate's limits,
including temporary parser state and private hash-table spare/control storage.

The legacy `VD_CELL_CENSUS` environment opt-in is retired for the owned engine. Setting it emits
a notice on stderr and does not change machine output. Use the owned engine's table exclusions
instead of treating the legacy histogram as an owned-cell measurement.

Shared text is apportioned rather than multiplied: a body referenced by three tests is counted once
and divided among them, so the per-test figure is an average rather than an overcount.

`peakRssBytes` is a process high-water mark for sanity only. It includes the allocator's slack and
the parser's transient arenas, neither of which is retained.

Back to [Reports](../reports.md).
