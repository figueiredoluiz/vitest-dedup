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

## `--debug-memory`

Adds one run-level `memory` record: the bytes retained after extraction, split by the field holding
them, with the number of tests and `bytesPerTest`, which is their average. The buckets are disjoint
and sum to the reported total, so a consumer can both trust the total and attribute it.

Shared text is apportioned rather than multiplied: a body referenced by three tests is counted once
and divided among them, so the per-test figure is an average rather than an overcount.

`peakRssBytes` is a process high-water mark for sanity only. It includes the allocator's slack and
the parser's transient arenas, neither of which is retained.

Back to [Reports](../reports.md).
