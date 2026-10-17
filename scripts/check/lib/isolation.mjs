#!/usr/bin/env node
// The isolation property for negative fixtures, checked against the analyzer's own fingerprints.
//
// THE DEFECT CLASS THIS EXISTS TO CATCH
// ------------------------------------
// A `rule=none` fixture passes by nothing happening. That makes every negative fixture a candidate
// for silent success, the failure mode the work-performed floor exists to catch: it can stop
// discriminating and still pass forever, exactly like a sanitizer test that picks a character inside the old range.
//
// `near-arrow-vs-function-this-twin.test.ts` is the worked example. It claimed to be the
// precondition-violating twin for the arrow-vs-`function` transformation, isolating `this` binding
// -- but its two bodies were `let value = 0; value = 5; expect(value).toBe(5)` and
// `this.value = 5; expect(this.value).toBe(5)`. Those differ in CONTENT. Their fingerprints
// diverged for reasons having nothing to do with `this`, so a regression that collapsed lexical and
// dynamic `this` would have left the `rule=none` assertion green. The fixture could not fail for
// its stated reason.
//
// The property that catches it: **a negative fixture that claims to isolate one variable must have
// participants whose fingerprints agree on everything else, and disagree exactly where the analyzer
// is supposed to see that variable.** Declare the variable; the gate checks both halves.
//
// WHAT THIS CHECKS AGAINST
// ------------------------
// `vitest-dedup <dir> --reporters jsonl --emit-test-fingerprints` emits one `"test"` record per
// analyzed test -- crucially, *including* the tests that appear in no finding, which is by
// definition what a `rule=none` fixture's participants are. Each record carries the analyzer's own
// normalized views, as hashes that may only be compared for equality:
//
//   alpha        identifiers normalized (locals renamed positionally; free identifiers keyed on
//                the binding they resolve to, not on their spelling), literal VALUES preserved.
//   structural   `alpha` with literal values collapsed to their KIND. So `structural` equal means
//                "identical but for literal values"; a literal whose *kind* changes still moves it.
//   behavior     the ordered behavior events, syntax erased. Not a coarsening of `structural`:
//                it drops syntax that `structural` keeps and keeps literal payloads that
//                `structural` drops, so the two can disagree in either direction.
//   trivial      the body is empty or a recognized stub, and is excluded from comparison entirely.
//   contextKey   the ambient equality key every context-sensitive rule groups on (hooks, mocks,
//                `test.extend` fixtures, import origins).
//   modifierKey  the behavior-significant subset of a test's modifiers (`.fails`, `.skipIf`, an
//                `.each` table's identity), as the rules key it.
//
// This replaced a source-level comparison that erased a declared dimension from both participants'
// tokenized callbacks and required the rest to match. That was an approximation of the analyzer,
// deliberately stricter than it and never looser, and it could not express most of the corpus's
// claims -- 29 of 61 negative expectations opted out of it. It is gone: the oracle here is now the
// analyzer itself, so a translation either states a fixture's claim exactly or the fixture opts out.
//
// WHY EVERY DIMENSION NAMES BOTH AN "EQUAL" AND A "DIFFER" SET
// -----------------------------------------------------------
// One half alone is always trivial, in one of two ways, and this repository has shipped both:
//
//   - Only equalities: "these two agree on X" is satisfied by two tests that are outright
//     identical, which proves nothing about the variable the fixture names.
//   - Only inequalities: "these two differ somewhere" is satisfied by any two unrelated tests.
//
// Together they are falsifiable from both sides. The DIFFER set is the guard against the
// regression the fixture exists for: it names the view where the analyzer must still see the
// isolated variable, so an analyzer change that stops distinguishing it turns a required DIFF into
// an EQ and fails this gate *before* the pair starts being reported as a duplicate. The EQUAL set
// is the guard against fixture drift: it names what the pair must still share, so an edit that
// introduces a second difference fails here rather than silently making the fixture prove less.
//
// A dimension that can supply only one half is not a check, and the vocabulary does not contain
// one. Where a fixture's claim cannot supply both, it opts out -- see UNCHECKED / UNSTATEABLE /
// NOT_ISOLABLE.
import { basename } from 'node:path'
import { runAnalyzerJsonl } from './run-analyzer.mjs'

/**
 * The closed vocabulary of variables a negative fixture may declare it isolates.
 *
 * Each entry translates one variable into a relation over the analyzer's emitted views: `equal`
 * names the views that must match across every participant, `differ` names the views that must
 * differ between every pair of them. `why` is the translation's justification and is printed in
 * failure messages, because a reader hitting a failure needs to know what the assertion was
 * derived from, not just that it failed.
 *
 * Two dimensions may share a relation without being the same claim; what differs is the regression
 * each one guards. See `binding` versus `identifier` below for the case that matters most.
 */
