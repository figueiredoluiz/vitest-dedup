use super::*;

// A raw-text fallback has to say what its names mean
// -----------------------------------------------------------------------------------------
//
// `opaque` and `degraded` emit the verbatim source slice. The rest of the body is alpha-renamed,
// row-substituted and import-anchored, so the slice alone cannot say which binding a name meant:
// `[x] = [1]` assigns whichever variable `x` names, and past the depth cap `config.v` reads
// whichever `config` is in scope. Two bodies whose slices read alike then compared equal while
// meaning different things, and `duplicate-test` reported them at error severity. The fallback now
// appends the identity of each binding the slice uses from outside itself, and a slice using `this`, `arguments` or `new.target` of a function outside it makes the
// body depend on its callback form.
//
// Each case pairs a distinct pair with a control that must stay equal: a fix that simply refused
// to compare raw text would pass the first half alone.

/// More levels than `MAX_DEPTH` allows, so the innermost expression is rendered by `degraded`.
const PAST_THE_CAP: usize = 300;

/// `leaf` wrapped in `PAST_THE_CAP` array literals.
fn deep(leaf: &str) -> String {
    format!(
        "{}{leaf}{}",
        "[".repeat(PAST_THE_CAP),
        "]".repeat(PAST_THE_CAP)
    )
}

/// The `alpha` view of every test body in `source`, in source order.
fn alphas(source: &str) -> Vec<String> {
    fingerprint_all(source)
        .into_iter()
        .map(|print| print.alpha)
        .collect()
}

/// The `alpha` views of exactly two test bodies.
fn pair(source: &str) -> (String, String) {
    let mut views = alphas(source);
    assert_eq!(views.len(), 2, "expected two test bodies in {source:.80}");
    let second = views.pop().expect("two bodies");
    (views.pop().expect("two bodies"), second)
}

/// The callback form a single test body depends on.
fn callback_form_of(source: &str) -> CallbackForm {
    let prints = fingerprint_all(source);
    assert_eq!(prints.len(), 1, "expected one test body in {source:.80}");
    prints[0].callback_form
}

/// A slice kept past the depth cap carries the identity of the binding it names.
#[test]
fn a_depth_limited_slice_carries_the_identity_of_the_outside_binding_it_names() {
    let leaf = deep("config.v");
    // Two separate `config` declarations are two bindings, however alike their text.
    let (first, second) = pair(&format!(
        "describe('a', () => {{ const config = {{ v: 1 }}; \
         test('t', () => {{ const x = {leaf}; }}); }}); \
         describe('b', () => {{ const config = {{ v: 1 }}; \
         test('t', () => {{ const x = {leaf}; }}); }});"
    ));
    assert_ne!(
        first, second,
        "different bindings behind identical raw text"
    );
    assert!(
        first.contains("<max-depth:"),
        "the slice must still be raw text"
    );
    // One binding used by both tests is the control: identical text, identical meaning.
    let (one, again) = pair(&format!(
        "const config = {{ v: 1 }}; \
         test('a', () => {{ const x = {leaf}; }}); \
         test('b', () => {{ const x = {leaf}; }});"
    ));
    assert_eq!(one, again, "the same binding behind identical raw text");
}

/// A binding the body declares itself is named by its canonical slot, not its spelling.
#[test]
fn an_unmodeled_construct_carries_which_variable_each_name_stands_for() {
    // Destructuring assignment is rendered as raw text. In the first test `[x]` assigns the first
    // declared variable, in the second the other one -- and only the first test passes.
    let (assigns_first, assigns_second) = pair(
        "test('a', () => { let x = 0; let y = 0; [x] = [1]; expect(y).toBe(0); });\n\
         test('b', () => { let y = 0; let x = 0; [x] = [1]; expect(x).toBe(0); });",
    );
    assert_ne!(assigns_first, assigns_second);
    let (one, two) = pair(
        "test('a', () => { let x = 0; let y = 0; [x] = [1]; expect(y).toBe(0); });\n\
         test('b', () => { let x = 0; let y = 0; [x] = [1]; expect(y).toBe(0); });",
    );
    assert_eq!(one, two, "the same mapping from names to variables");
}

/// Slices that name nothing outside themselves keep exactly the text they had.
#[test]
fn a_slice_that_names_nothing_outside_itself_is_unchanged() {
    let [meta] = alphas("test('t', () => { const v = import.meta.env; });")
        .try_into()
        .expect("one body");
    assert!(meta.contains("<opaque:import.meta>"), "{meta}");
    let global = deep("Math.PI");
    let [globals] = alphas(&format!("test('t', () => {{ const x = {global}; }});"))
        .try_into()
        .expect("one body");
    assert!(!globals.contains('|'), "a global is named by itself");
    let own = deep("((n) => n)(1)");
    let [inner] = alphas(&format!("test('t', () => {{ const x = {own}; }});"))
        .try_into()
        .expect("one body");
    assert!(
        !inner.contains('|'),
        "a parameter declared inside the slice"
    );
}

