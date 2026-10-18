use super::*;

// JSX: structured serialization
// -----------------------------------------------------------------------------------------
//
// The recall corpus pins the two end-to-end losses (a `duplicate-test` and a
// `parameterization-candidate`); these pin the serializer directly, including the shapes the
// corpus has no case for (fragments, spread attributes, boolean attributes, expression
// children, intrinsics vs components) and the text normalization the formatting case rests on.

/// Two small arms with no coverage, both reachable from ordinary test bodies.
///
/// A declaration with no initializer (`let captured;`) is the shape every "assign it in a hook,
/// read it in the test" file opens with, and the dataflow walk skips it -- there is nothing to
/// inline. A `..` segment in a dynamic-import specifier is how half of a monorepo spells its
/// imports, and it is normalized away so two spellings of one file agree.
#[test]
fn an_uninitialized_declaration_and_a_parent_path_segment_are_handled() {
    // No initializer: skipped, and its presence does not change what the rest of the body says.
    assert_eq!(
        alpha_of("let captured; captured = compute(); expect(captured).toBe(1);"),
        alpha_of("let captured; captured = compute(); expect(captured).toBe(1);"),
    );
    assert_ne!(
        alpha_of("let captured; captured = compute(); expect(captured).toBe(1);"),
        alpha_of("let captured; captured = other(); expect(captured).toBe(1);"),
        "the declaration is skipped, not the assignment that follows it"
    );

    // `..` is resolved away, so these two specifiers name one module and compare equal.
    assert_eq!(
        alpha_of("const m = await import('../helpers'); expect(m).toBeDefined();"),
        alpha_of("const m = await import('./sub/../../helpers'); expect(m).toBeDefined();"),
        "two spellings of one path must not read as two different modules"
    );
    assert_ne!(
        alpha_of("const m = await import('../helpers'); expect(m).toBeDefined();"),
        alpha_of("const m = await import('../other'); expect(m).toBeDefined();"),
        "and genuinely different modules still differ"
    );
}