export const DIMENSIONS = new Map([
  [
    'object-property',
    {
      summary: 'an object property differs in accessor, method or prototype-setter semantics',
      equal: ['contextKey', 'modifierKey'],
      differ: ['alpha', 'structural'],
      why:
        'Accessors, concise methods, data properties and prototype setters have different ' +
        'runtime descriptor semantics. That distinction must survive exact and structural ' +
        'serialization even when their callable bodies or initializer values are identical.',
    },
  ],
  [
    'for-await',
    {
      summary: 'a for-of loop differs in whether it awaits each element (for await vs for)',
      equal: ['contextKey', 'modifierKey'],
      differ: ['alpha', 'structural', 'behavior'],
      why:
        '`for await` selects the async iterator protocol, falling back to a wrapped sync ' +
        'iterator that awaits Promise-valued elements. Ordinary `for` selects the sync ' +
        'protocol. This difference must survive exact, structural, and behavior comparison ' +
        'to prevent false duplicate and near-duplicate findings; both callbacks are async ' +
        'so callback asyncness cannot carry the distinction.',
    },
  ],
  [
    'literal',
    {
      summary: 'literal VALUES differ, of the same kind; nothing else does',
      equal: ['structural', 'contextKey', 'modifierKey'],
      differ: ['alpha'],
      why:
        '`structural` is `alpha` with literal values collapsed to their kind, so `structural` ' +
        'equal is exactly the claim "identical but for literal values" and `alpha` differing is ' +
        'the claim that the values really do differ. `behavior` is deliberately NOT constrained: ' +
        'a literal that is an assertion argument is a behavior-event payload, so two rows of one ' +
        'table that vary input AND expected legitimately differ there ' +
        '(each-differing-input-expected-pairs.test.ts) while two that vary only the input do not ' +
        '(each-row-distinct-rows.test.ts). Constraining it would reject one of those two.',
    },
  ],
  [
    'literal-kind',
    {
      summary: 'two literals of different KINDS stand in the same position',
      equal: ['contextKey', 'modifierKey'],
      differ: ['alpha', 'structural'],
      why:
        'the kind of a literal is the one thing `structural` keeps after erasing its value, so ' +
        '`structural` differing IS the property these fixtures guard: a serializer that collapsed ' +
        "`1` with `'a'`, `true` with `'true'`, or `''` with `[]` (a literal that serializes to " +
        'NOTHING) would make `structural` equal here and the pair would be reported as a ' +
        'parameterization candidate at warning severity. Distinct from `literal`, which requires ' +
        'the opposite relation on the same field.',
    },
  ],
  [
    'identifier',
    {
      summary: 'the participants name different things: different subjects, helpers, or matchers',
      equal: ['contextKey', 'modifierKey'],
      differ: ['alpha', 'structural'],
      why:
        'a free identifier keeps its binding identity through `alpha` (only locals are renamed ' +
        'positionally), so two bodies that call different helpers or assert about different ' +
        'subjects must diverge at `alpha` and at `structural`. Weakening such a fixture into ' +
        'naming the SAME subject twice collapses both and fails here.',
    },
  ],
  [
    'binding',
    {
      summary: 'the same source text resolves to a different symbol (shadowing, local vs imported)',
      equal: ['modifierKey'],
      differ: ['alpha', 'structural'],
      why:
        'the relation matches `identifier`, the claim does not, and the difference is the whole ' +
        'point of the fixtures that declare it. Under `identifier` the two bodies are spelled ' +
        'differently, so `alpha` would differ even for an analyzer that keyed free identifiers on ' +
        'their NAME. Under `binding` the two bodies are spelled IDENTICALLY -- `formatLabel(x)` ' +
        'against `formatLabel(x)`, `format(d)` against `format(d)` -- and only the symbol behind ' +
        'the text differs, so `alpha` differing is a statement about symbol resolution and ' +
        'nothing else. It is the assertion that fails the moment `frontend/fingerprint.rs` goes ' +
        'back to name-keyed free identity, which is a bug it has actually shipped (scheme 2 in ' +
        'that module\'s history: sibling `describe`s each declaring their own `commonParams` ' +
        'collapsed into one identity and were reported at error severity). `contextKey` is NOT ' +
        'required equal: resolving to an import rather than a local changes the ambient import ' +
        'set as well, so the two shapes of this claim disagree there and constraining it would ' +
        'reject the import-origin half.',
    },
  ],
  [
    'type-arguments',
    {
      summary: 'an explicit type-argument list differs in arity, or is present on only one side',
      equal: ['contextKey', 'modifierKey'],
      differ: ['alpha', 'structural'],
      why:
        'type arguments are part of the call expression the serializer records, so two calls that ' +
        'differ only there must still diverge at `alpha`. These pairs are otherwise byte-identical ' +
        'JavaScript: erase the type arguments and the analyzer reports `duplicate-test` at error ' +
        'severity, which is exactly what a regression dropping type arguments from the fingerprint ' +
        'would do -- and what this relation catches, because it turns both required DIFFs into EQs.',
    },
  ],
  [
    'statement-order',
    {
      summary: 'the same top-level statements appear in a different order',
      equal: ['contextKey', 'modifierKey'],
      differ: ['alpha', 'structural', 'behavior'],
      why:
        'no emitted view is order-insensitive -- `behavior` is an ORDERED event list -- so a ' +
        'reorder must reach all three. `behavior` is included here and nowhere else: it is the ' +
        'view the near-duplicate rule keys on, and these fixtures exist to pin that a reorder ' +
        'whose statements are NOT independent stays visible there rather than being normalized ' +
        'away with the independent ones.',
    },
  ],
  [
    'context',
    {
      summary:
        'the surrounding context differs (hooks, vi.mock factories, test.extend fixtures) and ' +
        'nothing else does',
      equal: ['alpha', 'structural', 'behavior', 'modifierKey'],
      differ: ['contextKey'],
      why:
        'the strongest agreement statement the stream can make: the two bodies are identical to ' +
        'the analyzer at every level, so the ONLY thing that can be keeping them apart is the ' +
        'ambient context key. A fixture declaring this that drifts into differing bodies fails on ' +
        'the equalities; an analyzer that stops folding a hook, a mock factory, or a fixture ' +
        'binding into `ContextKey` fails on the inequality and the pair starts being reported.',
    },
  ],
  [
    'modifier',
    {
      summary: 'the declaration modifier differs (.fails/.skipIf/.each identity) and nothing else',
      equal: ['alpha', 'structural', 'behavior', 'contextKey'],
      differ: ['modifierKey'],
      why:
        "`context`'s relation with the two keys exchanged, for the same reason. `.fails` inverts " +
        'what an identical body asserts and `.skipIf` runs it under a different condition, so ' +
        '`ModifierKey` is the only thing that may separate these pairs -- including the inherited ' +
        'case, where a suite\'s own `.skipIf` must reach the nested test\'s key (F3).',
    },
  ],
  [
    'callback-form',
    {
      summary: 'arrow function vs `function` expression, over a body that reads `this`',
      equal: ['behavior', 'contextKey', 'modifierKey'],
      differ: ['alpha', 'structural'],
      why:
        'the fixture this gate was written for. `Serializer::ser_callable` collapses arrow and ' +
        '`function` forms unconditionally -- correct and desirable, since rewriting one as the ' +
        'other is the commonest cosmetic difference between two copies of a test -- but only ' +
        'while the body reads neither `this` nor `arguments`. Here it reads `this`, so the two ' +
        'forms are NOT interchangeable and the serializer must record the form: `alpha` and ' +
        '`structural` must differ. `behavior` must still be EQUAL, and that is what makes the ' +
        'claim isolating: the two bodies perform the same events in the same order, so the only ' +
        'thing left to explain a divergence at `alpha` is the callback form. Revert the ' +
        "serializer's `this`-awareness and `alpha` collapses; give the two bodies different " +
        'content, as the first version of that fixture did, and `behavior` diverges. Either way ' +
        'this fails.',
    },
  ],
  [
    'hook-literal',
    {
      summary: 'two same-kind hook bodies whose literal VALUES differ; nothing else does',
      kind: 'hook',
      equal: ['structural'],
      differ: ['alpha'],
      why:
        '`redundant-hook` (`analysis::emit_redundant_hook`) compares hook bodies by `alpha` ' +
        'alone -- it has no `structural`-level parameterization rule the way `duplicate-test` ' +
        'has a sibling for tests -- so a nested hook whose body differs only in a literal must ' +
        'still diverge at `alpha`; collapsing it to `structural` equality is exactly the claim ' +
        'these fixtures make: the two bodies are shaped identically but for that one literal, ' +
        'which is additional setup rather than repeated setup. `behavior` is deliberately not ' +
        "constrained: a literal folds into a hook's behavior events as a payload the same way " +
        "it does for a test's, so constraining it would add nothing this dimension needs. " +
        '`checkHookFingerprints` separately requires every participant to share `kind` -- ' +
        '`redundant-hook` never compares hooks of different kinds, so a pair that did would not ' +
        'be a candidate this rule could ever evaluate, and this dimension would be checking ' +
        'nothing.',
    },
  ],
  [
    'suite-path',
    {
      summary: 'the participants share a LEAF title but sit under different suites',
      kind: 'titles',
      why:
        '`duplicate-test-name` keys on `TestCase::full_title`, never on the bare leaf title, so ' +
        'the isolated variable is the suite prefix and it lives in the record\'s `title` rather ' +
        'than in any fingerprint. The check is therefore stated on titles directly: the leaf ' +
        'titles must be EQUAL (without that the pair is not a name-collision candidate at all and ' +
        'the fixture guards nothing) and the full titles must DIFFER (that is what keeps the rule ' +
        'silent). Renaming either test so the leaves stop matching is precisely the silent ' +
        'weakening this catches.',
    },
  ],
  [
    'suite-children',
    {
      summary: 'two same-titled suites whose child sequences are aligned but not copies',
      kind: 'suite',
      why:
        '`duplicate-suite` compares whole suites, so its negative cannot be stated as a relation ' +
        'between two tests; the participants are the `describe`s themselves, which get no ' +
        'fingerprint record (the stream emits executable declarations only). The claim is ' +
        'aggregated from the children instead: both suites must declare the SAME leaf titles in ' +
        'the SAME order under an identical context and modifier key -- without that they are not ' +
        'candidate copies and the silence is uninteresting -- while at least one aligned child ' +
        'pair must differ at `structural`, which is what denies the whole-suite claim. Make the ' +
        'two suites actual copies and every aligned pair matches, the requirement fails here, and ' +
        '`duplicate-suite` starts firing.',
    },
  ],
])

