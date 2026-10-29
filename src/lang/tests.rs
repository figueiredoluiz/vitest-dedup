use super::build::spell_lone_surrogates;
use super::{parse, resolve, resolve_file, Declaration, Imported, Lines, Name, Parsed, Refusal};
use std::fs;
use std::path::{Path, PathBuf};

fn parsed(source: &str) -> Parsed {
    parse(Path::new("a.test.ts"), source).expect("parses")
}

/// The first node of `kind`.
fn first(parsed: &Parsed, kind: &str) -> super::NodeId {
    parsed
        .tree
        .nodes()
        .find(|&node| parsed.tree.kind(node) == kind)
        .unwrap_or_else(|| panic!("no {kind}"))
}

#[test]
fn the_tree_is_the_estree_serialization_with_parents() {
    let parsed = parsed("describe('é', () => { it('a', () => {}) })");
    let tree = &parsed.tree;
    assert_eq!(tree.kind(tree.root()), "Program");
    let calls: Vec<_> = tree
        .nodes()
        .filter(|&node| tree.kind(node) == "CallExpression")
        .collect();
    assert_eq!(calls.len(), 2);
    let inner = calls[1];
    assert!(tree.ancestors(inner).any(|ancestor| ancestor == calls[0]));
    // Byte offsets, as oxc's spans are: `é` is two bytes.
    let title = tree.list(calls[0], "arguments")[0].unwrap();
    assert_eq!(tree.span(title), super::Span { start: 9, end: 13 });
    assert_eq!(tree.text(title, "value"), Some("é"));
    let callee = tree.child(inner, "callee").unwrap();
    assert_eq!(tree.text(callee, "name"), Some("it"));
    // The arrow body's statement shares the call's span and holds it.
    let statement = tree.outermost(tree.span(inner)).unwrap();
    assert_eq!(tree.kind(statement), "ExpressionStatement");
    assert_eq!(
        tree.find(tree.span(inner), &["CallExpression"]),
        Some(inner)
    );
    assert!(tree.list(inner, "missing").is_empty());
    assert!(!tree.flag(inner, "optional"));
}

/// A node's descendants are every node inside its span, and nothing after it.
#[test]
fn descendants_are_the_nodes_inside_a_span() {
    let parsed = parsed("f(() => { a; b }); c");
    let tree = &parsed.tree;
    let arrow = first(&parsed, "ArrowFunctionExpression");
    let span = tree.span(arrow);
    let inside: Vec<_> = tree
        .nodes()
        .filter(|&node| node != arrow && span.contains(tree.span(node)))
        .collect();
    assert_eq!(tree.descendants(arrow).collect::<Vec<_>>(), inside);
    assert!(
        inside.len() >= 5,
        "the body, two statements and their identifiers"
    );
    assert_eq!(
        tree.descendants(tree.root()).count(),
        tree.nodes().count() - 1
    );
}

#[test]
fn wrappers_unwrap_to_the_expression_they_hold() {
    let parsed = parsed("const x = ((y as any)!) satisfies unknown");
    let tree = &parsed.tree;
    let declarator = first(&parsed, "VariableDeclarator");
    let init = tree.child(declarator, "init").unwrap();
    let inner = tree.inner(init);
    assert_eq!(tree.kind(inner), "Identifier");
    assert_eq!(tree.outer_parent(inner), Some(declarator));
}

#[test]
fn positions_are_one_based_lines_and_utf16_columns() {
    let source = "a\n😀b\nc";
    let lines = Lines::new(source);
    let b = source.find('b').unwrap() as u32;
    // `😀` is four UTF-8 bytes and two UTF-16 units.
    assert_eq!(lines.position(source, b), (2, 3));
    assert_eq!(lines.position(source, 0), (1, 1));
    assert_eq!(lines.line(source.len() as u32), 3);
    assert_eq!(lines.line(2), 2);
}

