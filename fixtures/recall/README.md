# Recall corpus

A labelled set that measures what VitestDedup **finds**, not what it **retains**.

## Why retention is not recall

This repository had two corpus controls before this directory existed, and neither one measured
recall:

- `fixtures/duplicates/` and `fixtures/observed/`, checked by `check-duplicates-corpus.mjs`, assert
  a hand-written expectation per fixture. Every one of those expectations exists because somebody
  had already thought of the shape. A rule that never worked for a shape nobody imagined is
  invisible to them, and stays invisible.
- `run-external-corpus.mjs` asserts **known-positive retention**: previously confirmed true
  positives must still be reported. That is valuable and correctly named, and it is not recall, for
  a structural reason rather than a matter of degree — **the labels are selected by the detector
  being evaluated.** A rule can miss every reordered-setup pair, every `.each`-row duplicate and
  every `bench` declaration forever while retaining 100% of the exact duplicates it already found.

Retention and recall are two different mechanisms, and for most of this project's life only
retention existed. This directory is the other one.

## Shape

`manifest.json` carries a recall floor, a known-miss baseline, and one entry per case directory:

```json
{ "schemaVersion": 1, "minimumCases": 45, "knownMissBaseline": 0, "knownFalsePositiveBaseline": 2,
  "minimumRecall": 1.0, "recallDestination": 1.0,
  "cases": [
    { "name": "…", "path": "…", "rule": "duplicate-test", "role": "positive",
      "origin": "synthetic", "verdict": "generated", "classifiedAt": "2026-10-20",
      "expectedTests": 2, "expectedIncomplete": false,
      "arguments": ["--rule", "subsumed-test=warning"],
      "expectedFindings": [ { "rule": "…", "severity": "error", "titles": ["…", "…"] } ],
      "expectedAbsent":   [ { "rule": "…", "titles": ["…", "…"] } ],
      "knownMisses":      [ { "id": "…", "reason": "…", "rule": "…", "severity": "…",
                             "titles": ["…", "…"] } ],
      "knownFalsePositives": [ { "id": "…", "reason": "…", "rule": "…", "severity": "…",
                                 "titles": ["…", "…"] } ],
      "pending": "S-number and one sentence naming the defect this case waits on" } ] }
```

Each case is a self-contained directory, analyzed alone, and records one of three outcomes per
relation:

| Field | Claim | On a change |
| --- | --- | --- |
| `expectedFindings` | behaviour the analyzer supports and must retain | going silent is a recall regression |
| `expectedAbsent` | an intentional non-finding that protects precision | firing is a precision regression |
| `knownMisses` | a finding believed correct that the analyzer does not produce today | being reported fails the gate: move it to `expectedFindings` and decrement `knownMissBaseline` |
| `knownFalsePositives` | a finding the analyzer produces today that is believed wrong | stopping being reported fails the gate: delete the entry and decrement `knownFalsePositiveBaseline`, leaving `expectedAbsent` as the assertion |
| `pending` | a case whose expectations today's binary cannot satisfy at all (an extraction floor over a file it cannot yet discover) | every expectation passing fails the gate: remove the field so they become hard assertions |

`expectedAbsent` matters as much as `expectedFindings`. A recall set that only lists what must be
found cannot catch a rule that starts over-firing — and over-firing is the cheapest way to improve
a recall number. `knownFalsePositives` is `knownMisses`' mirror for exactly that direction, and
`pending` is the case-level form of the corpus gate's `@vitest-dedup:pending` ratchet; both exist
so a case pinning a KNOWN defect can be shipped red-on-arrival classified, rather than not shipped.

`expectedTests` and `expectedIncomplete` are asserted on every case. The first is this repository's
floor on work performed: a case that silently extracts nothing would satisfy every negative
expectation it declares. The second separates `distinct` (compared, found different) from `not-compared` (never
eligible, because a context was unresolved). Both produce no finding, and conflating them is how a
rule that analyzes nothing passes a precision gate.

`origin` (`synthetic`, `real`, `regression`), `verdict` (`reviewed` once someone has read the case
against its source, `generated` otherwise) and `classifiedAt` (a `YYYY-MM-DD` date) are required on
every case and format-checked only; nothing in a run can confirm them. A key the schema does not
name fails the gate. The schema is `scripts/check/lib/manifest-schema.mjs`, and
`scripts/check/lib/corpus-labels.mjs` turns the manifest into a flat list of cases and
expectations, each with a stable `<case>#<n>` id.

`arguments` passes extra CLI tokens to a case's run. One rule ships `Severity::Off`
(`subsumed-test`), so its two cases pass `--rule subsumed-test=warning`: without the override such
a case could only ever pin the rule's silence, which is also what a deleted rule produces. The
`cross-file-duplicate` cases carried the same override until that rule's default became `Warning`;
it was removed rather than left in place, because an override that duplicates the shipped default
keeps a case green through a reverted default — the same reason a case never gets an override it
does not need.

## How an expectation gets into the manifest

**By reading the case source and the rule that governs it, then running the binary — never by
recording what the binary prints.** A manifest generated from current output is a snapshot, not an
expectation, and it would ratify today's bugs as the contract. Where the reading and the binary
disagree and the binary is judged wrong, the manifest records the expectation believed correct and
marks the case a known miss, which counts against the recall floor. That is what
`knownMissBaseline` is for, and why raising it is a reviewed edit rather than a way to get green.

Every case's header comment states the claim, why the case is not already covered by another case,
and — for a negative — the one-edit change that makes the analyzer fire. That last part is the
`fixtures/observed/` rule applied here: "delete the type argument and it becomes an error-severity
`duplicate-test`" is a claim a later reader can re-run in ten seconds; "this should stay silent" is
not.

## Coverage