/**
 * Sentinel for an expectation this gate has no mechanism for YET. Debt: it should reach zero.
 *
 * Reserved for claims that ARE isolation claims -- "these two agree on everything but X" -- which
 * the current surface cannot state. Each one names the missing mechanism, so the reason doubles as
 * the work item.
 */
export const UNCHECKED = 'unchecked'

/**
 * Sentinel for an isolation claim whose EQUAL half names a view the fingerprint stream does not
 * emit and cannot emit without new analyzer semantics. Not debt, and not `NOT_ISOLABLE`.
 *
 * The distinction from `UNCHECKED` is buildability. `unchecked` says "build the mechanism and this
 * becomes a check"; `unstateable` says the mechanism requires the analyzer to SEE something it
 * currently has no view for at all, and the reason must name both the missing view and the event
 * that produces it. The worked example is the near-duplicate-context fixtures: their claim is
 * "these bodies are equivalent modulo phrasing, and only the context differs", but `alpha`,
 * `structural` and `behavior` are each phrasing-sensitive by design, the pairs deliberately differ
 * in all three, and the only field they share is `modifierKey` -- too thin an agreement to be a
 * check ("only equalities that two outright identical tests satisfy prove nothing", and so does an
 * equality every modifier-less pair satisfies). No relation over the current stream can require
 * these participants to agree on anything body-level. When near-duplicate normalization lands and
 * equates the phrasings, the claim restates as plain `context` -- that event, not a gate change, is
 * what closes an `unstateable` expectation, and `checkExpectation` enforces exactly that exit: the
 * prerequisites a checked relation runs (records present, comparable, non-trivial) and the claim's
 * premise (same modifiers, different contexts) fail as failures rather than hiding under the
 * opt-out, and the moment the full `context` relation holds the marker is declared stale -- so it
 * can neither outlive the view it waits on nor absorb a regression that breaks the claim's premise.
 *
 * The distinction from `NOT_ISOLABLE` is shape. Those claims are the wrong shape for a relation
 * between two records; these are the right shape with a missing premise. Collapsing them into one
 * bucket would hide exactly this difference: one bucket never changes with analyzer capabilities,
 * the other is expected to empty when a view lands.
 */
export const UNSTATEABLE = 'unstateable'

/**
 * Sentinel for an expectation that is NOT an isolation claim and never will be. Not debt.
 *
 * Three shapes qualify, and nothing else may use it:
 *
 *   1. **Policy silencing.** The participants are identical in every emitted field, and the reason
 *      no finding links them is a configuration or reporting decision (a whole-suite finding
 *      suppresses its children's) rather than anything about the code. Any assertion here passes
 *      no matter what the analyzer does. `cross-file-duplicate` shipping `Off` used to be the
 *      other example here, and its disappearance is the shape to watch for: a default flip turns
 *      a policy silence into a real finding, so an expectation resting on one has to be re-read
 *      rather than carried forward.
 *   2. **Indistinguishable by construction.** The fixture's own content is that the analyzer
 *      CANNOT tell the participants apart -- two unresolvable `.each` tables, two iterations of one
 *      loop -- which is what its `not-compared` verdict records. The guard is
 *      `check-duplicates-corpus.mjs`'s eligibility assertion on the solo run's `incomplete` flag,
 *      not a relation between two records.
 *   3. **Not a relation between the two participants.** A clustering property over three nodes, or
 *      a claim stated relative to a different fixture's pair.
 *
 * A fixture whose claim is a real "agrees on everything but X" belongs in UNCHECKED (mechanism
 * buildable at the current surface) or UNSTATEABLE (the equal half needs a view the analyzer does
 * not emit) even when the missing mechanism looks unlikely to be built. This bucket is for claims
 * that are the wrong SHAPE, not for claims that are merely inconvenient -- that distinction is the
 * only thing keeping it from becoming the dumping ground a suppression list usually becomes.
 */
