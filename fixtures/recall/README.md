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

## Framework application shapes

Projects built on a framework with generated files and auto-imports reach the analyzer through
shapes that plain Vitest projects do not. Each case below holds a duplicate pair that is wanted once
the shape is understood, and a control that fails if the shape is handled by marking every helper
inert or by disabling detection. Names are invented, apart from a generated `.nuxt` directory where the shape depends on it. "Today" is what the shipped analyzer does;
a known miss records the wanted finding and the extraction counts it will change.

| Shape | Cases | Wanted | Today |
| --- | --- | --- | --- |
| Runner exported as `const f = ...; export default f`, or `export default f` before a hoisted `function f` | `framework-runner-default-const` (+ `-negative`), `framework-runner-default-function-declared-after` | the runner's test is extracted and pairs with the hand-written one; a pair that differs stays silent | as wanted: found, and silent when the bodies differ |
| Runner that is both `export const f` and `export default f` | `framework-runner-named-and-default` | found | as wanted: found |
| Default export that must stay refused: a `let` binding, a reassigned function declaration, an imported binding | `framework-runner-default-let-negative`, `framework-runner-default-reassigned-negative`, `framework-runner-default-imported-negative` | not compared, silent: the runner cannot be proven to be the declaration | as wanted: not compared. Controls for any change that accepts every default identifier |
| Runner exported as `export { f as default }`, or by name | `framework-runner-default-alias-control`, `framework-runner-named-control` | unchanged: found | found |
| Setup helper whose body calls an auto-imported global, branches, loops with `forEach` or `Object.keys(...).forEach`, assigns a member, spies, or declares a local constant | `framework-setup-helper-shapes` (one file per trait) | a helper that registers nothing and cannot change what a test computes does not exclude the file; identical tests are reported | as wanted: all seven identical pairs are reported (the fixture carries a generated `.nuxt/imports.d.ts`) |
| Same, empty helper | `framework-setup-helper-empty-control` | found | found |
| Helper that registers a per-test spy through hooks, called in one suite only | `framework-setup-helper-spy-context-negative` | the hooks are part of the context: the suites never pair | as wanted: compared and distinct. A control for any change that treats helpers as inert |
| Top-level call of an installed package's mocking API (`mockImport(name, factory)`, `registerEndpoint(url, handler)`) | `framework-package-mock-call` | static name plus inline factory read as mock declarations; identical pair reported | as wanted: the pair is reported |
| Same calls, package not installed | `framework-package-mock-call-uninstalled` | stays not compared, and the run names the missing package | not compared |
| Same calls, different factories in two files | `framework-package-mock-factory-differs-negative` | the factory is part of the context: never paired | as wanted: compared and distinct |
| Call into a mock module that runs `vi.hoisted`/`vi.mock` at module level | `framework-mock-module-effects` | module effects are context: each file's pair is reported, the two files never pair | as wanted: each file's pair is reported, the two files never pair |
| Same module imported for its effects only | `framework-mock-module-import-control` | each file's pair found; files with different mock modules never pair | pairs found; the two files are paired wrongly (known false positive) |
| Helper that registers `beforeAll`/`afterAll` over module state (server setup) | `framework-hook-helper-server` | hooks are context shared by both tests; pair reported | not compared |
| Same hooks with inline callbacks, no module state | `framework-hook-helper-inline-control` | found | found |
| Hook-registering helper called in one suite only | `framework-hook-helper-context-negative` | the hook is part of the context: never paired | compared, distinct |
| `extends` of a generated tsconfig whose `paths` map `~` and `~/*` to the parent directory | `framework-tsconfig-extends-generated` | alias followed: pair reported | found |
| Generated parent absent | `framework-tsconfig-extends-missing` | silent, and the run says which file is missing and what creates it | as wanted: silent, incomplete, the missing file named |
| Analysis root below the generated parent's directory | `framework-tsconfig-extends-outside-root` | silent, and the run names the alias target outside the root | silent; the run reports incomplete, and the excluded-file list names `import-outside-root` |
| Auto-imported name called at the top level of a test file | `framework-auto-import-top-level` | resolved to its export; pair reported | found (an undeclared name is ignored) |
| Auto-imported name called inside a setup helper | `framework-auto-import-in-helper` | resolved to its export; pair reported. A name declared nowhere stays not compared | as wanted: the pair is reported with a generated list; a name the list does not hold stays not compared |
| The same helper and tests without `.nuxt/imports.d.ts` (`nuxt prepare` not run) | `framework-auto-import-ungenerated-control` | not compared: nothing says what the name stands for | as wanted: not compared |
| Parent ignore file with an unanchored directory name | `framework-nested-repository-ignore-control` | see below | `api/` ignored, as git does |