#[test]
fn scopes_classify_declarations_and_references() {
    let parsed = parsed(
        "import { it as test } from 'vitest'\n\
         import all, * as ns from './x'\n\
         let n = 0\n\
         function f() { n++ }\n\
         class C {}\n\
         test('a', (p) => { f(n, p, all, ns, C, undeclared) })",
    );
    let scopes = &parsed.scopes;
    let tree = &parsed.tree;
    let by_name = |name: &str| {
        scopes
            .symbols()
            .find(|&symbol| scopes.symbol(symbol).name == name)
            .unwrap_or_else(|| panic!("no symbol {name}"))
    };
    let test = by_name("test");
    match &scopes.symbol(test).declaration {
        Declaration::Import {
            import,
            specifier,
            imported,
        } => {
            assert_eq!(imported, &Imported::Named("it".to_owned()));
            assert_eq!(tree.kind(*import), "ImportDeclaration");
            assert_eq!(tree.kind(*specifier), "ImportSpecifier");
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        scopes.symbol(by_name("all")).declaration,
        Declaration::Import {
            imported: Imported::Default,
            ..
        }
    ));
    assert!(matches!(
        scopes.symbol(by_name("ns")).declaration,
        Declaration::Import {
            imported: Imported::Namespace,
            ..
        }
    ));
    let n = by_name("n");
    let Declaration::Variable(declarator) = scopes.symbol(n).declaration else {
        panic!("n is a variable");
    };
    assert_eq!(tree.kind(declarator), "VariableDeclarator");
    let writes: Vec<bool> = scopes
        .symbol(n)
        .references
        .iter()
        .map(|r| r.write)
        .collect();
    assert_eq!(writes, [true, false]);
    assert!(scopes.symbol(n).references.iter().all(|r| r.node.is_some()));
    assert!(matches!(
        scopes.symbol(by_name("f")).declaration,
        Declaration::Function(_)
    ));
    assert!(matches!(
        scopes.symbol(by_name("C")).declaration,
        Declaration::Class(_)
    ));
    let p = by_name("p");
    assert!(matches!(
        scopes.symbol(p).declaration,
        Declaration::Other(Some(_))
    ));
    assert!(!scopes.symbol(p).top_level);
    assert!(scopes.symbol(n).top_level);
    assert_eq!(scopes.top_level("n"), Some(n));
    assert_eq!(scopes.top_level("p"), None);
    assert_eq!(scopes.declaration_node(n), Some(declarator));
    let undeclared = parsed.tree.nodes().find(|&node| {
        tree.kind(node) == "Identifier" && tree.text(node, "name") == Some("undeclared")
    });
    let at = tree.span(undeclared.unwrap()).start;
    assert_eq!(scopes.name_at(at), Some(Name::Reference(None)));
    assert!(scopes.is_global_at(at));
    assert_eq!(scopes.symbol_at(scopes.symbol(n).span.start), Some(n));
}

#[test]
fn a_string_import_name_is_read_as_text() {
    let parsed = parsed("import { 'a-b' as x } from './m'\nx");
    let x = parsed.scopes.top_level("x").unwrap();
    assert!(matches!(
        &parsed.scopes.symbol(x).declaration,
        Declaration::Import { imported: Imported::Named(name), .. } if name == "a-b"
    ));
}

#[test]
fn refusals_name_their_cause() {
    assert_eq!(
        parse(Path::new("a.txt"), "x").err(),
        Some(Refusal::UnsupportedSource)
    );
    assert_eq!(parse(Path::new("a.ts"), "(((").err(), Some(Refusal::Fatal));
    let nested = |depth: usize| format!("x = {}1{}", "[".repeat(depth), "]".repeat(depth));
    assert_eq!(
        parse(Path::new("a.ts"), &nested(300)).err(),
        Some(Refusal::SourceLimit)
    );
    // Deeper than serde's own limit, within the preflight's.
    assert!(parse(Path::new("a.ts"), &nested(250)).is_ok());
    let recovered =
        parse(Path::new("a.ts"), "x = a ?? b || c; it('a', () => {})").expect("recovers");
    assert!(recovered.parse_errors > 0);
}

/// A source the preflight admits is parsed, however deep, without exhausting the caller's stack.
#[test]
fn an_admitted_source_parses_on_a_stack_of_its_own() {
    let source = format!("let a; {}0;", "a=".repeat(5_000));
    // A test thread's stack is a few megabytes, far less than the parse needs.
    let parsed = std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || parse(Path::new("a.ts"), &source).map(|parsed| parsed.tree.nodes().count()))
        .unwrap()
        .join()
        .unwrap();
    assert!(parsed.is_ok_and(|nodes| nodes > 10_000));
}