export const NOT_ISOLABLE = 'not-isolable'

/**
 * Runs the analyzer over one fixture directory and indexes its `"test"` and `"hook"` records.
 *
 * @param {string} binary path to the vitest-dedup binary.
 * @param {string} dir fixture directory to analyze as a whole (participants may span files inside
 *   it, so it must be one run).
 * @returns {{byTitle: Map<string, object[]>, byFile: Map<string, object[]>,
 *   byHookLine: Map<string, object[]>, count: number}}
 *   `byTitle` is keyed `"<file>\n<full title>"` and holds every `"test"` record with that key in
 *   stream order, because rows of one `.each` table and iterations of one unrolled loop legitimately
 *   share a location AND a rendered title. `byFile` keeps each file's `"test"` records in order, for
 *   suite participants, which have no record of their own and are resolved through their children.
 *   `byHookLine` is keyed `"<file>\n<declared line>"` -> *every* `"hook"` record on that line, in
 *   stream order. A line is not a unique key: `beforeEach(...); afterEach(...)` written on one line
 *   is two declarations, and the extractor records each with its own `call.span`, so they differ in
 *   column, `kind` and body. Keeping only the last would let a participant naming the first be
 *   rejected for the second's `kind`, or -- worse, if both are the same kind -- compared against
 *   the wrong body. Resolution therefore narrows by `kind` and refuses an ambiguity rather than
 *   guessing (see `resolveParticipants`).
 */
export function collectFingerprintRecords(binary, dir) {
  const { records } = runAnalyzerJsonl(binary, [dir, '--emit-test-fingerprints'])
  const byTitle = new Map()
  const byFile = new Map()
  const byHookLine = new Map()
  let count = 0
  for (const record of records) {
    if (record.type === 'hook') {
      const file = basename(record.path)
      const hookKey = `${file}\n${record.line}`
      if (!byHookLine.has(hookKey)) byHookLine.set(hookKey, [])
      byHookLine.get(hookKey).push(record)
      continue
    }
    if (record.type !== 'test') continue
    count += 1
    const file = basename(record.path)
    const key = `${file}\n${record.title}`
    if (!byTitle.has(key)) byTitle.set(key, [])
    byTitle.get(key).push(record)
    if (!byFile.has(file)) byFile.set(file, [])
    byFile.get(file).push(record)
  }
  return { byTitle, byFile, byHookLine, count }
}

/**
 * Combines several declared dimensions into one relation.
 *
 * A fixture may declare more than one variable (`identifier,literal`: the pair differs in both a
 * helper name and a literal). The union of the DIFFER sets is required, and the intersection of the
 * EQUAL sets -- minus anything any dimension requires to differ, since a DIFFER requirement is the
 * one that names the analyzer regression being guarded and must win. That resolution is what lets
 * `identifier,literal` mean what it reads as: `literal` alone wants `structural` equal,
 * `identifier` wants it to differ, and a pair differing in both plainly differs there.
 *
 * Total on `names`: an empty list -- which `lib/fixture-directives.mjs` rejects at parse time, but
 * this function does not get to assume its caller enforced that -- yields `{ equal: [], differ:
 * [] }` rather than throwing. `equal` starts `null` only to distinguish "no dimension has run yet"
 * from "the intersection so far is empty"; it must never reach the `.delete` call below still
 * `null`, which is exactly what an empty `names` used to do.
 */
export function combineDimensions(names) {
  const differ = new Set()
  let equal = null
  for (const name of names) {
    const dimension = DIMENSIONS.get(name)
    for (const field of dimension.differ) differ.add(field)
    const own = new Set(dimension.equal)
    equal = equal === null ? own : new Set([...equal].filter((field) => own.has(field)))
  }
  if (equal === null) equal = new Set()
  for (const field of differ) equal.delete(field)
  return { equal: [...equal], differ: [...differ] }
}

/**
 * Checks one `rule=none` expectation's isolation claim.
 *
 * `isolation-violation` inverts the verdict rather than suppressing it: a marked expectation that
 * fails is reported as a listed violation, and a marked expectation that PASSES is a failure,
 * because the marker has gone stale and would otherwise sit there hiding a fixture that is now fine.
 *
 * @param {object} expectation a parsed `rule=none` expectation.
 * @param {object} index the result of `collectTestRecords` for the expectation's directory.
 * @param {(file: string) => string|null} readSource resolves a bare fixture filename to its text,
 *   used only to validate the line number of a participant that has no record of its own.
 * @returns {{status: 'ok'|'unchecked'|'not-isolable'|'violation'|'fail', message?: string}} `fail`
 *   covers both a broken declaration (a participant naming nothing the analyzer emitted) and an
 *   unmarked divergence; neither is ever downgraded to `violation`.
 */