For every rule in `Rule::ALL`, a positive and a negative, then its variants. The gate derives the
rule set from `lib/fixture-directives.mjs`'s `VERDICT_RULE` — already this repository's single
JS-side mirror of `Rule::ALL` — and fails when any rule lacks either, so a tenth rule cannot arrive
here with no recall coverage and nothing to say so.

| Rule | Positive | Negative | Variants |
| --- | --- | --- | --- |
| `duplicate-test` | renamed locals | different subject | `bench` declarations, including discovery from a `*.bench.ts` file; `.concurrent` vs `.sequential`; dynamic title (`not-compared`); resolved and inline-expression `.skipIf` conditions; in-source `import.meta.vitest` block |
| `duplicate-test-name` | shared full title | same leaf title, different suites | parameterized titles (one finding per row); title from a function name; dynamic title (`not-compared`) |
| `parameterization-candidate` | three differing literals | differing literal **kind** | — |
| `near-duplicate-test` | reordered setup | reordering precondition violated | composed transformations; `.each` row against a hand-written test |
| `cross-file-duplicate` | same body, two files | different subject | — |
| `subsumed-test` | ordered subsequence | subset but not a subsequence | — |
| `duplicate-suite` | copied block | same title, different children; same children, differing hooks | nested duplicate describes; `describe.each`-generated suites must not pair |
| `duplicate-each-row` | repeated row | all rows distinct | the `.for` form; `describe.each` repeated row |
| `redundant-hook` | nested `beforeEach` repeats an ancestor | nested hook does different work; hook-kind mismatch | `afterEach`; `beforeAll`; `afterAll`; non-adjacent chain depth |

## Mixed-rule coverage

No single-rule case can observe interference: every `-positive` holds exactly one defect, so
nothing there can show one rule eating another's finding, a suppression reaching too far, or a
grouping change absorbing the wrong participants. `manifest.json`'s `mixedRuleCoverage` therefore
records one entry per unordered pair of rules — derived from the same `VERDICT_RULE` rule set as
the positive/negative requirement, so a tenth rule arrives with nine uncovered pairs that fail
the gate until covered or recorded.

Coverage means **both rules fire in the named case**. A finding of one rule plus an
`expectedAbsent` of the other is not coverage: the fact worth pinning about co-occurring rules is
that neither eats the other's finding. Suppression-shaped interactions are pinned *inside*
covering cases by their own `expectedAbsent` entries — `mixed-suite-copy-with-internal-duplicates`
asserts the duplicate-suite finding, the within-copy duplicate-test findings that must survive
next to it, and the cross-copy restatements that must stay suppressed, all in one case. An entry
may instead record `cannotCoOccur` with the structural reason no single run can hold both
defects; no pair uses it today, because every pair can co-occur — several only on *different*
participants, never on the same pair:

| Pair shape | Same two tests, both rules? |
| --- | --- |
| `duplicate-test` + `duplicate-test-name` | Yes — identical bodies under one title (`mixed-duplicate-test-and-name`) |
| `duplicate-test-name` + `near-duplicate-test` | Yes — same title, reordered body (`mixed-name-and-near-duplicate`) |
| `duplicate-test-name` + `subsumed-test` | Yes — same title, strict subsequence (`mixed-name-and-subsumed`) |
| `duplicate-test` + `parameterization-candidate` | Never — `representative_per_alpha` guarantees a parameterization pair differs at `alpha` |
| `duplicate-test` + `near-duplicate-test` | Never — same collapse, one representative per `alpha` |
| `duplicate-test` + `cross-file-duplicate` | Never — `representative_per_file` keeps the cross-file rule's pairs spanning files |
| `duplicate-test` + `duplicate-each-row` | Never — table rows share one declaration site, which `duplicate-test` collapses away |
| any test-body rule + `redundant-hook` | Never — hooks are not tests; different participants entirely |

The exclusivity facts above are asserted by each covering case's `expectedAbsent` entries, not
just recorded in this table.

## The floor

`minimumRecall` is the measured value rounded down, not the target: a gate that is red on arrival
gets routed around, after which the number it guards is worse than unmeasured. `recallDestination`
records the destination. The corpus currently detects every one of its 123 desired findings, so
both measured recall and the enforced floor are 100%, with `knownMissBaseline: 0`.

## Evaluating a binary

`npm run evaluate:corpus` runs every case against the release binary and reports, per rule, what
each expectation resolved to instead of stopping at the first failure:

```bash
node scripts/check/evaluate-corpus.mjs ./target/release/vitest-dedup --json report.json
```

For each rule it prints expected findings satisfied, forbidden findings held, known misses and known
false positives still in place, findings nobody labelled, and extraction counts that differ from the
case. Every ratio shows its denominator, and an empty population prints `n/a` rather than a
percentage. For each held `expectedAbsent` entry it also says whether the pair was eligible for
comparison: `eligible` lists the keys that differ (a pair whose keys are equal is counted apart,
because the analyzer keeps one representative per equality group and the silence is that, not
distinctness), `not compared` means a participant was set aside, the rule was off for the case by
default, by its `.vitest-dedup.json` or by `--rule`, or the pair spans two files for a rule that groups by file, and `unresolved`
means the entry names a suite or a row, or hooks whose chain the records do not show. `eligible` is
necessary, not sufficient: the records cannot show
whether `.each` rows belong to one declaration, or which hooks share a chain. Pending cases are
listed apart and left out of the per-rule figures.

A violation is data and exits 0; `--fail-on-violation` turns it into exit 1. A run that did not
measure the corpus (a case that cannot run, no summary record, a malformed record, zero extracted
tests, a binary that emits no test records) always fails. The evaluator is not part of
`npm run check`; the recall gate remains the enforced contract.

The fixtures are synthetic and contain no third-party source.
