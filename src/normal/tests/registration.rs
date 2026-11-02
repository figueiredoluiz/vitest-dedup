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
        let tree = &parsed.tree;
        let Declaration::Function(function) = parsed
            .scopes
            .symbol(parsed.scopes.top_level("run").unwrap())
            .declaration
        else {
            panic!("function")
        };
        let parameter = tree.list(function, "params")[0].unwrap();
        let symbol = parsed.scopes.symbol_at(tree.span(parameter).start).unwrap();
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