export function checkExpectation(expectation, index, readSource) {
  const { isolates } = expectation

  // Resolve every participant first, even for an opted-out expectation. A rule=none participant
  // line is otherwise validated by nothing at all: `check-duplicates-corpus.mjs` only asserts that
  // no FINDING links the pair, which a line number pointing at blank space satisfies perfectly.
  // Two such stale line numbers were already sitting in this corpus when this gate was written,
  // and moving to the analyzer's own records found four more (a participant title that no test
  // ever renders is the same defect wearing a different hat).
  const resolved = resolveParticipants(expectation, index, readSource)
  if (resolved.error) return { status: 'fail', message: resolved.error }

  if (isolates.unchecked) return { status: 'unchecked', message: isolates.reason }
  if (isolates.unstateable) {
    // An opt-out may not bypass the prerequisites a checked relation runs first: the
    // participants must still be records the analyzer emits and actually compares. A vanished
    // record, an unresolved context or a trivial body is not "unstateable" -- it is a fixture
    // the analyzer no longer sees, and holding the opt-out open over it would launder exactly
    // the never-compared silence `comparable` exists to expose.
    const missing = requireRecords(resolved.participants)
    if (missing) return missing
    const notComparable = requireComparable(resolved.participants)
    if (notComparable) return notComparable
    const trivial = rejectTrivialBodies(
      resolved.participants,
      'which is excluded from comparison entirely',
    )
    if (trivial) return trivial

    // The premise the opt-out rests on, stated as one relation over the emitted fields the
    // claim says are NOT the isolated variable: same modifiers, different contexts. A fixture
    // drift that adds a second undeclared difference (`.skip`, an `.each` identity) or a
    // context-folding regression that collapses the two contextKeys must fail here rather than
    // hide under the opt-out -- either breaks "differ only in phrasing and context", and no
    // `context` restatement could ever hold over such a pair.
    const premiseHolds = applyRelation(
      resolved.participants,
      ['modifierKey'],
      ['contextKey'],
      'context',
      DIMENSIONS.get('context').why,
    )
    if (premiseHolds.status !== 'ok') return premiseHolds

    // The equal half, as the stale-marker ratchet in `isolation-violation`'s shape: every
    // `unstateable` reason records the same exit -- "restate as plain `context` when the missing
    // view lands" -- which makes staleness mechanical. Today the bodies differ at `alpha`,
    // `structural` and `behavior`, the relation cannot hold, and the opt-out stays open; the day
    // near-duplicate normalization equates the phrasings it can, and this gate -- not a future
    // reader of a printed note -- is what forces the restatement.
    const asContext = evaluate(
      {
        ...expectation,
        isolates: { dimensions: ['context'], unchecked: false, unstateable: false, notIsolable: false, reason: null },
      },
      resolved.participants,
    )
    if (asContext.status === 'ok') {
      return {
        status: 'fail',
        message:
          `is marked unstateable, but the \`context\` relation now holds for its participants -- ` +
          `the body-level view this reason names as missing has landed. Remove the marker and ` +
          `declare \`isolates context\` so the agreement half is checked from here on`,
      }
    }
    return { status: 'unstateable', message: isolates.reason }
  }
  if (isolates.notIsolable) return { status: 'not-isolable', message: isolates.reason }

  const result = evaluate(expectation, resolved.participants)
  if (expectation.isolationViolation) {
    if (result.status === 'ok') {
      return {
        status: 'fail',
        message:
          `is marked @vitest-dedup:isolation-violation ("${expectation.isolationViolation}") but now ` +
          `PASSES the isolation check. Remove the marker -- a stale exception hides the next ` +
          `fixture that really does stop discriminating.`,
      }
    }
    return { status: 'violation', message: `${expectation.isolationViolation} -- ${result.message}` }
  }
  return result
}

/**
 * Resolves each participant to the analyzer record (or, for a `describe`, the child records) it
 * names, failing loudly rather than skipping when it names nothing.
 *
 * Resolution is by (file, full title), then checked against the declared line -- not the reverse.
 * A title is what the analyzer actually renders, so a participant whose title no test bears is a
 * stale declaration even when its line number happens to point at code; a line alone cannot
 * identify one row of an `.each` table. Several participants of one expectation may share a title
 * (two rows of a table that render the same string, two iterations of one loop): those are matched
 * positionally against the stream, which `analysis::test_fingerprints` emits in the frontend's
 * deterministic unroll order.
 */
