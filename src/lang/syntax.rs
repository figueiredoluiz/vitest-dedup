//! ESTree shapes several layers read: specifier names, property keys, pattern bindings.

use super::{Field, NodeId, Scopes, SymbolId, Tree};

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

/// The structural operation between a callee expression and its immediate input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalleeLinkKind<'t> {
    /// A member read; a dynamic key has no proved name but still has an object and anchor.
    Member(Option<&'t str>),
    /// A call, whose arguments remain available on the original node.
    Call,
    /// A template invocation, distinct from a call with arguments.
    Tagged,
}

/// One borrowed link, retaining the source identity of the expression that owns it.
pub(crate) struct CalleeLink<'t> {
    /// The member, call or tag expression, past transparent wrappers.
    pub node: NodeId,
    /// Its object, callee or tag, before removing that input's transparent wrappers.
    pub input: NodeId,
    /// Syntax only: no framework or binding interpretation.
    pub kind: CalleeLinkKind<'t>,
}

/// One inward callee link. Unknown member names remain structural links; other shapes end a path.
pub(crate) fn callee_link(tree: &Tree, node: NodeId) -> Option<CalleeLink<'_>> {
    let node = tree.inner(node);
    let (field, kind) = match tree.kind(node) {
        "MemberExpression" => ("object", CalleeLinkKind::Member(member_name(tree, node))),
        "CallExpression" => ("callee", CalleeLinkKind::Call),
        "TaggedTemplateExpression" => ("tag", CalleeLinkKind::Tagged),
        _ => return None,
    };
    Some(CalleeLink {
        node,
        input: tree.child(node, field)?,
        kind,
    })
}

