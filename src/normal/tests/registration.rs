//! Normal and trace views of one proved invocation, without admitting other nested functions.
use super::*;
use crate::lang::Declaration;

/// One root function permits its own registrations, binds its inputs and leaves other roots opaque.
#[test]
fn invocation_boundaries_preserve_normal_and_trace_dimensions() {
    let source = "import {test,expect} from 'vitest';
      export function run(value) {
        test('bound',()=>expect(value).toBe('x'));
        function nested(){test('opaque',()=>expect(value).toBe('x'))}
        test('shadow',(value)=>expect(value).toBe('x'));
      }
      test('literal',()=>expect('x').toBe('x'));
      test('different',()=>expect('y').toBe('x'));";
    crate::testing::project(source, &[], false, |path, parsed, found, _| {
        let tree = &parsed.tree;
        let structure = Structure::of(parsed, found);
        let exports = Exports::default();
        let setting = Setting {
            dialect: &DIALECT,
            root: path,
            exports: &exports,
        };
        let Declaration::Function(function) = parsed
            .scopes
            .symbol(parsed.scopes.top_level("run").unwrap())
            .declaration
        else {
            panic!("function declaration")
        };
        let parameter = tree.list(function, "params")[0].unwrap();
        let parameter = parsed.scopes.symbol_at(tree.span(parameter).start).unwrap();
        let callbacks: HashMap<_, _> = test_callbacks(tree, found).collect();
        let site = |title| {
            found
                .sites
                .iter()
                .find(|site| site.title.as_deref() == Some(title))
                .unwrap()
                .node
        };
        let file = path.join("a.test.tsx");
        let mut ordinary = Normalizer::new(&file, parsed, setting, &structure);
        assert!(ordinary.suites_around(site("bound")).is_none());
        let literal = tree
            .nodes()
            .find(|&node| tree.kind(node) == "Literal" && tree.text(node, "value") == Some("x"))
            .unwrap();
        let binding = crate::normal::InvocationValue {
            form: ordinary.of(literal),
            shape: ordinary.shape(literal, HashMap::new(), false),
        };
        let mut framed = Normalizer::new(&file, parsed, setting, &structure)
            .for_registration(function, HashMap::from([(parameter, binding)]));
        assert_eq!(framed.suites_around(site("bound")), Some(Vec::new()));
        assert!(framed.suites_around(site("opaque")).is_none());
        assert_eq!(
            framed.of(callbacks["bound"]),
            ordinary.of(callbacks["literal"])
        );
        assert_ne!(
            framed.of(callbacks["bound"]),
            ordinary.of(callbacks["different"])
        );
        assert_ne!(
            framed.of(callbacks["bound"]),
            framed.of(callbacks["shadow"])
        );
        let trace = crate::behavior::trace(
            &mut framed,
            parsed,
            callbacks["bound"],
            HashMap::new(),
            false,
        )
        .0;
        let direct = crate::behavior::trace(
            &mut ordinary,
            parsed,
            callbacks["literal"],
            HashMap::new(),
            false,
        )
        .0;
        assert!(trace.complete && direct.complete);
        assert!(!trace.events.is_empty());
        assert_eq!(
            trace.events,
            direct.events,
            "framed={:?}; direct={:?}",
            crate::behavior::trace_text(
                &mut framed,
                parsed,
                callbacks["bound"],
                HashMap::new(),
                false
            ),
            crate::behavior::trace_text(
                &mut ordinary,
                parsed,
                callbacks["literal"],
                HashMap::new(),
                false
            )
        );
    });
}

/// A parameter read in a title uses only its own symbol's proved invocation value.
#[test]
fn invocation_title_values_do_not_capture_shadowed_bindings() {
    let source = "import {test} from 'vitest'; export function run(label) { test(`${label} first`,()=>{}); function nested(label){test(`${label} second`,()=>{})} }";
    crate::testing::project(source, &[], false, |_, parsed, found, _| {
        let symbol = run_parameter(parsed);
        let values = HashMap::from([(symbol, serde_json::json!("page"))]);
        let sites: Vec<_> = found
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
            .collect();
        assert_eq!(sites.len(), 2);
        assert_eq!(
            crate::normal::titles::title_with_bindings(parsed, sites[0].node, &values).as_deref(),
            Some("page first")
        );
        assert_eq!(
            crate::normal::titles::title_with_bindings(parsed, sites[1].node, &values),
            None
        );
    });
}

