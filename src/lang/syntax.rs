//! ESTree shapes several layers read: specifier names, property keys, pattern bindings.

use super::{NodeId, Scopes, SymbolId, Tree};

/// Whether a subtree reads its surrounding invocation's receiver or implicit arguments.
/// Arrow functions and computed class keys inherit them; ordinary functions, field initializers
/// and static blocks bind their own.
/// Meta properties are not receiver reads: test callbacks and admitted runner helpers are
/// invoked as plain calls, so `new.target` is absent in those frames. Constructed runners are
/// refused by inventory. `import.meta` is fixed per module; cross-module expansion checks its
/// provenance separately before admitting a helper.
/// `scopes` must be the scopes of `tree`'s module, whose answers it keeps.
pub fn uses_receiver(tree: &Tree, scopes: &Scopes, root: NodeId) -> bool {
    let known = scopes.receivers.borrow().get(&root).copied();
    if let Some(known) = known {
        return known;
    }
    let reads = reads_receiver(tree, scopes, root);
    scopes.receivers.borrow_mut().insert(root, reads);
    reads
}

/// [`uses_receiver`], walked.
fn reads_receiver(tree: &Tree, scopes: &Scopes, root: NodeId) -> bool {
    let binds = |ancestor, read| {
        matches!(
            tree.kind(ancestor),
            "FunctionExpression" | "FunctionDeclaration" | "StaticBlock"
        ) || (matches!(
            tree.kind(ancestor),
            "PropertyDefinition" | "AccessorProperty"
        ) && tree
            .child(ancestor, "value")
            .is_some_and(|value| tree.span(value).contains(tree.span(read))))
    };
    std::iter::once(root)
        .chain(tree.descendants(root))
        .any(|node| {
            let reads = matches!(tree.kind(node), "ThisExpression" | "Super")
                || (tree.kind(node) == "Identifier"
                    && tree.text(node, "name") == Some("arguments")
                    && matches!(
                        scopes.name_at(tree.span(node).start),
                        Some(super::Name::Reference(_))
                    ));
            reads
                && (node == root
                    || !tree
                        .ancestors(node)
                        .take_while(|&ancestor| ancestor != root)
                        .chain(std::iter::once(root))
                        .any(|ancestor| binds(ancestor, node)))
        })
}

/// The name an import or export specifier's `imported`, `local` or `exported` field spells.
pub fn export_name(tree: &Tree, name: NodeId) -> String {
    tree.text(name, "name")
        .or_else(|| tree.text(name, "value"))
        .unwrap_or_default()
        .to_owned()
}

/// The static name of an object pattern or literal property's key.
pub fn property_name(tree: &Tree, property: NodeId) -> Option<&str> {
    let key = tree.child(property, "key")?;
    match tree.kind(key) {
        "Identifier" if !tree.flag(property, "computed") => tree.text(key, "name"),
        "Literal" => tree.text(key, "value"),
        _ => None,
    }
}

/// The static name a member expression reads: `.name`, or a string key in brackets.
pub fn member_name(tree: &Tree, member: NodeId) -> Option<&str> {
    let property = tree.child(member, "property")?;
    if tree.flag(member, "computed") {
        let key = tree.inner(property);
        (tree.kind(key) == "Literal")
            .then(|| tree.text(key, "value"))
            .flatten()
    } else {
        tree.text(property, "name")
    }
}

