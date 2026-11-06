//! Statement completions: whether a statement runs no code and ends where it started, so it sets
//! nothing up for the code around it.

use super::{Declaration, Field, NodeId, Scopes, Tree};
use std::collections::BTreeSet;

/// Nesting the evaluator follows before it gives up; deeper statements are not inert.
const MAX_DEPTH: u32 = 64;

/// How evaluating a statement can end.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Completion {
    /// It falls through to the next statement.
    Normal,
    /// A bare `return` leaves the enclosing function.
    Return,
    /// `break`, with its label when it has one.
    Break(Option<String>),
    /// `continue`, with its label when it has one.
    Continue(Option<String>),
}

/// Every way a statement can end, or `None` when it may run code or the evaluator cannot tell.
type Completions = Option<BTreeSet<Completion>>;

/// One completion.
fn only(completion: Completion) -> Completions {
    Some(BTreeSet::from([completion]))
}

/// Whether `statement` declares nothing, evaluates only primitive values, and ends normally on
/// every path: each `return`, `break` and `continue` it reaches is consumed inside it, after any
/// `finally` that overrides it, and each loop ends.
pub fn completes_inertly(tree: &Tree, scopes: &Scopes, statement: NodeId) -> bool {
    Evaluator { tree, scopes }.completes_inertly(statement)
}

/// Reads completions over one module's tree and bindings.
struct Evaluator<'a> {
    tree: &'a Tree,
    scopes: &'a Scopes,
}