/// The attribute forms beyond `name="string"`, each of which had no coverage.
///
/// `ser_jsx_attribute_value` handles four value shapes and the name handles two; only a quoted
/// string and a braced expression were exercised. The rest are ordinary in real markup --
/// `xlink:href` on an `<svg>`, an element handed straight to a prop -- and each is a place the
/// serializer could silently emit nothing, which would make two different elements compare equal.
#[test]
fn jsx_attribute_forms_are_each_serialized() {
    // A namespaced attribute name, which SVG markup uses constantly.
    assert_ne!(
        alpha_of_tsx(r#"const el = render(<svg xlink:href="a" />);"#),
        alpha_of_tsx(r#"const el = render(<svg xlink:href="b" />);"#),
        "a namespaced attribute's value still distinguishes two elements"
    );
    assert_ne!(
        alpha_of_tsx(r#"const el = render(<svg xlink:href="a" />);"#),
        alpha_of_tsx(r#"const el = render(<svg href="a" />);"#),
        "and the namespace is part of the name"
    );
    // An element, and a fragment, handed to an attribute directly rather than through braces.
    assert_ne!(
        alpha_of_tsx("const el = render(<Panel header=<Title /> />);"),
        alpha_of_tsx("const el = render(<Panel header=<Caption /> />);"),
        "an element-valued attribute is serialized, not dropped"
    );
    assert_ne!(
        alpha_of_tsx("const el = render(<Panel header=<Title /> />);"),
        alpha_of_tsx("const el = render(<Panel header=<></> />);"),
        "a fragment-valued attribute is distinguishable from an element-valued one"
    );
    // An empty container is not observable, so it reads as the valueless form.
    assert_eq!(
        alpha_of_tsx("const el = render(<Panel header={} />);"),
        alpha_of_tsx("const el = render(<Panel header={} />);"),
        "`attr='{{}}'` is stable, which is the only claim to make about a value a renderer \
             cannot see"
    );
}

/// The element-name forms JSX's grammar allows beyond a bare identifier.
///
/// `ser_jsx_name` and `ser_jsx_member_object` handle four shapes apiece and only the plain
/// identifier had a test: the member, nested-member and `this` arms were reachable from any
/// component library that namespaces its parts (`<Menu.Item />` is the common spelling) and had
/// no coverage at all, which is most of what held `fingerprint.rs` under its floor.
///
/// Each pair asserts the shape is DISTINGUISHED, which is the only thing a serializer can be
/// wrong about here: collapsing `<Menu.Item />` and `<Menu.Divider />` would make two different
/// components compare equal, and that is a false positive in a component suite.
#[test]
fn jsx_element_name_forms_are_each_distinguished() {
    // A dotted component name, and a sibling under the same namespace.
    assert_ne!(
        alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
        alpha_of_tsx("const el = render(<Menu.Divider>Open</Menu.Divider>);"),
        "two parts of one namespaced component render different things"
    );
    // Nesting one level deeper exercises the recursive arm.
    assert_ne!(
        alpha_of_tsx("const el = render(<Menu.Item.Label>Open</Menu.Item.Label>);"),
        alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
        "`Menu.Item.Label` is not `Menu.Item`"
    );
    // `<this.X />` reads the enclosing `this`, the same fact the non-JSX path tracks.
    assert_ne!(
        alpha_of_tsx("const el = render(<this.Panel>Open</this.Panel>);"),
        alpha_of_tsx("const el = render(<Panel>Open</Panel>);"),
        "a component read off `this` is not a free component of the same name"
    );
    // An intrinsic (lowercase) and a component (capitalized) are different by JSX's grammar.
    assert_ne!(
        alpha_of_tsx("const el = render(<button>Save</button>);"),
        alpha_of_tsx("const el = render(<Button>Save</Button>);"),
        "`<button>` is a DOM tag and `<Button>` is a component reference"
    );
    // ...and the same shape twice is still equal, so the inequalities above mean something.
    assert_eq!(
        alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
        alpha_of_tsx("const el = render(<Menu.Item>Open</Menu.Item>);"),
        "identical dotted names compare equal, or every assertion here is vacuous"
    );
}

/// The case that named the defect: the same element, written across several lines instead of
/// one. Formatting is not a behavioral difference, and the raw slice this arm used to emit
/// made it one.
#[test]
fn jsx_formatting_does_not_change_alpha() {
    assert_eq!(
        alpha_of_tsx(
            "const el = render(<Button variant=\"primary\" size=\"large\">Save</Button>);"
        ),
        alpha_of_tsx(
            "const el = render(\n  <Button variant=\"primary\" size=\"large\">\n    Save\n  </Button>,\n);"
        ),
        "an element reindented by a formatter renders identically and must fingerprint \
             identically in `alpha`"
    );
}

/// The other face of the same defect: an attribute literal is a VALUE, so `alpha` keeps it and
/// `structural` collapses it to the kind -- the exact relation that makes
/// `parameterization-candidate` possible outside JSX, which the raw slice hid.
#[test]
fn jsx_attribute_literal_is_a_literal_to_both_views() {
    let primary = "const el = render(<Button variant=\"primary\">Save</Button>);";
    let secondary = "const el = render(<Button variant=\"secondary\">Save</Button>);";
    assert_ne!(
        alpha_of_tsx(primary),
        alpha_of_tsx(secondary),
        "`alpha` preserves the attribute's value"
    );
    assert_eq!(
        structural_of_tsx(primary),
        structural_of_tsx(secondary),
        "`structural` collapses it to its kind, exactly as a literal outside JSX would"
    );
}

/// The precision counterweight: two different components taking the same props and children
/// must stay distinct, and so must an intrinsic and a component of the same name-shape.
#[test]
fn jsx_element_names_do_not_collapse() {
    assert_ne!(
        alpha_of_tsx("const el = render(<Button variant=\"primary\" />);"),
        alpha_of_tsx("const el = render(<Link variant=\"primary\" />);"),
        "two different component references are two different elements"
    );
    assert_ne!(
        alpha_of_tsx("const el = render(<button variant=\"primary\" />);"),
        alpha_of_tsx("const el = render(<Button variant=\"primary\" />);"),
        "an intrinsic `button` and a component `Button` are different elements, and the \
             intrinsic:/component: prefixes keep the two identifier variants from colliding"
    );
}

/// One element exercising every sub-form the serializer claims to model, with the exact
/// fragment each produces -- the JSX counterpart of `SERIALIZED_FRAGMENTS`' per-arm matrix.
#[test]
fn jsx_sub_forms_serialize_to_their_named_fragments() {
    let alpha =
        alpha_of_tsx("const el = render(<div className=\"x\" disabled {...rest}>{count}</div>);");
    assert!(alpha.contains("(jsx-elem intrinsic:div "), "name: {alpha}");
    assert!(
        alpha.contains("(jsx-attr className <string:x>) "),
        "value: {alpha}"
    );
    assert!(
        alpha.contains("(jsx-attr disabled) "),
        "boolean attribute: {alpha}"
    );
    assert!(
        alpha.contains("(jsx-spread-attr id:global:rest) "),
        "spread attribute: {alpha}"
    );
    assert!(
        alpha.contains("(jsx-expr id:global:count)"),
        "expression child: {alpha}"
    );

    let fragment = alpha_of_tsx("const all = <>{'a'}{items}</>;");
    assert!(
        fragment.contains("(jsx-frag (jsx-expr <string:a>) (jsx-expr id:global:items))"),
        "fragment with a literal-expression child and an identifier child: {fragment}"
    );

    let nested = alpha_of_tsx("const row = <Row label=<Icon /> />;");
    assert!(
        nested.contains("(jsx-attr label (jsx-elem "),
        "an element as an attribute value nests: {nested}"
    );

    let namespaced = alpha_of_tsx("const x = <f:bar />;");
    assert!(
        namespaced.contains("jsx-ns:f:bar"),
        "namespaced name: {namespaced}"
    );

    let member = alpha_of_tsx("const x = <Mods.Widget />;");
    assert!(
        member.contains("(jsx-member component:global:Mods .Widget)"),
        "member-expression name resolves its object like any reference: {member}"
    );

    let spread_children = alpha_of_tsx("const x = <div>{...items}</div>;");
    assert!(
        spread_children.contains("(jsx-spread-children id:global:items)"),
        "spread children: {spread_children}"
    );
}

/// `normalize_jsx_text` on the shapes that decide whether two layouts render the same text:
/// each row is (raw source text, what a renderer places in the DOM).
#[test]
fn jsx_text_normalizes_the_way_a_transform_renders_it() {
    // The corpus case: a single text child, reindented.
    assert_eq!(normalize_jsx_text("\n  Save\n"), "Save");
    assert_eq!(normalize_jsx_text("Save"), "Save");
    // Words on separate lines join with one space, interior indentation vanishes.
    assert_eq!(normalize_jsx_text("\n  Hello\n  World\n"), "Hello World");
    assert_eq!(normalize_jsx_text("Hello World"), "Hello World");
    // Blank interior lines and whitespace-only children render nothing.
    assert_eq!(normalize_jsx_text("\n\n    \n"), "");
    assert_eq!(normalize_jsx_text("   "), "");
    assert_eq!(normalize_jsx_text(""), "");
    // Tabs are whitespace like any other.
    assert_eq!(normalize_jsx_text("\n\tSave\n"), "Save");
    // CRLF and lone CR are line breaks like LF.
    assert_eq!(normalize_jsx_text("\r\n  Save\r\n"), "Save");
    assert_eq!(normalize_jsx_text("\r  Save\r"), "Save");
    // Leading text on the FIRST line keeps its leading spaces; a run of them is content.
    assert_eq!(normalize_jsx_text("  Save  More"), "  Save  More");
    // Text around a nested element on one line: the spaces next to the braces are content.
    assert_eq!(normalize_jsx_text("Hello "), "Hello ");
}

/// F1 regression: adversarial collision for the unescaped `<kind:raw>` literal encoding.
///
/// Before `escape_literal_delimiters` existed, a single string literal whose *value* spelled
/// out this serializer's own delimiter syntax could render byte-identical to a sequence of
/// several genuinely separate sibling literals. Concretely: `['x> <string:y']` (one array,
/// one string element containing `x> <string:y`) and `['x', 'y']` (one array, two ordinary
/// string elements) both serialized their elements to `<string:x> <string:y>` -- the first
/// literal's own raw text supplied the closing `>`, a space, and the *next* literal's
/// complete `<string:y>` shell, and `ser_array_element`'s `" "`.join could not tell that
/// forged boundary apart from a real one between two elements. That made two structurally
/// (and behaviorally) different test bodies produce the same `alpha`, which is exactly what
/// `duplicate-test` keys on at Error severity -- an adjudicator-provable false duplicate
/// build failure, self-inflicted by the kind-tagging fix `literal`'s doc comment describes.
/// This asserts the two `alpha` values now differ.
#[test]
fn literal_payload_cannot_forge_the_delimiter_grammar() {
    let source = "test('a', () => { const x = ['x> <string:y']; });\n\
                       test('b', () => { const x = ['x', 'y']; });";
    let prints = fingerprint_all(source);
    assert_eq!(prints.len(), 2, "expected exactly two test bodies");
    assert_ne!(
        prints[0].alpha, prints[1].alpha,
        "a one-element array whose string literal *contains* this serializer's own \
             `<kind:...>` delimiter syntax must not collide with a genuinely different \
             two-element array of separate string literals"
    );
}

/// This is the regression test for the bug an earlier draft shipped: identifying a free
/// reference by `SymbolId` instead of by what it resolves to. `SymbolId` is a per-file,
/// declaration-order index, so two *separately parsed* files (own `Allocator`, own
/// `Semantic`, and here deliberately different import orderings so `render`'s `SymbolId`
/// differs between them) that both import the same name from the same specifier would get
/// different, incomparable identities under that scheme -- silently defeating cross-file
/// `duplicate-test` for the single most common case (a shared imported helper). The fix
/// keys a free import's identity off the module specifier and exported name instead, which
/// this asserts directly.
#[test]
fn free_import_identity_is_stable_across_separately_parsed_files() {
    let source_a = "import { render } from './render';\ntest('a', () => { render(1); });";
    let source_b = "import { other } from './unrelated';\n\
                         import { render } from './render';\n\
                         test('b', () => { render(1); });";
    let a = fingerprint_all(source_a);
    let b = fingerprint_all(source_b);
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_eq!(
        a[0].alpha, b[0].alpha,
        "the same name imported from the same specifier in two independently parsed files \
             must produce the same alpha, regardless of what else either file imports first \
             (which would shift `render`'s SymbolId if identity were keyed on it)"
    );
}

/// A6 regression: an adversarial review proved that keying a relative import's identity on
/// the raw specifier text alone (no directory anchor) makes `pkgA/a.test.ts` and
/// `pkgB/b.test.ts`, each independently writing `import { helper } from './helpers'` (two
/// different files on disk, in two different directories), collapse to the same `alpha` --
/// a false `duplicate-test`. Two separate `Allocator`/`Semantic` instances, two different
/// `file` paths, identical bodies: the fix must tell them apart.
#[test]
fn free_import_identity_is_anchored_to_the_importing_files_directory() {
    let source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
    let a = fingerprint_all_at(source, Path::new("/repo/pkgA/a.test.ts"));
    let b = fingerprint_all_at(source, Path::new("/repo/pkgB/b.test.ts"));
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_ne!(
        a[0].alpha, b[0].alpha,
        "`./helpers` imported from two different directories names two different files and \
             must not produce the same alpha, even though the specifier text is identical"
    );

    // The positive case must still hold: two files in the *same* directory importing the
    // same relative specifier really do name the same file, and must still match.
    let c = fingerprint_all_at(source, Path::new("/repo/pkgA/c.test.ts"));
    assert_eq!(
        a[0].alpha, c[0].alpha,
        "`./helpers` imported from two files in the *same* directory does name the same \
             file and must still produce the same alpha"
    );
}

/// With a real file on disk to resolve against, two *differently spelled* relative
/// specifiers that name the same file must produce the same `alpha` -- lexical anchoring could
/// never do this (`./helpers` and `./sub/../helpers` anchor to different literal strings), but
/// real resolution must, because they are the same import.
#[test]
fn free_import_identity_matches_when_different_specifiers_resolve_to_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("helpers.ts"), "export const helper = 1;").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();

    let direct_source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
    let roundabout_source =
        "import { helper } from './sub/../helpers';\ntest('x', () => { helper(1); });";

    let direct = fingerprint_all_at(direct_source, &dir.path().join("a.test.ts"));
    let roundabout = fingerprint_all_at(roundabout_source, &dir.path().join("b.test.ts"));
    assert_eq!(direct.len(), 1);
    assert_eq!(roundabout.len(), 1);
    assert_eq!(
        direct[0].alpha, roundabout[0].alpha,
        "two specifiers that resolve to the same real file must produce the same alpha, even \
             written from different directories and spelled differently"
    );
}

/// Resolution regression, real-resolution flavor of the lexical-anchoring test above: two
/// directories that each
/// contain their *own*, different `helpers.ts` and both write the textually identical
/// `import { helper } from './helpers'` must resolve to two different files and must not
/// collapse to the same `alpha` -- the exact case real resolution must not regress relative to
/// lexical anchoring.
#[test]
fn free_import_identity_differs_when_the_same_specifier_text_resolves_to_different_files() {
    let dir = tempfile::tempdir().unwrap();
    let pkg_a = dir.path().join("pkgA");
    let pkg_b = dir.path().join("pkgB");
    std::fs::create_dir(&pkg_a).unwrap();
    std::fs::create_dir(&pkg_b).unwrap();
    std::fs::write(pkg_a.join("helpers.ts"), "export const helper = 1;").unwrap();
    std::fs::write(pkg_b.join("helpers.ts"), "export const helper = 2;").unwrap();

    let source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
    let a = fingerprint_all_at(source, &pkg_a.join("a.test.ts"));
    let b = fingerprint_all_at(source, &pkg_b.join("b.test.ts"));
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_ne!(
        a[0].alpha, b[0].alpha,
        "`./helpers` resolved from two different directories, each containing its own real \
             `helpers.ts`, must name two different files and must not produce the same alpha"
    );
}

/// An import specifier that cannot be resolved (no matching file on disk) must never
/// produce an identity that collides with a real, resolved import -- even when the unresolved
/// specifier's own anchored text would otherwise print identically to part of a resolved
/// identity. This is the direct test of the `resolved:`/`unresolved:` tag split.
#[test]
fn unresolved_import_identity_never_collides_with_a_resolved_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("helpers.ts"), "export const helper = 1;").unwrap();

    let resolved_source = "import { helper } from './helpers';\ntest('x', () => { helper(1); });";
    let unresolved_source = "import { helper } from './missing';\ntest('x', () => { helper(1); });";

    let resolved = fingerprint_all_at(resolved_source, &dir.path().join("a.test.ts"));
    let unresolved = fingerprint_all_at(unresolved_source, &dir.path().join("b.test.ts"));
    assert_eq!(resolved.len(), 1);
    assert_eq!(unresolved.len(), 1);
    assert_ne!(
        resolved[0].alpha, unresolved[0].alpha,
        "a resolved import and an unresolvable one must never produce the same alpha, \
             regardless of how similar their specifier text looks"
    );
}

// ---------------------------------------------------------------------------------------