function resolveParticipants(expectation, index, readSource) {
  const seen = new Map()
  const participants = []
  for (const participant of expectation.participants) {
    const source = readSource(participant.file)
    if (source === null) {
      return { error: `cannot read participant file ${participant.file}` }
    }
    const key = `${participant.file}\n${participant.title}`
    const matches = index.byTitle.get(key) ?? []
    const ordinal = seen.get(key) ?? 0
    seen.set(key, ordinal + 1)

    if (matches.length > ordinal) {
      const record = matches[ordinal]
      if (record.line !== participant.line) {
        return {
          error:
            `${participant.file}:L${participant.line} "${participant.title}" is declared on line ` +
            `${participant.line} but the analyzer reports that test at line ${record.line}. Fix ` +
            `the participant line; a rule=none participant is read by nothing else, so a wrong ` +
            `one is never caught by the corpus gate.`,
        }
      }
      participants.push({ participant, record, children: null })
      continue
    }
    if (matches.length > 0) {
      return {
        error:
          `${participant.file} declares ${expectation.participants.length} participants titled ` +
          `"${participant.title}" but the analyzer reports only ${matches.length} test(s) with ` +
          `that title`,
      }
    }

    // No test bears this title. A `describe` is the legitimate case: suite-level expectations name
    // the suite, which gets no record, and is resolved through the children whose full titles it
    // prefixes.
    //
    // The title prefix alone does NOT identify the suite: the interesting suite-level negatives are
    // precisely the ones whose two `describe`s share a title, so every child of both matches both.
    // Children are attributed by position instead -- a child belongs to the last same-titled suite
    // declared above it -- which is resolved below, once every participant's line is known.
    const candidates = (index.byFile.get(participant.file) ?? []).filter((record) =>
      record.title.startsWith(`${participant.title} > `),
    )
    if (candidates.length > 0) {
      const lineCheck = assertDeclaredOn(source, participant, 'suite')
      if (lineCheck) return { error: lineCheck }
      participants.push({ participant, record: null, children: candidates })
      continue
    }

    // A hook: unlike a test, it has no title of its own and can never be one row of an unrolled
    // table, so it resolves directly by (file, declared line) rather than by title-then-ordinal
    // the way a test participant does above. That is a genuinely different, simpler resolution
    // path -- not the same one bent to fit -- because a hook's `Location` is already a unique key
    // on its own; nothing about it can collide with a sibling the way two `.each` rows collide on
    // one declaration site.
    const hooksOnLine = index.byHookLine.get(`${participant.file}\n${participant.line}`) ?? []
    if (hooksOnLine.length > 0) {
      const matching = hooksOnLine.filter((record) => record.kind === participant.title)
      if (matching.length === 0) {
        const found = [...new Set(hooksOnLine.map((record) => record.kind))].join(', ')
        return {
          error:
            `${participant.file}:L${participant.line} is declared as "${participant.title}" but ` +
            `the analyzer recorded ${found} there. A hook participant's title must name the ` +
            `hook's own kind (beforeEach/afterEach/beforeAll/afterAll)`,
        }
      }
      if (matching.length > 1) {
        // Two hooks of the SAME kind on one line cannot be told apart by anything a participant
        // directive carries, and picking either would make the check depend on emission order.
        // Refusing is the honest answer: the fixture can put them on separate lines.
        return {
          error:
            `${participant.file}:L${participant.line} has ${matching.length} ${participant.title} ` +
            `hooks on one line, so "${participant.title}" does not identify which one is meant. ` +
            `Put each hook on its own line`,
        }
      }
      participants.push({ participant, record: matching[0], children: null })
      continue
    }

    // Nothing at all: a stale title, or a hook the analyzer did not record at the declared line.
    // Both must be visible, so the participant is kept with neither a record nor children and its
    // line is validated against the source text. An expectation holding one of these can only be
    // opted out (`evaluate` refuses it), which is what keeps "the analyzer emits no record for it"
    // from quietly becoming "it checks out".
    const lineCheck = assertDeclaredOn(source, participant, 'declaration')
    if (lineCheck) return { error: lineCheck }
    participants.push({ participant, record: null, children: null })
  }

  // Attribute each suite participant's children now that every declared line is known: a child
  // belongs to the last suite boundary strictly above it, in its own file. Without this, two
  // `describe('summary')` blocks in one file each claim all four children, every aligned pair
  // compares a record with itself, and `suite-children` could never report a difference -- an
  // assertion that cannot fail, in the gate written to keep those out of the corpus.
  const boundaries = new Map()
  for (const entry of participants) {
    if (entry.children === null) continue
    const file = entry.participant.file
    if (!boundaries.has(file)) boundaries.set(file, [])
    boundaries.get(file).push(entry.participant.line)
  }
  for (const lines of boundaries.values()) lines.sort((a, b) => a - b)
  for (const entry of participants) {
    if (entry.children === null) continue
    const lines = boundaries.get(entry.participant.file)
    const next = lines.find((line) => line > entry.participant.line) ?? Infinity
    entry.children = entry.children
      .filter((child) => child.line > entry.participant.line && child.line < next)
      .sort((a, b) => a.line - b.line)
    if (entry.children.length === 0) {
      return {
        error:
          `${entry.participant.file}:L${entry.participant.line} "${entry.participant.title}" ` +
          `resolves to a suite with no children of its own between line ` +
          `${entry.participant.line} and ${next === Infinity ? 'end of file' : `line ${next}`}`,
      }
    }
  }
  return { participants }
}

/**
 * The fallback line check for a participant with no fingerprint record: the declared line must
 * exist and must mention the participant's own leaf title.
 *
 * Much weaker than matching the analyzer's reported location, and deliberately so -- it is a string
 * search, not a parse. Its whole job is that a hook or suite participant pointing at blank space
 * still fails, which is the stale-line-number defect that motivated resolving participants at all.
 */
function assertDeclaredOn(source, participant, what) {
  const lines = source.split('\n')
  const text = lines[participant.line - 1]
  if (text === undefined) {
    return `${participant.file}:L${participant.line} is past the end of the file (${lines.length} lines)`
  }
  const leaf = participant.title.split(' > ').pop()
  if (!text.includes(leaf)) {
    return (
      `${participant.file}:L${participant.line} "${participant.title}" has no fingerprint record ` +
      `and line ${participant.line} does not mention "${leaf}", so it names no ${what} the ` +
      `analyzer saw: \`${text.trim()}\``
    )
  }
  return null
}

/** Dispatches an expectation to the relation its declared dimensions describe. */
function evaluate(expectation, participants) {
  const names = expectation.isolates.dimensions
  const kinds = new Set(names.map((name) => DIMENSIONS.get(name).kind ?? 'fingerprints'))
  if (kinds.size > 1) {
    return {
      status: 'fail',
      message:
        `declares ${names.join(',')}, which mixes dimensions checked on fingerprints with ones ` +
        `checked on titles or on a suite's children. Those are different assertions and combining ` +
        `them would silently apply only one`,
    }
  }
  const kind = [...kinds][0]
  if (kind === 'titles') return checkTitles(expectation, participants)
  if (kind === 'suite') return checkSuite(expectation, participants)
  if (kind === 'hook') return checkHookFingerprints(expectation, participants)
  return checkFingerprints(expectation, participants)
}

/** `<file>:L<line> "<title>"`, the way every failure message names a participant. */
const describe = ({ participant }) =>
  `${participant.file}:L${participant.line} "${participant.title}"`

function requireRecords(participants) {
  const missing = participants.filter((entry) => entry.record === null)
  if (missing.length === 0) return null
  return {
    status: 'fail',
    message:
      `${missing.map(describe).join(' and ')} ${missing.length === 1 ? 'has' : 'have'} no ` +
      `--emit-test-fingerprints record: the stream emits one record per executable TEST ` +
      `declaration and one per analyzed HOOK declaration, so only a suite has neither (compare it ` +
      `through suite-children instead). Such an expectation must opt out with ` +
      `"${UNCHECKED} <reason>" naming the record kind that would close it`,
  }
}

/**
 * Guards against `not-compared` laundered into `distinct`: `comparable` is `true` only when the
 * record's own context resolved (`Context::is_comparable`), i.e. only when the analyzer actually
 * put it up against anything. A `rule=none` expectation's silence over a participant that was
 * NEVER eligible for comparison proves nothing about the variable it claims to isolate -- the
 * silence would be there regardless of what the fingerprint fields say, exactly the
 * `not-compared`-shaped hole `check-duplicates-corpus.mjs`'s own `incomplete` guard exists to
 * close for `verdict=distinct`, restated here per participant instead of per solo run.
 *
 * Missing the field entirely is a harder failure than `comparable: false`, and deliberately not
 * folded into it: a stale binary that predates the field would otherwise read `undefined` as
 * falsy and report every checked expectation as a comparability violation, burying the real
 * signal under noise from a binary that simply needs rebuilding. Defaulting a missing field to
 * `true` would be worse -- silently reopening exactly the hole this guard exists to close -- so
 * neither default is taken; both shapes fail loudly, with different messages.
 */