/// A title past 4,096 bytes reads as unknown, written out, through a `const` or in a template;
/// one at the limit is read. A literal value written in place is read at any length: only text
/// every reader copies again is limited.
#[test]
fn a_title_past_the_text_limit_is_unknown() {
    let at = "0".repeat(crate::lang::limits::MAX_TEXT_BYTES);
    // A function title's name: `_` and these zeros spell the limit exactly.
    let name = "0".repeat(crate::lang::limits::MAX_TEXT_BYTES - 1);
    let big = "0".repeat(65_537);
    let (half, rest) = ("0".repeat(2_500), "0".repeat(2_000));
    let source = format!(
        "import {{test}} from 'vitest'; const at = '{at}'; const past = '{at}0'; const half = '{half}'; const rest = '{rest}'; const listed = ['{half}']; const fits = {{ name: '{at}', id: 1 }}; const over = {{ name: '{at}0', id: 1 }};
         test('{at}',()=>{{}}); test('{at}0',()=>{{}});
         test(at,()=>{{}}); test(past,()=>{{}});
         test(`${{at}}`,()=>{{}}); test(`${{at}}0`,()=>{{}}); test(`0${{at}}`,()=>{{}});
         test(`{at}0`,()=>{{}}); test(`${{at}}${{at}}`,()=>{{}}); test(`${{'{at}'}}0`,()=>{{}});
         test(function _{name}(){{}},()=>{{}}); test(function _{name}0(){{}},()=>{{}});
         test([at, at],()=>{{}}); test(`${{'{big}'}}`,()=>{{}}); test(`{name}${{12}}`,()=>{{}});
         test({{a: half, b: rest}},()=>{{}}); test([listed, half],()=>{{}});
         test(fits.id,()=>{{}}); test(over.id,()=>{{}});"
    );
    crate::testing::project(&source, &[], false, |_, parsed, found, normalizer| {
        let sites: Vec<_> = found
            .sites
            .iter()
            .filter(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
            .collect();
        let titles: Vec<_> = sites
            .iter()
            .map(|site| crate::normal::titles::title(parsed, site.node).map(|title| title.len()))
            .collect();
        let read = Some(at.len());
        assert_eq!(
            titles,
            [
                read, None, read, None, read, None, None, None, None, None, read, None, None, None,
                None, None, None, None, None
            ]
        );
        let written = parsed.tree.list(sites[1].node, "arguments")[0].unwrap();
        let value =
            crate::normal::titles::value_with_bindings(parsed, written, 0, &mut 1, &HashMap::new());
        assert_eq!(
            value.and_then(|v| v.as_str().map(str::len)),
            Some(at.len() + 1)
        );
        // Each read of a `const` copies it again, so a long one is unknown wherever it is read.
        let read = |site: usize| {
            let argument = parsed.tree.list(sites[site].node, "arguments")[0].unwrap();
            crate::normal::titles::value_with_bindings(
                parsed,
                argument,
                0,
                &mut 256,
                &HashMap::new(),
            )
            .and_then(|v| v.as_str().map(str::len))
        };
        assert_eq!((read(2), read(3)), (Some(at.len()), None));
        // Row values are proved by their own reader, which measures a `const` the same way.
        let scalar = |site: usize, normalizer: &mut crate::normal::Normalizer| {
            let argument = parsed.tree.list(sites[site].node, "arguments")[0].unwrap();
            match normalizer.row_scalar(argument)? {
                crate::lang::Scalar::String(text) => Some(text.len()),
                _ => None,
            }
        };
        assert_eq!(
            (scalar(2, normalizer), scalar(3, normalizer)),
            (Some(at.len()), None)
        );
        // A member read copies the whole `const` object, so all its text counts.
        let member = |site: usize, normalizer: &mut crate::normal::Normalizer| {
            let argument = parsed.tree.list(sites[site].node, "arguments")[0].unwrap();
            normalizer.row_scalar(argument)
        };
        assert!(member(17, normalizer).is_some());
        assert!(member(18, normalizer).is_none());
        // One budget serves a read: naming a `const` twice spends it twice.
        assert_eq!(read(8), None);
        let pair = parsed.tree.list(sites[12].node, "arguments")[0].unwrap();
        let pair =
            crate::normal::titles::value_with_bindings(parsed, pair, 0, &mut 256, &HashMap::new());
        assert_eq!(pair, None);
        // A row reads its cells from one budget too.
        let cells = parsed.tree.list(
            parsed.tree.list(sites[12].node, "arguments")[0].unwrap(),
            "elements",
        );
        let row = crate::normal::Row::Spread(cells.into_iter().flatten().collect());
        let (arguments, _) = crate::normal::titles::row(parsed, &row, false);
        let lengths: Vec<_> = arguments
            .iter()
            .map(|v| v.as_ref().and_then(|v| v.as_str().map(str::len)))
            .collect();
        assert_eq!(lengths, [Some(at.len()), None]);
        // A field read after a failed whole-row read starts from the row's budget: `a` fits.
        let object = parsed.tree.list(sites[15].node, "arguments")[0].unwrap();
        let (_, fields) =
            crate::normal::titles::row(parsed, &crate::normal::Row::Whole(object, None), false);
        let fields = fields.unwrap();
        assert_eq!(
            fields.get("a").and_then(|v| v.as_str()).map(str::len),
            Some(2_500)
        );
        assert!(!fields.contains_key("b"));
        // A `const` refused as an array spends nothing: the cell after it still fits.
        let cells = parsed.tree.list(
            parsed.tree.list(sites[16].node, "arguments")[0].unwrap(),
            "elements",
        );
        let row = crate::normal::Row::Spread(cells.into_iter().flatten().collect());
        let (arguments, _) = crate::normal::titles::row(parsed, &row, false);
        let lengths: Vec<_> = arguments
            .iter()
            .map(|v| v.as_ref().and_then(|v| v.as_str().map(str::len)))
            .collect();
        assert_eq!(lengths, [None, Some(2_500)]);
        // A template written in place copies its text once per place, so only its own length
        // limit applies to it.
        assert_eq!(read(9), Some(at.len() + 1));
        assert_eq!(read(13), None);
        // A runner charges its arguments to its own budget, templates included.
        let template = parsed.tree.list(sites[9].node, "arguments")[0].unwrap();
        let argument = crate::normal::titles::argument(parsed, template, &HashMap::new());
        assert_eq!(
            argument.and_then(|v| v.as_str().map(str::len)),
            Some(at.len() + 1)
        );
    });
}

/// A title read through an invocation binding is limited as one written out: the binding is
/// measured before each read copies it.
#[test]
fn a_bound_title_past_the_text_limit_is_unknown() {
    let source = "import {test} from 'vitest'; export function run(label) { test(label,()=>{}) }";
    crate::testing::project(source, &[], false, |_, parsed, found, _| {
        let symbol = run_parameter(parsed);
        let site = found
            .sites
            .iter()
            .find(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
            .unwrap();
        let at = "0".repeat(crate::lang::limits::MAX_TEXT_BYTES);
        let title = |label: String| {
            let values = HashMap::from([(symbol, serde_json::Value::String(label))]);
            crate::normal::titles::title_with_bindings(parsed, site.node, &values).map(|t| t.len())
        };
        assert_eq!(title(at.clone()), Some(at.len()));
        assert_eq!(title(format!("{at}0")), None);
    });
}

/// The first parameter of the module's top-level `run` function.
fn run_parameter(parsed: &crate::lang::Parsed) -> crate::lang::SymbolId {
    let tree = &parsed.tree;
    let Declaration::Function(function) = parsed
        .scopes
        .symbol(parsed.scopes.top_level("run").unwrap())
        .declaration
    else {
        panic!("function")
    };
    let parameter = tree.list(function, "params")[0].unwrap();
    parsed.scopes.symbol_at(tree.span(parameter).start).unwrap()
}

/// A runner caller's title is read within a budget that counts every string the read copies,
/// not only the title it yields: an object substituted into a template copies each property.
#[test]
fn a_title_read_within_a_budget_counts_what_it_copies() {
    let long = "0".repeat(crate::lang::limits::MAX_TEXT_BYTES + 1);
    let source = format!(
        "import {{test}} from 'vitest'; const LONG = '{long}';
         test(`${{{{ a: LONG, b: LONG }}}}`,()=>{{}});"
    );
    crate::testing::project(&source, &[], false, |_, parsed, found, _| {
        let site = found
            .sites
            .iter()
            .find(|site| site.kind == SiteKind::Registers(crate::framework::Registers::Test))
            .unwrap();
        let title = parsed.tree.list(site.node, "arguments")[0].unwrap();
        // Two keys of one byte and two copies of LONG.
        let copied = 2 * (1 + long.len());
        assert_eq!(
            crate::normal::titles::title_within(parsed, title, copied),
            Some(("[object Object]".to_owned(), copied))
        );
        assert_eq!(
            crate::normal::titles::title_within(parsed, title, copied - 1),
            None
        );
    });
}