/// A direct child's structural role in its parent's value, without interpreting an API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ValueRole<'t> {
    /// Parentheses, `as`, `satisfies` or non-null syntax around the expression.
    Transparent,
    /// An angle-bracket TypeScript assertion around its expression.
    TypeAssertion,
    /// Type arguments applied to a value separately from a call's own type arguments.
    Instantiation,
    /// The input of a member, call or tagged invocation; its result still needs interpretation.
    Callee(CalleeLinkKind<'t>),
    /// The initial value of a binding, without claiming what that binding later does.
    Initializer,
    /// A local/default export operand, without claiming the exported value is inert.
    Exported,
    /// An operand consumed by a unary operation, distinct from other consuming positions.
    UnaryOperand,
    /// An operand whose value can supply the result when that operand is evaluated.
    Result,
    /// A value held in an array, object, property or spread.
    Contained,
    /// A tested left operand; its operator lets the consumer retain its value eligibility policy.
    LogicalLeft(&'t str),
    /// A condition, discarded operand or operation computing a new value from its input.
    Consumed,
    /// No result-flow fact, without claiming the use has no effects or cannot escape.
    Other,
}

/// The role of one actual parent/child edge. Unknown positions establish no result flow.
pub(crate) fn value_role(tree: &Tree, outer: NodeId, inner: NodeId) -> ValueRole<'_> {
    if tree.parent(inner) != Some(outer) {
        return ValueRole::Other;
    }
    let holds = |field| tree.child(outer, field) == Some(inner);
    match tree.kind(outer) {
        "ParenthesizedExpression"
        | "TSAsExpression"
        | "TSSatisfiesExpression"
        | "TSNonNullExpression" => {
            if holds("expression") {
                ValueRole::Transparent
            } else {
                ValueRole::Other
            }
        }
        "TSTypeAssertion" if holds("expression") => ValueRole::TypeAssertion,
        "TSInstantiationExpression" if holds("expression") => ValueRole::Instantiation,
        "VariableDeclarator" if holds("init") => ValueRole::Initializer,
        "ExportSpecifier" | "ExportDefaultDeclaration" => ValueRole::Exported,
        "UnaryExpression" => ValueRole::UnaryOperand,
        "AwaitExpression" if holds("argument") => ValueRole::Result,
        "ArrayExpression" | "ObjectExpression" | "SpreadElement" => ValueRole::Contained,
        "Property" => {
            if holds("value") {
                ValueRole::Contained
            } else {
                ValueRole::Consumed
            }
        }
        "ConditionalExpression" => {
            if holds("consequent") || holds("alternate") {
                ValueRole::Result
            } else {
                ValueRole::Consumed
            }
        }
        "LogicalExpression" if holds("right") => ValueRole::Result,
        "LogicalExpression" if holds("left") => tree
            .text(outer, "operator")
            .map_or(ValueRole::Other, ValueRole::LogicalLeft),
        "SequenceExpression" => {
            if matches!(tree.field_of(outer, "expressions"), Field::List(items)
                if matches!(items.last(), Some(Field::Node(last)) if *last == inner))
            {
                ValueRole::Result
            } else {
                ValueRole::Consumed
            }
        }
        "AssignmentExpression" => {
            if !holds("right") {
                ValueRole::Other
            } else if matches!(
                tree.text(outer, "operator"),
                Some("=" | "||=" | "&&=" | "??=")
            ) {
                ValueRole::Result
            } else {
                ValueRole::Consumed
            }
        }
        "BinaryExpression" | "TemplateLiteral" => ValueRole::Consumed,
        _ => match callee_link(tree, outer) {
            Some(link) if link.node == outer && link.input == inner => ValueRole::Callee(link.kind),
            _ => ValueRole::Other,
        },
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

    /// Call type arguments and a separate instantiation expression have different owned edges.
    #[test]
    fn type_arguments_keep_their_actual_parent_edges() {
        for (expression, kind) in [
            ("held<any>()", "CallExpression"),
            ("(held<any>)()", "TSInstantiationExpression"),
        ] {
            let (parsed, _) = arrow_body(&format!("() => {expression}"));
            assert_eq!(parsed.parse_errors, 0);
            let tree = &parsed.tree;
            let held = tree
                .nodes()
                .find(|&node| {
                    tree.kind(node) == "Identifier" && tree.text(node, "name") == Some("held")
                })
                .unwrap();
            assert_eq!(tree.kind(tree.parent(held).unwrap()), kind, "{expression}");
        }
    }

    /// Roles distinguish the actual result operand from conditions, keys and other inputs.
    #[test]
    fn value_roles_distinguish_result_positions() {
        use ValueRole::{
            Callee, Consumed, Contained, Exported, Initializer, Instantiation, LogicalLeft, Other,
            Result, Transparent, TypeAssertion, UnaryOperand,
        };
        for (expression, expected) in [
            ("(held as any)", Transparent),
            ("held!", Transparent),
            ("<any>held", TypeAssertion),
            ("held<number>", Instantiation),
            (
                "held.member",
                Callee(CalleeLinkKind::Member(Some("member"))),
            ),
            ("held[key]", Callee(CalleeLinkKind::Member(None))),
            ("held()", Callee(CalleeLinkKind::Call)),
            ("held<any>()", Callee(CalleeLinkKind::Call)),
            ("held`text`", Callee(CalleeLinkKind::Tagged)),
            ("await held", Result),
            ("flag ? held : 0", Result),
            ("flag ? 0 : held", Result),
            ("held ? 1 : 0", Consumed),
            ("0 && held", Result),
            ("held && 0", LogicalLeft("&&")),
            ("held || 0", LogicalLeft("||")),
            ("held ?? 0", LogicalLeft("??")),
            ("(0, held)", Result),
            ("(held, 0)", Consumed),
            ("[held]", Contained),
            ("[...held]", Contained),
            ("({ value: held })", Contained),
            ("({ [held]: 0 })", Consumed),
            ("target = held", Result),
            ("target += held", Consumed),
            ("held = 0", Other),
            ("held ||= 0", Other),
            ("typeof held", UnaryOperand),
            ("held === 0", Consumed),
            ("`${held}`", Consumed),
            ("consume(held)", Other),
            ("() => held", Other),
            ("{ const binding = held; }", Initializer),
            ("{}; export default held", Exported),
            ("{}; export { held }", Exported),
        ] {
            let (parsed, _) = arrow_body(&format!("async () => {expression}"));
            assert_eq!(parsed.parse_errors, 0, "{expression}");
            let tree = &parsed.tree;
            let child = tree
                .nodes()
                .find(|&node| {
                    tree.kind(node) == "Identifier" && tree.text(node, "name") == Some("held")
                })
                .unwrap();
            let parent = tree.parent(child).unwrap();
            assert_eq!(value_role(tree, parent, child), expected, "{expression}");
            assert_eq!(value_role(tree, child, parent), Other, "not a child edge");
        }
    }

    /// Each link keeps its original anchor while walking from the outer invocation to the root.
    #[test]
    fn callee_links_keep_order_and_source_identity() {
        let source = "() => root.alpha(1).beta`text`";
        let (parsed, mut node) = arrow_body(source);
        assert_eq!(parsed.parse_errors, 0);
        let tree = &parsed.tree;
        for (anchor, kind) in [
            ("root.alpha(1).beta`text`", CalleeLinkKind::Tagged),
            ("root.alpha(1).beta", CalleeLinkKind::Member(Some("beta"))),
            ("root.alpha(1)", CalleeLinkKind::Call),
            ("root.alpha", CalleeLinkKind::Member(Some("alpha"))),
        ] {
            let link = callee_link(tree, node).expect("a structural link");
            assert_eq!(link.kind, kind);
            let span = tree.span(link.node);
            assert_eq!(&source[span.start as usize..span.end as usize], anchor);
            node = link.input;
        }
        assert!(callee_link(tree, node).is_none());
        assert_eq!(tree.text(node, "name"), Some("root"));
    }

    /// Transparent syntax and static keys do not change the underlying member's identity.
    #[test]
    fn callee_links_unwrap_members_without_deciding_dynamic_names() {
        for (expression, anchor) in [
            ("root.a", "root.a"),
            ("(root.a)", "root.a"),
            ("(root as any).a", "(root as any).a"),
            ("(root!.a satisfies unknown)", "root!.a"),
            ("root['a']", "root['a']"),
            ("root[('a' as const)]", "root[('a' as const)]"),
        ] {
            let source = format!("() => {expression}");
            let (parsed, node) = arrow_body(&source);
            assert_eq!(parsed.parse_errors, 0);
            let tree = &parsed.tree;
            let link = callee_link(tree, node).unwrap();
            assert_eq!(link.kind, CalleeLinkKind::Member(Some("a")));
            let span = tree.span(link.node);
            assert_eq!(&source[span.start as usize..span.end as usize], anchor);
            assert_eq!(tree.text(tree.inner(link.input), "name"), Some("root"));
        }
        for expression in ["root[key]", "root[1]", "root[`a`]"] {
            let (parsed, node) = arrow_body(&format!("() => {expression}"));
            let link = callee_link(&parsed.tree, node).unwrap();
            assert_eq!(link.kind, CalleeLinkKind::Member(None));
            assert_eq!(parsed.tree.text(link.input, "name"), Some("root"));
        }
        for expression in ["root", "flag ? root : other", "new root()"] {
            let (parsed, node) = arrow_body(&format!("() => {expression}"));
            assert!(callee_link(&parsed.tree, node).is_none(), "{expression}");
        }
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