function requireComparable(participants) {
  for (const entry of participants) {
    if (entry.record === null) continue
    if (typeof entry.record.comparable !== 'boolean') {
      return {
        status: 'fail',
        message:
          `${describe(entry)}'s --emit-test-fingerprints record has no \`comparable\` field. This ` +
          `gate refuses to default it to \`true\` -- that would silently readmit the ` +
          `not-compared-laundered-into-distinct hole \`comparable\` exists to close. Rebuild the ` +
          `binary from current source; it must emit \`comparable\` on every "test" record`,
      }
    }
  }
  const notComparable = participants.filter((entry) => entry.record && !entry.record.comparable)
  if (notComparable.length > 0) {
    return {
      status: 'fail',
      message:
        `${notComparable.map(describe).join(' and ')} ${notComparable.length === 1 ? 'is' : 'are'} ` +
        `marked \`comparable: false\`: its context never resolved, so the analyzer never actually ` +
        `weighed it against anything. This expectation's silence is therefore \`not-compared\`, ` +
        `not \`distinct\` -- it proves nothing about the variable declared here. Either resolve ` +
        `whatever leaves the context unresolved, or restate the expectation with ` +
        `"${NOT_ISOLABLE} <reason>" the way not-compared fixtures already do, guarded by ` +
        `check-duplicates-corpus.mjs's eligibility assertion instead of this one`,
    }
  }
  return null
}

/**
 * Applies one combined dimension's relation: every field in `equal` matches across all
 * participants, every field in `differ` differs between every pair.
 *
 * Shared by `checkFingerprints` (`"test"` records) and `checkHookFingerprints` (`"hook"`
 * records): the relation itself does not know or care which record kind supplied the fields, only
 * that `equal`/`differ` name fields every participant's `record` actually carries -- which is
 * guaranteed by construction, since a dimension's `kind` (`'fingerprints'` default, or `'hook'`)
 * already routes an expectation to the matching checker before this runs.
 */
function applyRelation(participants, equal, differ, declared, why) {
  for (const field of equal) {
    for (let i = 1; i < participants.length; i += 1) {
      const left = participants[0]
      const right = participants[i]
      if (left.record[field] !== right.record[field]) {
        return {
          status: 'fail',
          message:
            `declares it isolates ${declared}, so ${describe(left)} and ${describe(right)} must ` +
            `agree on \`${field}\` -- they do not (${left.record[field]} vs ${right.record[field]}). ` +
            `The pair differs in more than the variable the fixture claims to isolate, so the ` +
            `guard it names is not actually under test.\n    ${why}`,
        }
      }
    }
  }

  for (const field of differ) {
    for (let i = 0; i < participants.length; i += 1) {
      for (let j = i + 1; j < participants.length; j += 1) {
        const left = participants[i]
        const right = participants[j]
        if (left.record[field] === right.record[field]) {
          return {
            status: 'fail',
            message:
              `declares it isolates ${declared}, so the analyzer must still tell ${describe(left)} ` +
              `from ${describe(right)} at \`${field}\` -- it does not (both ${left.record[field]}). ` +
              `The declared variable is invisible to the analyzer here, so this fixture's silence ` +
              `proves nothing about it.\n    ${why}`,
          }
        }
      }
    }
  }
  return { status: 'ok' }
}

/**
 * The main relation for `"test"` records: every field in `equal` matches across all participants,
 * every field in `differ` differs between every pair, and no participant's body is trivial.
 *
 * The triviality requirement applies to every checked expectation, not to any one dimension. A
 * trivial body (empty, or a recognized stub) is excluded from comparison outright, so a pair of
 * them is silent for a reason that has nothing to do with the declared variable -- the same
 * never-compared-at-all laundering `not-compared` exists to keep out of `distinct`.
 */
function checkFingerprints(expectation, participants) {
  const missing = requireRecords(participants)
  if (missing) return missing

  const notComparable = requireComparable(participants)
  if (notComparable) return notComparable

  const trivial = rejectTrivialBodies(participants, 'which is excluded from comparison entirely')
  if (trivial) return trivial

  return relateOverDimensions(expectation, participants)
}

/**
 * The trivial-body guard both fingerprint relations run before relating anything: a participant
 * with a trivial body is excluded from comparison entirely (by `duplicate-test` for tests, by
 * `redundant-hook` for hooks), so an expectation whose silence rests on that exclusion pins
 * nothing about the variable it declares. `exclusionClause` is the full "which ..." clause, so
 * each caller keeps its exact wording: "which is excluded ..." for tests, "which redundant-hook
 * excludes ..." for hooks.
 */
function rejectTrivialBodies(participants, exclusionClause) {
  const trivial = participants.filter((entry) => entry.record.trivial)
  if (trivial.length > 0) {
    return {
      status: 'fail',
      message:
        `${trivial.map(describe).join(' and ')} ${trivial.length === 1 ? 'has' : 'have'} a ` +
        `trivial body, ${exclusionClause}. The silence this ` +
        `fixture pins is explained by that exclusion and not by the variable it declares`,
    }
  }
  return null
}

/**
 * The equal/differ relation over an expectation's named dimensions -- shared verbatim by the
 * test-fingerprint and hook-fingerprint checks, which differ only in which records they admit.
 */
function relateOverDimensions(expectation, participants) {
  const names = expectation.isolates.dimensions
  const { equal, differ } = combineDimensions(names)
  const declared = names.join(', ')
  const why = names.map((name) => `${name}: ${DIMENSIONS.get(name).why}`).join('\n    ')
  return applyRelation(participants, equal, differ, declared, why)
}

