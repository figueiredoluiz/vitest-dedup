use super::*;

// Explicit type arguments
// ---------------------------------------------------------------------------------------

/// The Error-severity false positive this modeling exists for: two type-level assertions whose
/// only difference is the type they assert about. Erasing the type arguments made these one
/// `duplicate-test` at Error severity on correct code -- the same "a value's kind or content
/// reduced to nothing" class as the literal-kind bug above, and the exact shape that made a
/// type-level suite unusable.
#[test]
fn type_arguments_distinguish_otherwise_identical_calls() {
    let prints = fingerprint_all(
        "import { assertEqual } from './helpers';\n\
             type Expected = Record<string, unknown>;\n\
             test('string mapping', () => { assertEqual<Record<string, string>, Expected>(true); });\n\
             test('number mapping', () => { assertEqual<Record<string, number>, Expected>(true); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "two type-level assertions differing only in their type arguments must not share an \
             `alpha`: that is a `duplicate-test` at Error severity on correct code"
    );
    assert_ne!(
        prints[0].structural, prints[1].structural,
        "a type argument cannot be lifted into a `test.each` row, so `structural` must keep \
             it too rather than proposing an impossible parameterization"
    );
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "`near-duplicate-test`/`subsumed-test` key on behavior events, not `alpha` -- the \
             type arguments have to reach the events as well"
    );
}

/// The other direction of the same fix: modeling type arguments must not be a licence to make
/// everything distinct. Two genuinely identical calls, type arguments included, still match.
#[test]
fn identical_type_arguments_still_compare_equal() {
    let prints = fingerprint_all(
        "import { assertEqual } from './helpers';\n\
             type Expected = Record<string, unknown>;\n\
             test('first', () => { assertEqual<Record<string, string>, Expected>(true); });\n\
             test('second', () => { assertEqual<Record<string, string>, Expected>(true); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_eq!(
        prints[0].alpha, prints[1].alpha,
        "identical type-level assertions must still be recognized as duplicates"
    );
    assert_eq!(prints[0].behavior, prints[1].behavior);
}

/// Type arguments are compared as source text, so formatting must be normalized away first:
/// reporting two tests as different because a formatter inserted a space would be a defect of
/// its own. Line breaks, indentation, and spacing around `<`, `,` and `>` all normalize out,
/// while a space that separates two words (`keyof T`) is preserved.
#[test]
fn whitespace_only_differences_in_type_arguments_still_compare_equal() {
    let prints = fingerprint_all(
        "import { assertEqual } from './helpers';\n\
             type Expected = { a: 1 };\n\
             test('tight', () => { assertEqual<Record<string,keyof Expected>>(true); });\n\
             test('loose', () => { assertEqual< Record< string , keyof Expected >\n>(true); });",
    );
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_eq!(
        prints[0].alpha, prints[1].alpha,
        "type arguments differing only in whitespace name the same type and must fingerprint \
             identically"
    );
}

/// The normalization must not weld two words together: dropping *every* space would make
/// `keyof T` collide with an identifier spelled `keyofT`, trading a false positive for a
/// quieter one.
#[test]
fn type_argument_normalization_keeps_word_separators() {
    assert_eq!(
        normalize_type_argument_text("< Record< string , number > >"),
        "<Record<string,number>>"
    );
    assert_eq!(normalize_type_argument_text("<keyof  T>"), "<keyof T>");
    assert_ne!(
        normalize_type_argument_text("<keyof T>"),
        normalize_type_argument_text("<keyofT>")
    );
    assert_eq!(
        normalize_type_argument_text("<'a  b'>"),
        "<'a  b'>",
        "whitespace inside a string literal type is content, not formatting"
    );
    assert_ne!(
        normalize_type_argument_text("<'a b'>"),
        normalize_type_argument_text("<'ab'>")
    );
}

/// `CallExpression` is not the only construct that carries type arguments, and missing a
/// sibling is this repository's most repeated defect. `new Foo<T>()`, a tagged template's
/// `` tag<T>`...` ``, and a bare `TSInstantiationExpression` (`foo<T>` with no call) all get
/// the same treatment.
#[test]
fn type_arguments_are_modeled_on_every_construct_that_carries_them() {
    let new_expression = fingerprint_all(
        "import { Box } from './box';\n\
             test('string box', () => { const box = new Box<string>(); return box; });\n\
             test('number box', () => { const box = new Box<number>(); return box; });",
    );
    assert_eq!(new_expression.len(), 2);
    assert_ne!(
        new_expression[0].alpha, new_expression[1].alpha,
        "`new Box<string>()` and `new Box<number>()` must not share an `alpha`"
    );

    let tagged = fingerprint_all(
        "import { sql } from './sql';\n\
             test('string rows', () => { const rows = sql<string>`select 1`; return rows; });\n\
             test('number rows', () => { const rows = sql<number>`select 1`; return rows; });",
    );
    assert_eq!(tagged.len(), 2);
    assert_ne!(
        tagged[0].alpha, tagged[1].alpha,
        "a tagged template's type arguments must reach the fingerprint"
    );

    let instantiation = fingerprint_all(
        "import { identity } from './identity';\n\
             test('string identity', () => { const pick = identity<string>; return pick; });\n\
             test('number identity', () => { const pick = identity<number>; return pick; });",
    );
    assert_eq!(instantiation.len(), 2);
    assert!(
        instantiation[0].alpha.contains("(instantiate "),
        "expected a TSInstantiationExpression, not some other parse: {}",
        instantiation[0].alpha
    );
    assert_ne!(
        instantiation[0].alpha, instantiation[1].alpha,
        "`identity<string>` and `identity<number>` are different expressions"
    );
}