/// Every symbol a binding pattern declares, in source order.
pub fn bound_symbols(tree: &Tree, scopes: &Scopes, pattern: NodeId) -> Vec<SymbolId> {
    let mut symbols = Vec::new();
    let mut pending = vec![pattern];
    while let Some(node) = pending.pop() {
        match tree.kind(node) {
            "Identifier" => symbols.extend(scopes.symbol_at(tree.span(node).start)),
            "ObjectPattern" => {
                pending.extend(tree.list(node, "properties").into_iter().flatten().rev())
            }
            "ArrayPattern" => {
                pending.extend(tree.list(node, "elements").into_iter().flatten().rev())
            }
            "Property" => pending.extend(tree.child(node, "value")),
            "RestElement" => pending.extend(tree.child(node, "argument")),
            "AssignmentPattern" => pending.extend(tree.child(node, "left")),
            "TSParameterProperty" => pending.extend(tree.child(node, "parameter")),
            _ => {}
        }
    }
    symbols
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::{parse, Parsed};
    use std::path::Path;

    /// The body node of the first arrow function in `source`.
    fn arrow_body(source: &str) -> (Parsed, NodeId) {
        let parsed = parse(Path::new("a.test.ts"), source).expect("parses");
        let tree = &parsed.tree;
        let arrow = tree
            .nodes()
            .find(|&node| tree.kind(node) == "ArrowFunctionExpression")
            .expect("an arrow");
        let body = tree.child(arrow, "body").expect("a body");
        (parsed, body)
    }

    fn body_reads(source: &str) -> bool {
        let (parsed, body) = arrow_body(source);
        uses_receiver(&parsed.tree, &parsed.scopes, body)
    }

    #[test]
    fn import_meta_is_not_a_receiver_read() {
        assert!(!body_reads("() => import.meta.url"));
        assert!(!body_reads("() => ({ env: import.meta.env })"));
    }

    #[test]
    fn new_target_reads_are_not_receiver_reads() {
        // `() => new.target` at module scope does not parse; the factory shape is the valid form.
        let (parsed, body) = arrow_body("function factory() { return () => new.target }");
        assert!(parsed.parse_errors == 0);
        assert!(!uses_receiver(&parsed.tree, &parsed.scopes, body));
    }

    #[test]
    fn new_target_in_a_nested_function_binds_its_own() {
        // The ordinary function between the callback and the read binds its own `new.target`,
        // so the read does not depend on the callback's invocation.
        let source =
            "import { test } from 'vitest'; test('a', () => { function inner() { return new.target } return inner() })";
        let (parsed, body) = arrow_body(source);
        assert!(parsed.parse_errors == 0);
        assert!(!uses_receiver(&parsed.tree, &parsed.scopes, body));
    }

    #[test]
    fn this_and_arguments_remain_receiver_reads() {
        assert!(body_reads("() => this.value"));
        assert!(body_reads("() => arguments.length"));
    }

    /// A kept answer is the walk's answer for that root, whichever roots were asked first.
    #[test]
    fn kept_answers_match_a_fresh_walk_for_every_root() {
        let source = "const a = () => this.value;\n\
                      const b = () => { function own() { return this } return own };\n\
                      const c = () => arguments.length;\n\
                      const d = (arguments) => arguments;\n\
                      class E extends Object { f = this.g; static { this.h = 1 } [this.k]() { return super.m() } }\n\
                      const g = () => { const h = () => this.i; return h };\n";
        let parsed = parse(Path::new("a.test.ts"), source).expect("parses");
        let (tree, scopes) = (&parsed.tree, &parsed.scopes);
        let nodes: Vec<_> = tree.nodes().collect();
        let walked: Vec<_> = nodes
            .iter()
            .map(|&node| reads_receiver(tree, scopes, node))
            .collect();
        assert!(walked.contains(&true) && walked.contains(&false));
        for order in [nodes.clone(), nodes.iter().rev().copied().collect()] {
            for &node in &order {
                let index = nodes.iter().position(|&n| n == node).unwrap();
                assert_eq!(uses_receiver(tree, scopes, node), walked[index]);
                assert_eq!(uses_receiver(tree, scopes, node), walked[index]);
            }
        }
        assert_eq!(scopes.receivers.borrow().len(), nodes.len());
    }

    /// A root asked before is answered from what was kept, without walking its subtree again.
    #[test]
    fn a_kept_answer_is_not_walked_again() {
        let (parsed, body) = arrow_body("() => this.value");
        let (tree, scopes) = (&parsed.tree, &parsed.scopes);
        assert!(uses_receiver(tree, scopes, body));
        scopes.receivers.borrow_mut().insert(body, false);
        assert!(!uses_receiver(tree, scopes, body));
    }
}
