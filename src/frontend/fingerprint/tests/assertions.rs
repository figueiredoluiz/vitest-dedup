use super::*;

// Assertion recognition edges
// -----------------------------------------------------------------------------------------

#[test]
fn an_absurd_modifier_chain_and_an_unmodeled_base_are_not_assertions() {
    // `MAX_ASSERTION_CHAIN` bounds the unwrap loop: past it the call is treated as an
    // ordinary call rather than walked forever.
    let chain = "not.".repeat(MAX_ASSERTION_CHAIN + 2);
    let events = behavior_of(&format!("expect(1).{chain}toBe(2);"));
    assert!(
        !events.iter().any(|event| event.starts_with("assert:")),
        "a modifier chain longer than {MAX_ASSERTION_CHAIN} is not an assertion: {events:?}"
    );
    // ...and one link inside the bound still is.
    let chain = "not.".repeat(MAX_ASSERTION_CHAIN);
    let events = behavior_of(&format!("expect(1).{chain}toBe(2);"));
    assert!(
        events.iter().any(|event| event.starts_with("assert:")),
        "a chain of exactly {MAX_ASSERTION_CHAIN} modifiers is still an assertion: {events:?}"
    );

    #[rustfmt::skip]
    let not_assertions: &[(&str, &str)] = &[
        ("base-callee-is-a-call", "f()(1).toBe(2);"),
        ("soft-on-something-else", "o.expect.soft(1).toBe(2);"),
        ("modifier-on-a-non-member", "(1).toBe(2);"),
        ("matcher-on-a-bare-identifier", "o.toBe(2);"),
    ];
    for (name, body) in not_assertions {
        let events = behavior_of(body);
        assert!(
            !events.iter().any(|event| event.starts_with("assert:")),
            "{name}: `{body}` is not an `expect` assertion: {events:?}"
        );
    }
}

/// Every `assert:chain:` event a body emits, in order.
fn chain_events(body: &str) -> Vec<String> {
    behavior_of(body)
        .into_iter()
        .filter(|event| event.starts_with("assert:chain:"))
        .collect()
}

/// The defect `property_chain_event` exists for: a chai-style property assertion carries no
/// call node, so before it the chain contributed *nothing* and
/// `expect(spy).to.not.have.been.calledOnce` compared equal to
/// `expect(spy).to.have.been.calledTwice` -- opposite polarity and a different assertion.
/// Distilled from `vitest`'s own `test/unit/test/chai-style-assertions.test.ts` (:111/:419);
/// the extractor-level twin is `extract.rs`'s
/// `behavior_trace_records_a_property_chain_assertion_and_its_polarity`.
#[test]
fn a_property_chain_assertion_reaches_behavior_with_its_polarity() {
    let setup = "const spy = vi.fn(); spy(); spy();";
    let negated = behavior_of(&format!("{setup} expect(spy).to.not.have.been.calledOnce;"));
    let plain = behavior_of(&format!("{setup} expect(spy).to.have.been.calledTwice;"));
    assert_ne!(
        negated, plain,
        "opposite-polarity property chains over identical setup must not compare equal"
    );
    assert!(
        negated.contains(&"assert:chain:expect.to.not.have.been.calledOnce".to_owned()),
        "the chain must reach the trace verbatim, `not` included: {negated:?}"
    );
    // Polarity alone, with the rest of the chain held fixed, is enough to separate them --
    // the narrowest form of the same claim.
    assert_ne!(
        chain_events("expect(spy).to.have.been.calledOnce;"),
        chain_events("expect(spy).to.not.have.been.calledOnce;"),
        "`not` must be load-bearing on its own"
    );
    // Additive, not atomic: the `expect(...)` call event that carries the subject in
    // `Mode::Alpha` still follows, which is what keeps two chains differing only in a
    // subject literal apart.
    assert_ne!(
        behavior_of("expect(build(1)).to.be.ok;"),
        behavior_of("expect(build(2)).to.be.ok;"),
        "the subject must still discriminate under a property chain"
    );
}

