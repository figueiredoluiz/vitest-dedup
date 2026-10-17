# Vitest Test Fixture Corpus

This directory contains a comprehensive fixture corpus of small Vitest test files for use in the static analyzer's test suite. Each file is minimal, focuses on a single concept, and includes a comment line describing what it exercises and what the extractor should produce.

## Fixture Index

| # | File | Category | Exercises | Expected Extraction |
|---|------|----------|-----------|---------------------|
| 1 | `declarations/basic-test-string-title.test.ts` | Declarations | `test('title', fn)` with string title, named import | 1 test 'adds two numbers' |
| 2 | `declarations/basic-it-alias.test.ts` | Declarations | `it` as alias for `test`, same interchangeability | 1 test 'multiplies two numbers' via `it` |
| 3 | `declarations/describe-suite-alias.test.ts` | Declarations | `describe` and `suite` both used, interchangeability | 2 suites using `describe` and `suite` interchangeably |
| 4 | `declarations/nested-describe.test.ts` | Declarations | Multiple nested `describe` levels (3 deep) | Suite paths: ['Utils'], ['Utils', 'Math'], ['Utils', 'Math', 'Arithmetic'] |
| 5 | `declarations/template-literal-title-static.test.ts` | Declarations | Template literal with no substitutions | 1 test with static literal title |
| 6 | `declarations/template-literal-title-dynamic.test.ts` | Declarations | Template literal with runtime variable substitution | 1 test with dynamic title (unresolvable) |
| 7 | `declarations/template-literal-title-const-fold.test.ts` | Declarations | Template literal with const-foldable expression `${1+1}` | 1 test with static title 'title with 2' |
| 8 | `declarations/named-function-title.test.ts` | Declarations | Title derived from function name via `test(function myTestName() {})` | 1 test titled 'myTestName' |
| 9 | `declarations/options-object-2nd-arg.test.ts` | Declarations | Options object as 2nd argument, fn as 3rd | 1 test with timeout and retry options |
| 10 | `modifiers/modifier-skip.test.ts` | Modifiers | `test.skip` and `describe.skip` modifiers | 1 skipped test, 1 skipped suite |
| 11 | `modifiers/modifier-only.test.ts` | Modifiers | `test.only` and `describe.only` modifiers | 1 test with Only modifier, 1 suite with Only |
| 12 | `modifiers/modifier-todo.test.ts` | Modifiers | `test.todo` and `describe.todo` (no body) | 1 todo test, 1 todo suite |
| 13 | `modifiers/modifier-fails.test.ts` | Modifiers | `test.fails` modifier | 1 test with Fails modifier |
| 14 | `modifiers/modifier-concurrent-order-a.test.ts` | Modifiers | `test.skip.concurrent(...)` chain order | 1 test with Skip + Concurrent modifiers |
| 15 | `modifiers/modifier-concurrent-order-b.test.ts` | Modifiers | `test.concurrent.skip(...)` opposite chain order | 1 test with Concurrent + Skip (same semantics) |
| 16 | `modifiers/modifier-concurrent-describe.test.ts` | Modifiers | `describe.concurrent.only(...)` on suite | 1 suite with Concurrent + Only modifiers |
| 17 | `modifiers/modifier-skipif.test.ts` | Modifiers | `test.skipIf(cond)` conditional skip | 1 test with skipIf modifier |
| 18 | `modifiers/modifier-runif.test.ts` | Modifiers | `describe.runIf(cond)` inverse of skipIf | 1 suite with runIf modifier |
| 19 | `modifiers/modifier-sequential-legacy.test.ts` | Modifiers | `test.sequential(...)` pre-v5 syntax | 1 test with Sequential modifier (legacy) |
| 20 | `modifiers/modifier-shuffle-describe.test.ts` | Modifiers | `describe.shuffle(...)` randomize execution order | 1 suite with Shuffle modifier |
| 21 | `each/each-array-of-arrays.test.ts` | Each | `.each` with array of arrays, printf directives | 3 parameterized tests from literal table |
| 22 | `each/each-array-of-objects.test.ts` | Each | `.each` with array of objects, `$field` interpolation | 2 parameterized tests from object array |
| 23 | `each/each-tagged-template.test.ts` | Each | `.each` with tagged template literal table | 2 parameterized tests from tagged template |
| 24 | `each/each-printf-directives.test.ts` | Each | All printf directives: `%s %d %i %f %j %o %# %$ %%` | 1 parameterized test using all directives |
| 25 | `each/each-dollar-indexed.test.ts` | Each | `$0`, `$1` positional and `$field.nested` dotted paths | 2 parameterized tests with nested property access |
| 26 | `each/each-nonliteral-table.test.ts` | Each | `.each(getCases())` with non-literal table argument | Parameterized test marked as unresolved (table not static) |
| 27 | `each/for-array-rows.test.ts` | Each | `.for` with array rows (no spreading), destructured | 2 parameterized tests via `.for` |
| 28 | `each/for-with-context.test.ts` | Each | `.for` with TestContext as 2nd param for concurrent | 1 concurrent parameterized test |
| 29 | `each/describe-each.test.ts` | Each | `describe.each` generates multiple suites | 2 generated suites with nested tests |
| 30 | `hooks/hooks-file-level.test.ts` | Hooks | Top-level `beforeAll/afterAll/beforeEach/afterEach` | Hooks at file scope, 1 test with hook context |
| 31 | `hooks/hooks-nested-order.test.ts` | Hooks | Hooks at file level and 2 nested describe levels | Verifies outer-to-inner and inner-to-outer ordering |
| 32 | `hooks/hooks-beforeeach-returns-cleanup.test.ts` | Hooks | `beforeEach` returning cleanup function | 1 test with cleanup-returning hook |
| 33 | `hooks/hooks-ontestfinished.test.ts` | Hooks | `onTestFinished` called inside test body | 1 test using onTestFinished hook |
| 34 | `hooks/hooks-ontestfailed.test.ts` | Hooks | `onTestFailed` called inside test body | 1 test using onTestFailed hook |
| 35 | `fixtures/fixtures-basic-extend.test.ts` | Fixtures | `test.extend` with 1 fixture (object-syntax) | Extended test with `db` fixture |
| 36 | `fixtures/fixtures-dependency-chain.test.ts` | Fixtures | Fixture depending on another via `.extend().extend()` | Extended fixtures with dependency chain |
| 37 | `fixtures/fixtures-builder-syntax.test.ts` | Fixtures | Builder-syntax fixture `.extend('name', async (deps, {onCleanup}) => value)` | Builder-form fixture with onCleanup |
| 38 | `fixtures/fixtures-scope-file.test.ts` | Fixtures | Fixture with `{ scope: 'file' }` | File-scoped fixture shared across tests |
| 39 | `fixtures/fixtures-auto-true.test.ts` | Fixtures | Fixture with `{ auto: true }` initialized unconditionally | Auto-initialized fixture |
| 40 | `fixtures/fixtures-injected.test.ts` | Fixtures | Fixture with `{ injected: true }` external value | Injected fixture from config |
| 41 | `fixtures/fixtures-override.test.ts` | Fixtures | `test.override(...)` at top of describe block | Suite-level fixture override |
| 42 | `fixtures/fixtures-scoped-hooks.test.ts` | Fixtures | `test.beforeEach/afterEach` on extended test binding | Scoped hooks tied to extended test |
| 43 | `resolution/imports-named.test.ts` | Resolution | Named imports from `'vitest'` | Tests resolved via named import bindings |
| 44 | `resolution/imports-renamed.test.ts` | Resolution | Renamed imports `import { test as t, ... }` | Tests resolved via renamed bindings |
| 45 | `resolution/imports-namespace.test.ts` | Resolution | Namespace import `import * as vitest from 'vitest'` | Tests accessed as `vitest.test(...)` |
| 46 | `resolution/globals-ambient.test.ts` | Resolution | Bare `test/describe/expect` (no import), requires `globals: true` config | Ambient globals + paired vitest.config.ts |
| 47 | `resolution/globals-per-project.test.ts` | Resolution | Ambient globals resolvable only via project-specific `globals: true` | Per-project globals + paired vitest.config.ts |
| 48 | `resolution/mixed-file-vitest-and-helper-named-test.test.ts` | Resolution | Local variable shadowing `test` import name | Tests using aliased import + shadowed local var |
| 49 | `insource/in-source-basic.ts` | In-source | `if (import.meta.vitest)` block with destructured `{ it, expect }` | In-source test extracted + paired vitest.config.ts |
| 50 | `insource/in-source-not-included.ts` | In-source | In-source test code NOT matched by `includeSource` glob | File skipped (not in includeSource pattern) |
| 51 | `declarations/bench-v5-fixture-style.test.ts` | Declarations | `bench` as TestContext fixture (v5 shape) `test('name', ({bench}) => {})` | 1 test with nested bench |
| 52 | `declarations/bench-legacy-toplevel.bench.ts` | Declarations | `bench` as top-level import (v2-v4 shape) with `.skip` | 1 bench with Skip modifier |
| 53 | `declarations/legacy-numeric-timeout.test.ts` | Declarations | Legacy 3-arg form `test('name', fn, 5000)` | 1 test with timeout as 3rd arg |
| 54 | `declarations/test-context-destructured.test.ts` | Declarations | Destructured TestContext `test('name', ({expect, skip, task}) => {})` | 1 test accessing destructured context |
| 55 | `declarations/options-object-modifiers.test.ts` | Declarations | Options object with modifier flags `{ skip: true }` | 1 test with Skip via options + chained flags |
| 56 | `edge/vi-mock-hoisting.test.ts` | Edge | `vi.mock()` placed after other statements, hoisting understood | Mock call hoisting recognized |
| 57 | `edge/vi-hoisted-shared.test.ts` | Edge | `vi.hoisted(() => {...})` referenced in later `vi.mock` | Shared hoisted mocks |
| 58 | `discovery/discovery-include-default.test.ts` | Discovery | File matching default `include` glob pattern | Standard test file matching default glob |
| 59 | `discovery/faux_node_modules/excluded.test.ts` | Discovery | Test under faux `node_modules/` excluded by default | File matched by default `exclude` glob |
| 60 | `discovery/discovery-custom-include.spec.js` | Discovery | Non-default glob `.spec.js` requires config override | Custom include glob + paired vitest.config.ts |
| 61 | `workspace/workspace-legacy.workspace.ts` | Workspace | Legacy `vitest.workspace.ts` (deprecated v3.2+, still functional) | Multiple projects defined in legacy format |
| 62 | `workspace/projects-inline-vs-referenced-vitest.config.ts` | Workspace | `test.projects` with glob-string and inline-object entries | Projects from glob patterns + inline configs |
| 63 | `lookalikes/lookalike-playwright.spec.ts` | Lookalikes | `import { test } from '@playwright/test'` — NOT Vitest | Must be excluded (Playwright import) |
| 64 | `lookalikes/lookalike-jest-globals.test.ts` | Lookalikes | `import { test } from '@jest/globals'` — NOT Vitest | Must be excluded (Jest import) |
| 65 | `lookalikes/lookalike-cypress.cy.ts` | Lookalikes | Ambient `describe/it` + `cy.*` commands — NOT Vitest | Must be excluded (Cypress-specific) |
| 66 | `lookalikes/lookalike-node-test.test.mjs` | Lookalikes | `import { test } from 'node:test'` — NOT Vitest | Must be excluded (node:test builtin) |
| 67 | `lookalikes/lookalike-mocha-before-after.test.ts` | Lookalikes | Ambient `describe/it` with `before/after` (not Vitest hooks) — NOT Vitest | Must be excluded (Mocha, wrong hook names) |
| (a) | `duplicates/duplicates-same-context-a.test.ts` | Duplicates | Two tests, identical bodies, identical hook context | **duplicate-test** (genuine duplicates) |
| (b) | `duplicates/not-duplicates-different-hooks.test.ts` | Duplicates | Two tests, identical bodies, different beforeEach hooks | NOT a duplicate (hook context changes behavior) |
| (c) | `duplicates/not-duplicates-different-mocks.test.ts` | Duplicates | Two tests, identical bodies, different vi.mock calls | NOT a duplicate (mocked behavior differs) |
| (d) | `duplicates/not-duplicates-literal-variation.test.ts` | Duplicates | Two tests, identical bodies differing only in literal values | NOT a duplicate (parameterization candidate for .each) |
| (e) | `duplicates/duplicate-test-name.test.ts` | Duplicates | Two tests with same title but different bodies | **duplicate-test-name** (not duplicate-test) |
| (f) | `duplicates/duplicates-one-skipped.test.ts` | Duplicates | Two identical tests where one is .skip | **duplicate-test** (still duplicates; one is inert) |
| (g) | `duplicates/duplicates-different-suites-no-hooks.test.ts` | Duplicates | Two identical tests in same file under different suite paths (no hooks) | **duplicate-test** (suite path differs but behavior doesn't) |
| (h) | `duplicates/duplicates-alpha-equivalent.test.ts` | Duplicates | Two tests with alpha-equivalent bodies (same structure, different var names) | **duplicate-test** (alpha-normalization catches this) |
| (i) | `duplicates/not-duplicates-local-vs-imported-name.test.ts` | Duplicates | Two tests, one with local `page`, other with imported `page` | NOT a duplicate (**REGRESSION**: semantic parser, not name-based) |
| (j) | `duplicates/not-duplicates-different-helper-modules.test.ts` | Duplicates | Two tests calling same-named helper from DIFFERENT modules | NOT a duplicate (helpers come from different modules) |

## Directory Structure

- **declarations/** - Basic test/it/describe/suite forms, signatures, contexts, options, bench forms (15 files)
- **modifiers/** - Skip, only, todo, fails, concurrent, skipIf, runIf, sequential, shuffle chains (11 files)
- **each/** - .each and .for table forms, title interpolation, describe.each (9 files)
- **hooks/** - beforeAll/beforeEach/afterEach/afterAll, nesting, cleanup, onTestFinished/Failed (6 files)
- **fixtures/** - test.extend, dependency chains, scopes, override, scoped hooks (8 files)
- **resolution/** - Named/renamed/namespace imports, globals config, shadowing (8 files)
- **insource/** - import.meta.vitest patterns, includeSource globs (3 files + 1 config)
- **lookalikes/** - Playwright, Jest, Cypress, node:test, Mocha (5 files to exclude)
- **discovery/** - include/exclude glob patterns, custom configs (3 files + 1 config + faux node_modules/)
- **workspace/** - Legacy workspace.ts, test.projects configurations (2 files + config)
- **edge/** - vi.mock hoisting, vi.hoisted patterns, doMock/unmock/doUnmock scoping (5 files)
- **duplicates/** - Core test deduplication rule discrimination cases (162 files). The
  index above documents the first ten; the rest are self-documenting through their headers and
  their `@vitest-dedup:` blocks, which are what the gate reads.
- **mixed/** - Cases where TWO named defects interact in one file (5 files). Admission
  rule: the file must exercise two defects cited by S-number from `backlog.local.md`, and its
  header must say what a fix for either one could break in the other. A fixture holding one
  defect belongs in `duplicates/`. On the blocking path via `check-duplicates-corpus.mjs`
  `TARGETS` and `check-fixture-isolation.mjs` `DIRS`. Cross-file identity is run-scoped and
  each `TARGETS` directory is one run, so a `mixed/` body can only collide with another
  `mixed/` body -- and every collision must be declared in both files, as in `duplicates/`.
  A case needing its own `vitest.config.ts` cannot live here (the gate's solo run copies a
  bare `.test.ts`); those go to `recall/`.
- **observed/** - Distilled reproductions of shapes found on the external corpus (see below)
- **near/** - Fixtures pending an unimplemented rule or an analyzer fix, outside the finding gate
- **recall/** - The labelled recall set: one self-contained case directory per rule behaviour, a
  positive and a negative for every rule plus its variants, behind a recall floor and a known-miss
  baseline. Checked by `scripts/check/check-recall.mjs`, **not** by
  `check-duplicates-corpus.mjs` — it uses a JSON manifest rather than `@vitest-dedup:` directive
  blocks, because an expectation here is per *case directory* (extraction count, run completeness,
  CLI arguments, desired-but-missing findings) rather than per file. See
  `fixtures/recall/README.md` for why known-positive retention is not recall -- its labels are
  selected by the detector being evaluated -- and this directory is the difference.

## Implementation Notes

1. **Syntactic Validity**: All fixture files are syntactically valid JavaScript/TypeScript. Some imports may be from packages not installed, which is intentional (e.g., `@playwright/test` for lookalikes).

2. **Minimal Code**: Each fixture is 5-30 lines, focusing on a single extraction concept.

3. **Comment Headers**: Every file starts with a `//` comment describing what it exercises and what the extractor should produce.

4. **Config Pairing**: Some fixtures require paired config files (e.g., `globals-ambient.test.ts` + `globals-ambient-vitest.config.ts`) to be resolvable. These are included in the same directory.

5. **Non-Vitest Files**: Lookalike files intentionally import from competing frameworks to test specifier-based exclusion logic.

6. **Table Files**: For `.each`, both literal and non-literal table forms are represented. Non-literal tables degrade to "unresolved parameterization."

7. **Fixture Exhaustiveness**: The 68 original plan items are all represented; additional paired config files (vitest.config.ts, workspace.ts) are included where needed for extraction context.

## Duplicate-discrimination fixtures

`fixtures/duplicates/` is the precision and recall gate for the analysis rules, and is read
differently from the extraction fixtures above. It is **not** read by filename: each file carries a
machine-readable `@vitest-dedup:` directive block in its header comment, parsed by
`scripts/check/lib/fixture-directives.mjs` and checked by `scripts/check/check-duplicates-corpus.mjs`
(plus its companion `check-fixture-isolation.mjs`). The `duplicates-*` / `not-duplicates-*` naming
habit below is kept for human readability, not as the gate's source of truth — a filename that
doesn't match a fixture's actual `@vitest-dedup:expect` block is a fixture bug, not a gate bug.

A fixture declares, per finding it must produce (or must not):

- `@vitest-dedup:extracted <N>` — how many tests must be extracted from the file alone.
- `@vitest-dedup:expect rule=<rule-id|none> verdict=<verdict> severity=<severity|none>` — the rule
  that must fire (or `rule=none` for a pair that must produce no finding), naming its verdict from
  the fixed verdict vocabulary (`redundant`, `consolidatable`,
  `name-collision`, `similar-uncertain`, `distinct`, `not-compared`) -- never merely "duplicate",
  which conflates three claims with three different remedies.
- `@vitest-dedup:participant L<line> "<full title>"` — one line per test the expectation is about.
- `@vitest-dedup:isolates <dimension>` (required on `rule=none`) — the one variable a negative
  fixture claims to isolate, checked independently by the isolation gate against the analyzer's own
  per-test fingerprints. The three opt-outs are not interchangeable: `unchecked <reason>` is a real
  isolation claim with no mechanism to state it yet (debt, and the reason must name the missing
  mechanism); `unstateable <reason>` is a real isolation claim whose AGREEMENT half names a view the
  fingerprint stream does not emit (not debt at this surface — the reason names the missing view
  and the analyzer event that produces it, and the gate fails the moment the recorded exit, plain
  `context`, becomes stateable); and `not-isolable <reason>` is for an expectation that is not an
  isolation claim at all (policy silencing, participants the analyzer cannot distinguish by
  construction, or a property over a cluster rather than a pair).
- `@vitest-dedup:pending <reason>` — marks ONE expectation (the `expect` entry it sits directly
  under) whose true verdict today's binary can't produce. The corpus gate runs it as a one-way
  ratchet: a pending expectation must NOT be satisfied today — while the defect it names is still
  present, the gate holds, counts it, and prints it — and it fails the moment the expectation
  starts being satisfied, demanding the marker's removal so the verdict becomes a hard assertion.
  A pending expectation therefore pins the *defect*, never the bug as correct behaviour, and a
  fixture can carry one beside a live twin (`fixtures/mixed/` exists for exactly that shape).
  `@vitest-dedup:not-runnable <reason>` — metadata for a fixture that can't run under real
  Vitest even though it analyzes correctly; **inert: no gate reads it.** It is parsed so a gate
  that wants it later need not invent a second parser. What the corpus gate reports as "pending"
  is a different thing entirely — every file in `fixtures/near/`, by directory, regardless of
  directives. **`fixtures/near/` is not scanned by the corpus gate at all**: neither its findings
  nor its per-expectation `pending` markers are asserted or ratcheted there (the isolation gate
  still parses and checks those files). A fixture whose waiting needs to be *enforced* belongs in
  an asserted directory with a `pending` marker, not in `fixtures/near/`.

A fixture with no block, or one that fails to parse, is a **gate failure**, never a silent pass —
see `CONTRIBUTING.md`'s "Fixture conventions" for the full directive reference before adding one.
`duplicates-*`/`not-duplicates-*` as shorthand for the common case still mean roughly what they
say:

- `duplicates-*` — MUST produce a finding. A silent one is a recall regression.
- `not-duplicates-*` — MUST produce no finding. A firing one is a precision regression, and
  `duplicate-test` defaults to error severity, so it fails builds.

Several exist specifically because an implementation once got them wrong, and each such file
records the defect it guards in its header. Three are worth knowing about as a set, because they
cover failure modes that are easy to reintroduce:

| Fixture | Guards against |
| --- | --- |
| `not-duplicates-literal-kind-*` | a serializer that keeps a literal's text but drops its kind, making `true` and `'true'` compare equal |
| `not-duplicates-literal-empty-string-vs-empty-array` | an empty string literal serializing to nothing, making `['']` identical to `[]` |
| `not-duplicates-sibling-describes-shadowed-const` | free-identifier identity keyed on name rather than on the binding, collapsing same-named constants in sibling scopes |

Two fixtures form a deliberate matched pair and should be read together:
`duplicates-same-fixture-different-label` and `not-duplicates-sibling-describes-shadowed-const`
have the same outward shape — identical bodies under two sibling `describe` blocks — and opposite
verdicts, because one pair's describes bind nothing and the other's bind different values. A suite
path never decides the verdict on its own; what the bodies resolve to does.

A caution when writing new ones: a fixture that varies a value while holding its *kind* constant
cannot detect kind erasure, because both sides degrade identically. That is exactly why
`not-duplicates-literal-variation` (two numbers) stayed green while the property it appeared to
test was broken. When guarding a distinction, vary the thing the implementation might collapse.

### Incidental cross-file collisions

This corpus is built out of matched pairs: a positive fixture and a negative one that differ in
exactly one variable. That construction guarantees bodies repeat across files — which is precisely
what `cross-file-duplicate` reports. Enabling it over this directory produces **eight** findings:
the `cross-file-duplicate-would-fire-{a,b}` pair, which exists for it, and seven edges between
fixtures that were written for other rules entirely.

All eight are declared, in both participating files, and that is the point. While the rule shipped
`Off` the seven were dodged by scoping the gate's `--rule` override to `fixtures/observed/`; with
the rule shipping `Warning` there is nothing left to scope, and the gate's unclaimed-finding sweep
fails on any finding no fixture accounts for. Dodging would have meant either turning the sweep off
for one rule or keeping the corpus's own default run untested.

Read each declaration as a statement about the **rule**, not as advice about the fixtures. Every
one is a true positive by the rule's criteria — the same alpha-equal body under an equivalent
context, in two files — and each header says which pair collides and why. The remedy the
`redundant` verdict names (delete one side) does not apply: both files in each pair are
load-bearing, and deleting either would delete a claim. A rule's verdict describes what a finding
asserts about the code; whether the code wants that remedy is a judgement the corpus makes here on
the reader's behalf, once, in writing.

Two consequences for anyone adding a fixture to this directory:

- A new fixture whose body matches an existing one produces a new `cross-file-duplicate` finding
  and the gate fails until it is declared, in both files. That is working as intended; declare it
  or vary the body.
- The collisions are asserted at `severity=warning` under the **default** config — no `--rule`
  override is passed for this directory or for `fixtures/observed/`. Reverting the rule's default
  to `Off` therefore fails eight expectations here, which is how a shipped default stays pinned by
  something other than a table in a document.

## `fixtures/observed/` — reproductions of what the external corpus found

`fixtures/duplicates/` encodes failure modes someone imagined. `fixtures/observed/` encodes ones
that were *found*, on real third-party code: each fixture is a small, self-contained reproduction
of a shape the analyzer met on a pinned external corpus of six public repositories. Those
repositories are benchmarks, not dependencies: they are named only in a local ledger, they are not a
blocking gate (running them at all is future work), and this directory is the owned, offline control
that keeps what they taught pinned:
a third party's repository can change under us for reasons unrelated to this tool, and a fixture
cannot discover a new shape, so both exist and only this one blocks a merge.

It is read by `scripts/check/check-duplicates-corpus.mjs` on exactly the same terms as
`fixtures/duplicates/`: the same `@vitest-dedup:` directive block, the same per-fixture extraction
count, the same `distinct`/`not-compared` eligibility check, the same unclaimed-finding sweep, the
same isolation gate, and now the same `TARGETS` row — both directories are analyzed with
`--rule subsumed-test=warning` and nothing else. The `--rule cross-file-duplicate=warning`
override this directory used to carry is gone: the rule ships `Warning`, so both directories'
`cross-file-duplicate` expectations are now assertions about the **default** config, and an
override would have hidden a reverted default behind a flag.

Three conventions specific to this directory:

- **The header names its source.** Which project, which file, what the analyzer did wrong there,
  and what it must do now. A fixture whose header cannot answer "observed where?" belongs in
  `fixtures/duplicates/`.
- **Reproduce the shape, never the source.** These distil a pattern; they do not copy substantial
  code from the projects they came from.
- **A negative must be load-bearing, and usually needs a twin.** Either the isolation gate can
  express its claim, or its opt-out reason records what carries it instead: an `unchecked` reason
  names a manual weakening — an edit that makes the analyzer fire — that was actually run and can
  be re-run, and an `unstateable` reason names the missing body-level view the gate ratchets on.
  And because a precision fix
  and a deleted rule produce identical silence, most negatives here ship with a positive twin:
  `observed-type-args-differ` with `observed-type-args-same`, `observed-mixed-each-table` with
  `observed-uniform-each-table`.