#[test]
fn a_lone_surrogate_is_spelled_as_the_string_reader_spells_it() {
    let parsed = parsed("it('\\ud800x\\ud83d\\ude00', () => {})");
    let literal = parsed
        .tree
        .nodes()
        .find(|&node| parsed.tree.kind(node) == "Literal")
        .unwrap();
    assert_eq!(parsed.tree.text(literal, "value"), Some("\u{FFFD}d800x😀"));
    assert_eq!(
        spell_lone_surrogates(r#"{"a":"\\ud800"}"#),
        r#"{"a":"\\ud800"}"#
    );
    assert_eq!(
        spell_lone_surrogates(r#""\uDC00\n""#),
        "\"\u{FFFD}dc00\\n\""
    );
    assert_eq!(spell_lone_surrogates(r#""A""#), r#""A""#);
}

/// A project directory with `files`, returned with its root.
fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let root = dir.path().to_path_buf();
    (dir, root)
}

#[test]
fn resolution_names_project_sources_only() {
    let (_dir, root) = project(&[
        ("src/a.test.ts", ""),
        ("src/helper.ts", ""),
        ("src/lib/index.ts", ""),
        ("src/alias/target.ts", ""),
        ("src/data.json", "{}"),
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["src/alias/*"]}}}"#,
        ),
        (
            "node_modules/pkg/package.json",
            r#"{"name":"pkg","main":"index.js"}"#,
        ),
        ("node_modules/pkg/index.js", ""),
    ]);
    let from = root.join("src/a.test.ts");
    let name = |specifier: &str| {
        resolve(&from, specifier, &root).map(|file| {
            file.strip_prefix(fs::canonicalize(&root).unwrap())
                .or_else(|_| file.strip_prefix(&root))
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
    };
    assert_eq!(name("./helper").as_deref(), Some("src/helper.ts"));
    assert_eq!(name("./lib").as_deref(), Some("src/lib/index.ts"));
    assert_eq!(name("@/target").as_deref(), Some("src/alias/target.ts"));
    assert_eq!(name("./data.json"), None);
    assert!(
        resolve_file(&from, "./data.json", &root).is_some_and(|file| file.ends_with("data.json"))
    );
    assert_eq!(resolve_file(&from, "pkg", &root), None);
    assert_eq!(name("pkg"), None);
    assert_eq!(name("./missing"), None);
    let inner = root.join("src");
    assert_eq!(resolve(&from, "../tsconfig.json", &inner), None);
    assert_eq!(resolve(&from, "./helper", &root.join("src/lib")), None);
}

/// The `oxc_*` crates `source` names in code (not in `//` comments).
fn oxc_paths(source: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or_default();
        let mut rest = code;
        while let Some(at) = rest.find("oxc_") {
            let before = rest[..at].chars().next_back();
            let name: String = rest[at..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            let after = &rest[at + name.len()..];
            if !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '`')
                && after.starts_with("::")
            {
                found.push(name.clone());
            }
            rest = &rest[at + name.len()..];
        }
    }
    found
}

/// Every `.rs` file under `dir`.
fn rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn only_the_parser_layers_import_oxc() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(
        files.len() >= 100,
        "only {} source files were scanned",
        files.len()
    );
    let allowed = |path: &Path| {
        let relative = path.strip_prefix(&src).unwrap();
        let first = relative.components().next().unwrap().as_os_str();
        ["frontend", "frontend.rs", "lang", "lang.rs"]
            .iter()
            .any(|allowed| first == *allowed)
    };
    let mut importing = 0;
    for file in files {
        let found = oxc_paths(&fs::read_to_string(&file).unwrap());
        if allowed(&file) {
            importing += usize::from(!found.is_empty());
            continue;
        }
        assert!(
            found.is_empty(),
            "{} names {found:?}; parse through `crate::lang`",
            file.display()
        );
    }
    assert!(
        importing >= 2,
        "the parser layers import oxc in only {importing} files"
    );
}

#[test]
fn the_containment_check_sees_code_and_ignores_prose() {
    assert_eq!(
        oxc_paths("use oxc_ast::ast::Expression; // oxc_span::Span in a comment"),
        ["oxc_ast"]
    );
    assert_eq!(
        oxc_paths("let x = oxc_span::Span::default();"),
        ["oxc_span"]
    );
    assert!(oxc_paths("//! the `oxc_resolver`-based resolver").is_empty());
    assert!(oxc_paths("my_oxc_ast::thing()").is_empty());
}

#[test]
fn patterns_bind_every_name_they_spell() {
    let parsed = parsed("const [a, , ...rest] = x\nconst { b = 1, 'c': d, ...others } = y");
    let tree = &parsed.tree;
    let names: Vec<String> = tree
        .nodes()
        .filter(|&node| tree.kind(node) == "VariableDeclarator")
        .flat_map(|declarator| {
            super::bound_symbols(tree, &parsed.scopes, tree.child(declarator, "id").unwrap())
        })
        .map(|symbol| parsed.scopes.symbol(symbol).name.clone())
        .collect();
    assert_eq!(names, ["a", "rest", "b", "d", "others"]);
    let keys: Vec<Option<&str>> = tree
        .nodes()
        .filter(|&node| tree.kind(node) == "Property")
        .map(|property| super::property_name(tree, property))
        .collect();
    assert_eq!(keys, [Some("b"), Some("c")]);
    let computed = parsed_property("const o = { [k]: 1 }");
    assert_eq!(computed, None);
}

/// The static name of the first property `source` spells.
fn parsed_property(source: &str) -> Option<String> {
    let parsed = parsed(source);
    let tree = &parsed.tree;
    let property = tree.nodes().find(|&node| tree.kind(node) == "Property")?;
    super::property_name(tree, property).map(str::to_owned)
}