/**
 * The relation for `kind: 'hook'` dimensions, over `"hook"` records.
 *
 * Deliberately not `checkFingerprints` with a flag: a `"hook"` record has neither `comparable`
 * nor `contextKey`/`modifierKey` (see `HookFingerprint`'s doc comment in `src/model.rs`), so there
 * is no comparability guard to run here -- a hook body's own fingerprint always succeeds, unlike a
 * test's ambient context, which is the whole reason `comparable` exists for tests and not for
 * hooks. What does carry over is the triviality guard (`redundant-hook` excludes a trivial hook
 * body from comparison exactly as `duplicate-test` excludes a trivial test body) and the same
 * equal/differ relation, via `applyRelation`.
 *
 * Also requires every participant to share `kind`: `redundant-hook` never compares a `beforeEach`
 * against an `afterEach`, so a hook-fingerprint claim spanning two kinds is not a pair that rule
 * could ever evaluate, and checking one would prove nothing about `redundant-hook`'s behavior.
 */
function checkHookFingerprints(expectation, participants) {
  const missing = requireRecords(participants)
  if (missing) return missing

  const kinds = new Set(participants.map((entry) => entry.record.kind))
  if (kinds.size > 1) {
    return {
      status: 'fail',
      message:
        `declares a hook-fingerprint dimension, but its participants are different hook kinds ` +
        `(${[...kinds].join(', ')}). \`redundant-hook\` only ever compares hooks of the same ` +
        `kind, so a pair of different kinds is not a candidate that rule would ever evaluate, and ` +
        `this expectation's silence proves nothing about it`,
    }
  }

  const trivial = rejectTrivialBodies(
    participants,
    'which redundant-hook excludes from comparison entirely',
  )
  if (trivial) return trivial

  return relateOverDimensions(expectation, participants)
}

/** `suite-path`: shared leaf title, differing full title. See the dimension's `why`. */
function checkTitles(expectation, participants) {
  const missing = requireRecords(participants)
  if (missing) return missing
  const leaf = (entry) => entry.record.title.split(' > ').pop()
  const why = DIMENSIONS.get('suite-path').why
  for (let i = 1; i < participants.length; i += 1) {
    const left = participants[0]
    const right = participants[i]
    if (leaf(left) !== leaf(right)) {
      return {
        status: 'fail',
        message:
          `declares it isolates suite-path, so ${describe(left)} and ${describe(right)} must share ` +
          `a LEAF title -- they do not ("${leaf(left)}" vs "${leaf(right)}"). Nothing about this ` +
          `pair is a name-collision candidate, so the rule's silence is not attributable to the ` +
          `suite prefix.\n    ${why}`,
      }
    }
    if (left.record.title === right.record.title) {
      return {
        status: 'fail',
        message:
          `declares it isolates suite-path, so ${describe(left)} and ${describe(right)} must have ` +
          `DIFFERENT full titles -- both render "${left.record.title}".\n    ${why}`,
      }
    }
  }
  return { status: 'ok' }
}

/** `suite-children`: aligned child sequences, at least one structurally differing pair. */
function checkSuite(expectation, participants) {
  const notSuites = participants.filter((entry) => entry.children === null)
  if (notSuites.length > 0) {
    return {
      status: 'fail',
      message:
        `declares it isolates suite-children, but ${notSuites.map(describe).join(' and ')} ` +
        `resolve(s) to a test rather than to a suite with children`,
    }
  }
  const why = DIMENSIONS.get('suite-children').why
  const leaves = (entry) => entry.children.map((child) => child.title.split(' > ').pop())
  for (let i = 1; i < participants.length; i += 1) {
    const left = participants[0]
    const right = participants[i]
    const leftLeaves = leaves(left)
    const rightLeaves = leaves(right)
    if (leftLeaves.join(' | ') !== rightLeaves.join(' | ')) {
      return {
        status: 'fail',
        message:
          `declares it isolates suite-children, so ${describe(left)} and ${describe(right)} must ` +
          `declare the same child titles in the same order -- they do not ` +
          `([${leftLeaves.join(', ')}] vs [${rightLeaves.join(', ')}]). Two suites that do not ` +
          `even line up are not candidate copies, so duplicate-suite's silence is uninteresting.` +
          `\n    ${why}`,
      }
    }
    let structurallyDifferent = 0
    for (let k = 0; k < left.children.length; k += 1) {
      const a = left.children[k]
      const b = right.children[k]
      if (typeof a.comparable !== 'boolean' || typeof b.comparable !== 'boolean') {
        return {
          status: 'fail',
          message:
            `the aligned child "${leftLeaves[k]}" has no \`comparable\` field on its ` +
            `--emit-test-fingerprints record. This gate refuses to default it to \`true\` -- ` +
            `rebuild the binary from current source; it must emit \`comparable\` on every "test" ` +
            `record`,
        }
      }
      if (!a.comparable || !b.comparable) {
        return {
          status: 'fail',
          message:
            `declares it isolates suite-children, but the aligned child "${leftLeaves[k]}" is ` +
            `marked \`comparable: false\` on at least one side: its context never resolved, so it ` +
            `was never actually weighed against its counterpart. This suite pair's silence is ` +
            `\`not-compared\`, not a proven non-copy, and proves nothing about duplicate-suite here` +
            `\n    ${why}`,
        }
      }
      if (a.trivial || b.trivial) {
        return {
          status: 'fail',
          message:
            `declares it isolates suite-children, but the aligned child "${leftLeaves[k]}" has a ` +
            `trivial body on one side and is excluded from comparison entirely\n    ${why}`,
        }
      }
      for (const field of ['contextKey', 'modifierKey']) {
        if (a[field] !== b[field]) {
          return {
            status: 'fail',
            message:
              `declares it isolates suite-children, so the aligned children "${leftLeaves[k]}" ` +
              `must agree on \`${field}\` -- they do not (${a[field]} vs ${b[field]}). The suites ` +
              `differ in ambient context as well, so their structural difference is not the only ` +
              `thing keeping duplicate-suite silent.\n    ${why}`,
          }
        }
      }
      if (a.structural !== b.structural) structurallyDifferent += 1
    }
    if (structurallyDifferent === 0) {
      return {
        status: 'fail',
        message:
          `declares it isolates suite-children, so at least one aligned child pair of ` +
          `${describe(left)} and ${describe(right)} must differ at \`structural\` -- none does. ` +
          `These suites ARE copies, and duplicate-suite should be firing on them.\n    ${why}`,
      }
    }
  }
  return { status: 'ok' }
}