/// One assertion, one event. The chain is walked by skipping straight to the base
/// `expect(...)` call; descending link by link instead would re-enter `walk_member_value` on
/// every prefix and emit `expect.to.not.have.been`, `expect.to.not.have`, ... as well.
#[test]
fn a_property_chain_emits_exactly_one_event_however_long_it_is() {
    assert_eq!(
        chain_events("expect(spy).to.not.have.been.calledOnce;"),
        vec!["assert:chain:expect.to.not.have.been.calledOnce".to_owned()],
    );
    // `expect.soft`/`expect.poll` are the base spellings `expect_base` also accepts, and the
    // qualifier has to say which one was written.
    assert_eq!(
        chain_events("expect.soft(x).to.be.ok;"),
        vec!["assert:chain:expect.soft.to.be.ok".to_owned()],
    );
    assert_eq!(
        chain_events("expect.poll(x).to.be.ok;"),
        vec!["assert:chain:expect.poll.to.be.ok".to_owned()],
    );
}

/// A chai *method* assertion (`expect(x).to.have.property('a')`) is a call, so its member
/// names already reach the trace through the ordinary `call:` path's own dotted callee path.
/// A chain event for its `expect(x).to.have` prefix would count one assertion twice, which is
/// why callees are walked by `walk_under_members` rather than `walk_member_value`. The second half
/// pins that skipping the links costs nothing: everything inside the callee is still walked.
#[test]
fn a_chain_in_callee_position_is_not_a_property_chain_assertion() {
    let events = behavior_of("expect(x).to.have.property('a');");
    assert!(
        chain_events("expect(x).to.have.property('a');").is_empty(),
        "a called chai assertion must not also emit a chain event: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.starts_with("call:global:expect.to.have.property")),
        "the call path already names the chain: {events:?}"
    );
    // The callee descent still reaches the calls nested inside it.
    let fluent = behavior_of("page.locator(s).click();");
    for expected in ["call:global:page.locator.click", "call:global:page.locator"] {
        assert!(
            fluent.iter().any(|event| event.starts_with(expected)),
            "`{expected}` must survive walking the callee: {fluent:?}"
        );
    }
    let computed = behavior_of("registry[key()].run();");
    assert!(
        computed
            .iter()
            .any(|event| event.starts_with("call:global:key")),
        "a computed callee segment's own call must still be walked: {computed:?}"
    );
}

/// The negative side. Each of these is one edit away from the recognized shape, so a
/// recognizer that stopped checking its premise would fail here rather than quietly widen.
#[test]
fn a_member_chain_that_is_not_rooted_in_expect_emits_no_chain_event() {
    #[rustfmt::skip]
    let declined: &[(&str, &str)] = &[
        ("root-is-not-a-call", "o.to.be.ok;"),
        ("root-is-a-local-named-expect", "const expect = mk(); expect(x).to.be.ok;"),
        ("root-is-another-call", "assertThat(x).to.be.ok;"),
        ("root-is-an-unmodeled-expect-member", "expect.unknown(x).to.be.ok;"),
        ("computed-link", "expect(x).to['be'].ok;"),
    ];
    for (name, body) in declined {
        let events = chain_events(body);
        assert!(
            events.is_empty(),
            "{name}: `{body}` is not a chai-style property assertion: {events:?}"
        );
    }
    // The control: the same bodies with the one edit undone are recognized.
    assert_eq!(chain_events("expect(x).to.be.ok;").len(), 1);
}

/// `MAX_ASSERTION_CHAIN` bounds the property-chain unwrap loop exactly as it bounds
/// `assertion_event`'s. Probed one link either side of the edge, not comfortably inside it.
#[test]
fn a_property_chain_longer_than_the_bound_is_not_an_assertion() {
    let at_bound = "a.".repeat(MAX_ASSERTION_CHAIN - 1);
    let events = chain_events(&format!("expect(x).{at_bound}last;"));
    assert_eq!(
        events.len(),
        1,
        "a chain of exactly {MAX_ASSERTION_CHAIN} links is still an assertion: {events:?}"
    );
    let past_bound = "a.".repeat(MAX_ASSERTION_CHAIN);
    let events = chain_events(&format!("expect(x).{past_bound}last;"));
    assert!(
        events.is_empty(),
        "a chain of {} links is past the bound: {events:?}",
        MAX_ASSERTION_CHAIN + 1
    );
}

// -----------------------------------------------------------------------------------------