/// A fluent type-level chain (`expectTypeOf<A>().toEqualTypeOf<B>()`) puts type arguments on
/// both halves. Behavior events, which `near-duplicate-test` and `subsumed-test` key on, must
/// distinguish a difference in either one.
#[test]
fn type_arguments_reach_behavior_events_on_both_halves_of_a_fluent_chain() {
    let inner = fingerprint_all(
        "import { expectTypeOf } from 'vitest';\n\
             type Expected = string;\n\
             test('a', () => { expectTypeOf<string>().toEqualTypeOf<Expected>(); });\n\
             test('b', () => { expectTypeOf<number>().toEqualTypeOf<Expected>(); });",
    );
    assert_ne!(
        inner[0].behavior, inner[1].behavior,
        "a difference in the subject's type argument must show up in the events: {:?} vs {:?}",
        inner[0].behavior, inner[1].behavior
    );

    let outer = fingerprint_all(
        "import { expectTypeOf } from 'vitest';\n\
             type Subject = string;\n\
             test('a', () => { expectTypeOf<Subject>().toEqualTypeOf<string>(); });\n\
             test('b', () => { expectTypeOf<Subject>().toEqualTypeOf<number>(); });",
    );
    assert_ne!(
        outer[0].behavior, outer[1].behavior,
        "a difference in the matcher's type argument must show up in the events: {:?} vs {:?}",
        outer[0].behavior, outer[1].behavior
    );
}

/// The `expect(...)` assertion path builds its own event string instead of going through
/// `walk_call`, so it needs the same treatment or type-level matchers hung off `expect` stay
/// indistinguishable.
#[test]
fn type_arguments_reach_the_assertion_event() {
    let prints = fingerprint_all(
        "import { expect } from 'vitest';\n\
             import { value } from './value';\n\
             test('a', () => { expect(value).toEqual<string>('x'); });\n\
             test('b', () => { expect(value).toEqual<number>('x'); });",
    );
    assert_eq!(prints.len(), 2);
    assert!(
        prints[0]
            .behavior
            .iter()
            .any(|event| event.starts_with("assert:")),
        "expected the assertion path, not an ordinary call event: {:?}",
        prints[0].behavior
    );
    assert_ne!(
        prints[0].behavior, prints[1].behavior,
        "a matcher's own type arguments must reach the assertion event: {:?} vs {:?}",
        prints[0].behavior, prints[1].behavior
    );
}

// -----------------------------------------------------------------------------------------

// Type-argument text normalization
// -----------------------------------------------------------------------------------------

/// `normalize_type_argument_text` canonicalizes a type-argument list's raw source so that
/// formatting alone never changes a fingerprint -- while keeping anything inside a string
/// literal type byte-exact, and keeping the one space that separates two words.
#[test]
fn type_argument_text_normalizes_formatting_but_not_meaning() {
    let behavior_for =
        |type_arguments: &str| behavior_of(&format!("expect(v).toEqual<{type_arguments}>(1);"));

    // Formatting only: equal. The whitespace after a quoted string type is the part that
    // fails if the quote-tracking loop mishandles its escape or its closing delimiter.
    assert_eq!(
        behavior_for("{ a: 'p q', b: 1 }"),
        behavior_for("{a:'p q',b:1}"),
        "whitespace outside a string literal type must normalize away"
    );

    // Inside quotes: preserved, so two different literal types stay different.
    assert_ne!(
        behavior_for("'p q'"),
        behavior_for("'pq'"),
        "whitespace *inside* a string literal type is part of the type"
    );

    // A word boundary carries meaning and the single separating space survives.
    assert_ne!(
        behavior_for("keyof T"),
        behavior_for("keyofT"),
        "`keyof T` and `keyofT` are different types; the separating space must survive"
    );
    assert_eq!(
        behavior_for("keyof   T"),
        behavior_for("keyof T"),
        "...collapsed to exactly one space, however it was written"
    );

    // A non-word boundary needs no space at all.
    assert_eq!(
        behavior_for("A | B"),
        behavior_for("A|B"),
        "no space is needed either side of a punctuation boundary"
    );
}

/// `arguments` is the other construct an arrow does not bind, and only the *free* form -- the
/// real `arguments` object -- carries that meaning: a resolved symbol by that name is
/// somebody's own binding.
#[test]
fn only_a_free_arguments_reference_marks_a_body_as_this_dependent() {
    // A `function` body absorbs its own `arguments`, so the form it prints changes.
    let free = alpha_of("const h = function(){ g(arguments); };");
    assert!(
        free.contains("(function-fn () "),
        "a free `arguments` inside a `function` must print the specific form: {free}"
    );

    // A local of that name is not the `arguments` object, and an ordinary free identifier is
    // not either -- both must leave the interchangeable `fn` form in place.
    for (name, body) in [
        ("shadowed", "const h = function(args){ g(args); };"),
        (
            "other-free-identifier",
            "const h = function(){ g(other); };",
        ),
    ] {
        let alpha = alpha_of(body);
        assert!(
            !alpha.contains("function-fn"),
            "{name}: `{body}` observes no `this`/`arguments`, so the two callable forms stay \
                 interchangeable: {alpha}"
        );
    }
}

// -----------------------------------------------------------------------------------------