impl Evaluator<'_> {
    /// Whether `statement` declares nothing, evaluates only primitive values, and ends normally on
    /// every path: each `return`, `break` and `continue` it reaches is consumed inside it, after
    /// any `finally` that overrides it. Its loops end: a `for...of` over an array literal, or a loop
    /// whose every iteration leaves it.
    fn completes_inertly(&self, statement: NodeId) -> bool {
        !self.declares(statement)
            && self
                .completions(statement, &[], 0)
                .is_some_and(|completions| completions == BTreeSet::from([Completion::Normal]))
    }

    /// Whether `statement` holds a declaration anywhere, reachable or not: hoisting binds a `var`
    /// or a function in the enclosing scope whether or not it runs. A `for...of` binding is the
    /// loop's own, checked where the loop is read.
    fn declares(&self, statement: NodeId) -> bool {
        let tree = self.tree;
        std::iter::once(statement)
            .chain(tree.descendants(statement))
            .any(|node| {
                tree.kind(node).ends_with("Declaration")
                    && !tree.parent(node).is_some_and(|parent| {
                        tree.kind(parent) == "ForOfStatement"
                            && tree.child(parent, "left") == Some(node)
                    })
            })
    }

    /// The completions of `statement`; `labels` are the labels directly on it.
    fn completions(&self, statement: NodeId, labels: &[String], depth: u32) -> Completions {
        let tree = self.tree;
        if depth > MAX_DEPTH {
            return None;
        }
        let depth = depth + 1;
        match tree.kind(statement) {
            "EmptyStatement" => only(Completion::Normal),
            "ExpressionStatement" => self
                .primitive_read(tree.child(statement, "expression")?)
                .then_some(BTreeSet::from([Completion::Normal])),
            "BlockStatement" => self.sequence(&tree.list(statement, "body"), depth),
            "BreakStatement" => only(Completion::Break(self.label(statement))),
            "ContinueStatement" => only(Completion::Continue(self.label(statement))),
            "ReturnStatement" if tree.child(statement, "argument").is_none() => {
                only(Completion::Return)
            }
            "LabeledStatement" => {
                let mut labels = labels.to_vec();
                labels.push(self.label(statement)?);
                let body = tree.child(statement, "body")?;
                let inner = self.completions(body, &labels, depth)?;
                let own = labels.last().cloned();
                Some(
                    inner
                        .into_iter()
                        .map(|completion| match completion {
                            Completion::Break(label) if label.is_some() && label == own => {
                                Completion::Normal
                            }
                            completion => completion,
                        })
                        .collect(),
                )
            }
            "IfStatement" => {
                if !self.primitive_read(tree.child(statement, "test")?) {
                    return None;
                }
                let mut out = self.completions(tree.child(statement, "consequent")?, &[], depth)?;
                match tree.child(statement, "alternate") {
                    Some(alternate) => out.extend(self.completions(alternate, &[], depth)?),
                    None => {
                        out.insert(Completion::Normal);
                    }
                }
                Some(out)
            }
            "SwitchStatement" => self.switch(statement, depth),
            "TryStatement" => {
                // A handler would run only if the block threw, and the block evaluates nothing
                // that throws; it is refused rather than reasoned about.
                if tree.child(statement, "handler").is_some() {
                    return None;
                }
                let block = self.completions(tree.child(statement, "block")?, &[], depth)?;
                let Some(finalizer) = tree.child(statement, "finalizer") else {
                    return Some(block);
                };
                let mut finally = self.completions(finalizer, &[], depth)?;
                // A finalizer that ends normally lets the block's completion stand; one that ends
                // abruptly replaces it.
                if finally.remove(&Completion::Normal) {
                    finally.extend(block);
                }
                Some(finally)
            }
            "WhileStatement" => {
                if !self.primitive_read(tree.child(statement, "test")?) {
                    return None;
                }
                self.r#loop(tree.child(statement, "body")?, labels, false, depth)
            }
            // `for (;;)` and `for (; x;)` iterate like `while`: they end only if no iteration
            // reaches the next, or the condition is read as primitive and the body leaves.
            "ForStatement" => {
                if tree.child(statement, "init").is_some()
                    || tree.child(statement, "update").is_some()
                {
                    return None;
                }
                if let Some(test) = tree.child(statement, "test") {
                    if !self.primitive_read(test) {
                        return None;
                    }
                }
                self.r#loop(tree.child(statement, "body")?, labels, false, depth)
            }
            "ForOfStatement" => {
                if tree.flag(statement, "await")
                    || !self.fresh_binding(tree.child(statement, "left")?)
                {
                    return None;
                }
                let right = tree.inner(tree.child(statement, "right")?);
                let finite = tree.kind(right) == "ArrayExpression"
                    && tree
                        .list(right, "elements")
                        .into_iter()
                        .flatten()
                        .all(|element| self.primitive_read(element));
                if !finite {
                    return None;
                }
                self.r#loop(tree.child(statement, "body")?, labels, true, depth)
            }
            _ => None,
        }
    }

    /// The completions of a statement list run in order. A statement after one that cannot end
    /// normally never runs.
    fn sequence(&self, statements: &[Option<NodeId>], depth: u32) -> Completions {
        let mut out = BTreeSet::new();
        let mut reachable = true;
        for statement in statements {
            let statement = (*statement)?;
            if !reachable {
                continue;
            }
            let mut completions = self.completions(statement, &[], depth)?;
            reachable = completions.remove(&Completion::Normal);
            out.extend(completions);
        }
        if reachable {
            out.insert(Completion::Normal);
        }
        Some(out)
    }

    /// A `switch` over primitives: control enters at any case, or at none when there is no
    /// `default`, and falls through the cases after it. An unlabeled `break` ends the switch.
    fn switch(&self, statement: NodeId, depth: u32) -> Completions {
        let tree = self.tree;
        if !self.primitive_read(tree.child(statement, "discriminant")?) {
            return None;
        }
        let cases: Vec<NodeId> = tree
            .list(statement, "cases")
            .into_iter()
            .collect::<Option<_>>()?;
        let mut defaulted = false;
        for &case in &cases {
            match tree.child(case, "test") {
                Some(test) if !self.primitive_read(test) => return None,
                Some(_) => {}
                None => defaulted = true,
            }
        }
        let mut out = BTreeSet::new();
        if !defaulted {
            out.insert(Completion::Normal);
        }
        // Entering at a case runs it and, when it ends normally, falls into the next: each case
        // is read once, from the last back, so nested switches cost their size.
        let mut after = BTreeSet::from([Completion::Normal]);
        for &case in cases.iter().rev() {
            let mut entered = self.sequence(&tree.list(case, "consequent"), depth)?;
            if entered.remove(&Completion::Normal) {
                entered.extend(after.iter().cloned());
            }
            out.extend(entered.iter().cloned());
            after = entered;
        }
        Some(
            out.into_iter()
                .map(|completion| match completion {
                    Completion::Break(None) => Completion::Normal,
                    completion => completion,
                })
                .collect(),
        )
    }

    /// A loop body's completions, as the loop's own. An unlabeled `break` or one naming the loop
    /// leaves it; normal completion and a `continue` it consumes start the next iteration, so the
    /// loop ends only if it is `finite` or no iteration can reach the next.
    fn r#loop(&self, body: NodeId, labels: &[String], finite: bool, depth: u32) -> Completions {
        let ours =
            |label: &Option<String>| label.as_ref().is_none_or(|label| labels.contains(label));
        let mut out = BTreeSet::from([Completion::Normal]);
        let mut repeats = false;
        for completion in self.completions(body, &[], depth)? {
            match completion {
                Completion::Break(label) if ours(&label) => {}
                Completion::Continue(label) if ours(&label) => repeats = true,
                Completion::Normal => repeats = true,
                completion => {
                    out.insert(completion);
                }
            }
        }
        (finite || !repeats).then_some(out)
    }

    /// Whether `left` of a `for...of` declares one fresh block-scoped name.
    fn fresh_binding(&self, left: NodeId) -> bool {
        let tree = self.tree;
        tree.kind(left) == "VariableDeclaration"
            && matches!(tree.text(left, "kind"), Some("const" | "let"))
            && matches!(tree.list(left, "declarations").as_slice(), [Some(declarator)]
                if tree.child(*declarator, "init").is_none()
                    && tree.child(*declarator, "id").is_some_and(|id| tree.kind(id) == "Identifier"))
    }

    /// Whether evaluating `node` reads only a primitive: a literal, or a `const` initialized to one
    /// that has run before `node` does, so the read neither calls code nor meets its temporal dead
    /// zone.
    fn primitive_read(&self, node: NodeId) -> bool {
        let tree = self.tree;
        let node = tree.inner(node);
        match tree.kind(node) {
            "Literal" => primitive(tree, node),
            "Identifier" => {
                let Some(symbol) = self.scopes.symbol_at(tree.span(node).start) else {
                    return false;
                };
                let Declaration::Variable(declarator) = self.scopes.symbol(symbol).declaration
                else {
                    return false;
                };
                // `declare const` is erased: the read reaches a global, which may be a getter.
                tree.parent(declarator).is_some_and(|declaration| {
                    tree.text(declaration, "kind") == Some("const")
                        && !tree.erased_declaration(declaration)
                        && self.initialized_before(declaration, node)
                }) && tree
                    .child(declarator, "id")
                    .is_some_and(|id| tree.kind(id) == "Identifier")
                    && tree
                        .child(declarator, "init")
                        .is_some_and(|init| primitive(tree, tree.inner(init)))
            }
            _ => false,
        }
    }

    /// Whether `declaration` has run whenever `read` does: `read` sits in a later statement of the
    /// list holding `declaration`, and never in a function declaration, which hoisting can call
    /// first. Each `switch` case is a list of its own, so a later case, which control can enter
    /// directly, never counts.
    fn initialized_before(&self, declaration: NodeId, read: NodeId) -> bool {
        let tree = self.tree;
        let Some(body) = tree.parent(declaration) else {
            return false;
        };
        !tree
            .ancestors(read)
            .any(|ancestor| tree.kind(ancestor) == "FunctionDeclaration")
            && tree.ancestors(read).any(|statement| {
                tree.parent(statement) == Some(body)
                    && tree.span(statement).start >= tree.span(declaration).end
            })
    }

    /// The label a labeled statement, `break` or `continue` names.
    fn label(&self, statement: NodeId) -> Option<String> {
        let tree = self.tree;
        tree.child(statement, "label")
            .and_then(|label| tree.text(label, "name"))
            .map(str::to_owned)
    }
}

/// Primitive values only; regular expressions allocate.
pub(crate) fn primitive(tree: &Tree, node: NodeId) -> bool {
    tree.kind(node) == "Literal" && !matches!(tree.field_of(node, "regex"), Field::Object(_))
}