A nested repository cannot be a fixture: git refuses to track a `.git` entry. The control case is the
same tree without one, where the parent's `api` rule rightly removes `project/api/`. To reproduce the
nested case, copy the case directory to a scratch location, run `git init` in the copy's outer
directory and again in `project/`, and analyze the outer directory: both `project/api/` and
`project/web/` must be analyzed (four tests, two in-file pairs and one cross-file pair), because the parent's rule stops at
the inner repository's root. Rust tests in `src/discovery.rs` build that layout at run time.

The cases assert today's counts (`expectedTests`, `expectedIncomplete`, `expectedUnresolved`), so a
fix that changes eligibility fails the case until its labels move. Under real Vitest (a scratch
config defining the auto-imported globals, and a `~` alias for the tsconfig case) every case passes
except the three that depend on something missing by design: the uninstalled package, the absent
generated parent and the analysis root below it.

## The floor

`minimumRecall` is the measured value rounded down, not the target: a gate that is red on arrival
gets routed around, after which the number it guards is worse than unmeasured. `recallDestination`
records the destination. The corpus currently detects 170 of the 171 findings its labels desire --
every `expectedFindings` entry matches; the denominator's one gap is the known miss of the framework
cases above -- so `minimumRecall` stays 0.99 and `knownMissBaseline` stays 1. Each change that finds one of them moves its entry from `knownMisses` to
`expectedFindings`, lowers the baseline and raises `minimumRecall` to the new measured value in the
same commit.

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

### Comparing two binaries

`--baseline-binary FILE` runs the same corpus through an older binary and classes every expectation
by what changed: `retained`, `newly-satisfied` (a wanted finding gained), `corrected` (a forbidden
or known false-positive finding gone), `regressed` (a wanted finding lost), `new-forbidden` (a
forbidden finding gained) or `still-unmet`. Findings nobody labelled are compared the same way. A
case the baseline cannot be run or read on (a crash, an old report schema, no extracted tests) is
`unavailable` with the reason; it is never read as zero findings, and a baseline that ran on no case
is an error. The baseline is not asked for `--emit-test-fingerprints`, since it may predate it.
Classes follow the labels as written: a known false positive whose `expectCount` the candidate no
longer meets leaves its overlapping `expectedAbsent` entry violated, and is reported as such.

Each run also states how many tests it left out of comparison because their context could not be resolved
(the summary's `unresolvedTests`). The comparison totals that figure over the cases both runs state it for,
lists the cases that leave more tests out than the baseline as regressions (a rise narrows what the detector
looks at without moving a finding, unless the case's labels pin exactly that count), and says `not measured` when no case states it in both runs, so a zero
over nothing is never read as a clean result.

`--baseline-ref REF` names the commit the baseline was built from. Cases that are new there, whose
expectations differ, or whose files or governing configuration (the nearest `.vitest-dedup.json`,
or the one `--config` names) differ are totalled apart, because a difference on them is not a
detector change. A case is taken to be self-contained: a change confined to a file it imports from
outside its own directory is not seen. The baseline tree is extracted with `git archive`, so `git`
and `tar` are needed. `--json` carries the whole comparison and a run record: both binaries by
sha256 and `--version`, the corpus by sha256, the checkout and baseline commits, and the arguments
each case ran with. `--fail-on-regression` exits 1 on a regression, a new unclaimed finding, a case that no longer
extracts what its labels say, a case that leaves more tests out of comparison, or an unavailable case; a baseline that cannot run a case new or changed since it was built stays data.

The fixtures are synthetic and contain no third-party source.