/// `this`, `arguments` and `new.target` of a function outside the slice make the body depend on
/// whether its callback is an arrow.
#[test]
fn a_slice_that_uses_an_outside_this_depends_on_the_callback_form() {
    let this_value = deep("this.v");
    assert_eq!(
        callback_form_of(&format!("test('t', () => {{ const x = {this_value}; }});")),
        CallbackForm::Arrow,
        "`this` of the callback"
    );
    assert_eq!(
        callback_form_of(&format!(
            "function suite() {{ test('t', () => {{ const x = {this_value}; }}); }}"
        )),
        CallbackForm::Arrow,
        "`this` of a function enclosing the test"
    );
    let arguments = deep("arguments");
    assert_eq!(
        callback_form_of(&format!("test('t', () => {{ const x = {arguments}; }});")),
        CallbackForm::Arrow,
        "the free `arguments`"
    );
    assert_eq!(
        callback_form_of(&format!(
            "test('t', () => {{ function inner() {{ return {this_value}; }} inner(); }});"
        )),
        CallbackForm::Interchangeable,
        "`this` of a function the body declares"
    );
    assert_eq!(
        callback_form_of(&format!(
            "test('t', () => {{ const x = {}; }});",
            deep("Math.PI")
        )),
        CallbackForm::Interchangeable,
        "no `this` at all"
    );
    // A class binds `this` for its static blocks and field initializers, but not for a computed
    // key, which is evaluated in the scope around the class.
    for (leaf, expected, name) in [
        (
            "(class { v = this.w })",
            CallbackForm::Interchangeable,
            "a field initializer",
        ),
        (
            "(class { static { this.x = 1; } })",
            CallbackForm::Interchangeable,
            "a static block",
        ),
        (
            "(class { accessor a = this.w })",
            CallbackForm::Interchangeable,
            "an accessor initializer",
        ),
        (
            "(class { [this.k] = 1 })",
            CallbackForm::Arrow,
            "a computed key",
        ),
    ] {
        assert_eq!(
            callback_form_of(&format!(
                "test('t', () => {{ const x = {}; }});",
                deep(leaf)
            )),
            expected,
            "`this` in {name}"
        );
    }
}

/// The body scan relies on a node's descendants being the run of ids after it whose parent is not
/// before it. Nodes are NOT created in source order (a template literal's expressions precede its
/// quasis), so the run is pinned against a brute-force walk of each node's ancestors, over the
/// constructs most likely to reorder them.
#[test]
fn a_nodes_descendants_are_the_run_after_it_whose_parent_is_not_before_it() {
    let sources = [
        ("a.ts", "@dec() class C { @a x = 1; static #p = 2; m(@p y: number) { return this.#p; } }"),
        ("a.ts", "const { a = 1, b: [c = 2] } = o; label: for (const k of ks) { break label; }"),
        ("a.ts", "type T<U> = U extends string ? 1 : 2; enum E { A = 1 } namespace N { export const v = 1; }"),
        ("a.ts", "const t = tag`a${1}b${f(2)}`; const o = { get x() { return 1; }, [k]: 2, ...r };"),
        ("a.tsx", "const e = <A b={1} {...p}>{c}<D /></A>;"),
        ("a.ts", "async function* g() { for await (const x of y) { yield* z; } }"),
    ];
    for (name, source) in sources {
        let allocator = Allocator::default();
        let parsed = Parser::new(
            &allocator,
            source,
            SourceType::from_path(name).expect("source type"),
        )
        .parse();
        let semantic = SemanticBuilder::new()
            .with_build_nodes(true)
            .build(&parsed.program)
            .semantic;
        let nodes = semantic.nodes();
        for (root, _) in nodes.iter_enumerated() {
            let run: Vec<_> = nodes
                .iter_enumerated()
                .skip(root.index())
                .map_while(|(id, _)| {
                    (id == root || nodes.parent_id(id).index() >= root.index()).then_some(id)
                })
                .collect();
            let walked: Vec<_> = nodes
                .iter_enumerated()
                .filter(|(id, _)| {
                    *id == root || nodes.ancestors(*id).any(|ancestor| ancestor.id() == root)
                })
                .map(|(id, _)| id)
                .collect();
            assert_eq!(run, walked, "{source}: subtree of node {}", root.index());
        }
    }
}

/// A raw slice that names a class declared in the body must not make identical bodies
/// incomparable: the binding is tied to its slot, and the class itself is rendered where it is
/// declared. Rendering the name through the class-expanding path used to mark both bodies
/// unresolved and drop a genuine duplicate.
#[test]
fn a_raw_slice_naming_a_class_instance_keeps_identical_bodies_comparable() {
    let body = "class Box { value = 0 } const box = new Box(); ;[box.value] = [1]; \
                expect(box.value).toBe(1);";
    let prints = fingerprint_all(&format!(
        "test('a', () => {{ {body} }}); test('b', () => {{ {body} }});"
    ));
    assert_eq!(prints.len(), 2);
    assert!(
        prints.iter().all(|print| !print.class_unresolved),
        "identical bodies must stay comparable"
    );
    assert_eq!(prints[0].alpha, prints[1].alpha);
    assert!(
        prints[0].alpha.contains("|box=id:"),
        "the name is still tied to its binding: {}",
        prints[0].alpha
    );
}
