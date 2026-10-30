// Non-transitive similarity, the hazard for any clustering rule: A resembles B, B resembles
// C, but A and C have incompatible assertions. If a rule linked findings by "shares an edge with a
// similar member" and then unioned those edges into a cluster, it would wrongly propose deleting A
// in favor of C (or vice versa) -- asserting an equivalence between two tests that assert opposite
// things.
//
// A: reads `status` immediately after creating an order (before any mutation).
// B: reads `status` after loading an order via an extracted local (vs A's direct member access) --
//    "near" A only in the narrative sense of an extracted-local transformation.
// C: reads `status` after loading an order, but AFTER cancelling it -- "near" B only in the
//    narrative sense of a reordered-setup transformation shape, and only if the
//    intervening `cancelOrder()` call is ignored, which it must not be: it changes the state, so A
//    and C assert opposite statuses. B is the narrative pivot; A and C do not resemble each other
//    even loosely.
//
// Decision, reached deliberately rather than by default (see src/analysis.rs's `NearDuplicateKey`
// and the module-level `NearDuplicateTest` doc, and src/pairs.rs's module doc): this rule
// groups by EXACT equality of a canonical key (path, equivalent context, equivalent modifiers, and
// the sorted multiset of `body.behavior` events) -- never a fuzzy "resembles" score. `group_and_order`
// partitions tests with a `BTreeMap` keyed on that struct, so two tests land in the same group iff
// their keys are literally equal, and struct equality is transitive by construction (derived
// `Eq`/`Ord` on `String`/`Vec<String>` fields). There is no "shares an edge with the cluster" step
// to verify after the fact -- a cluster's pairwise relation is not assumed, it is the *definition*
// of how the group was built. This is why the alternative design ("clusters are
// only emitted when the relation genuinely holds pairwise across all members") is not a separate
// implementation task here: it is what exact-key grouping already guarantees for every rule in this
// module, `near-duplicate-test` included.
//
// What that means concretely for A/B/C: `near-duplicate-test` currently bridges exactly one
// transformation, reordered-independent-setup (see the rule doc), so A and B's differing callee
// (`createOrder` vs `loadOrder`) alone makes their behavior multisets unequal -- they are never
// placed in the same group, regardless of how similar the narrative reads. B and C differ the same
// way (`cancelOrder` is an extra event C carries and B does not) AND in assertion polarity
// (`toBe('pending')` vs `not.toBe('pending')`), one of this rule's non-negotiable guards: a
// polarity flip is the opposite claim. So no pair here is linked, let alone clustered, verified by
// running the real binary over this file alone (testsAnalyzed=3, zero findings, incomplete=false):
// all three pairs are independently `distinct`. The non-transitive triple this fixture set out to
// stress cannot arise under exact-key grouping precisely because "near" never degrades into "shares
// some events with something that shares some events with something else": every relationship this
// rule reports is a full key match, not a chain of partial ones.
//
// @vitest-dedup:extracted 3
// @vitest-dedup:expect rule=none verdict=distinct severity=none reason=different-callee
// @vitest-dedup:isolates not-isolable this expectation asserts a clustering property (non-transitivity) over a three-node set, not the isolation of one variable between two participants; the three bodies are deliberately pairwise different in more than one way
// @vitest-dedup:participant L58 "a new order is pending"
// @vitest-dedup:participant L63 "a loaded order is pending"
// @vitest-dedup:expect rule=none verdict=distinct severity=none reason=callee-and-polarity
// @vitest-dedup:isolates not-isolable this expectation asserts a clustering property (non-transitivity) over a three-node set, not the isolation of one variable between two participants; the three bodies are deliberately pairwise different in more than one way
// @vitest-dedup:participant L63 "a loaded order is pending"
// @vitest-dedup:participant L69 "a cancelled order is not pending"
// @vitest-dedup:expect rule=none verdict=distinct severity=none reason=never-linked-transitively
// @vitest-dedup:isolates not-isolable this expectation asserts a clustering property (non-transitivity) over a three-node set, not the isolation of one variable between two participants; the three bodies are deliberately pairwise different in more than one way
// @vitest-dedup:participant L58 "a new order is pending"
// @vitest-dedup:participant L69 "a cancelled order is not pending"
import { test, expect } from 'vitest'
import { createOrder, loadOrder, cancelOrder } from './orders'

test('a new order is pending', () => {
  const order = createOrder()
  expect(order.status).toBe('pending')
})

test('a loaded order is pending', () => {
  const order = loadOrder(1)
  const status = order.status
  expect(status).toBe('pending')
})

test('a cancelled order is not pending', () => {
  cancelOrder(1)
  const order = loadOrder(1)
  const status = order.status
  expect(status).not.toBe('pending')
})
