//! How many times a collection-scope registration runs, as enclosing loops and iteration
//! callbacks decide it.
use super::Normalizer;
use crate::lang::{Field, NodeId, Tree};
use serde_json::Value;

#[cfg(test)]
thread_local! {
    /// Bodies [`jumps`] has scanned on this test thread.
    pub(super) static JUMP_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Iterations above this make a population unknown rather than materialize its cases.
const MAX_ITERATIONS: u64 = 10_000;

/// How many times a registration runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Population {
    /// Exactly this many, zero included.
    Known(u32),
    /// An enclosing loop or callback whose count the analyzer cannot prove.
    Unknown,
}

impl Normalizer<'_> {
    /// The product of the iteration counts of every loop body and `.forEach`/`.map` callback
    /// between `site` and the registration root, around enclosing suites too.
    pub fn population(&mut self, site: NodeId) -> Population {
        let tree = self.tree;
        let mut total: u64 = 1;
        let mut child = site;
        for ancestor in tree.ancestors(site) {
            if Some(ancestor) == self.registration_root {
                break;
            }
            let count = match tree.kind(ancestor) {
                "ForStatement" | "ForInStatement" | "ForOfStatement" | "WhileStatement"
                | "DoWhileStatement" => match loop_part(tree, ancestor, child) {
                    LoopPart::Body => self.loop_count(ancestor, site),
                    LoopPart::Once => {
                        child = ancestor;
                        continue;
                    }
                    LoopPart::Repeated => None,
                },
                _ if tree.is_function(ancestor) && self.registration_time_callback(ancestor) => {
                    self.callback_count(ancestor, site)
                }
                _ => {
                    child = ancestor;
                    continue;
                }
            };
            let Some(count) = count else {
                return Population::Unknown;
            };
            total = match total.checked_mul(count) {
                Some(total) if total <= MAX_ITERATIONS => total,
                _ => return Population::Unknown,
            };
            child = ancestor;
        }
        Population::Known(u32::try_from(total).unwrap_or(u32::MAX))
    }

    /// One loop statement's iteration count, when its body registers `site` unconditionally.
    fn loop_count(&mut self, statement: NodeId, site: NodeId) -> Option<u64> {
        let tree = self.tree;
        let body = tree.child(statement, "body")?;
        if !self.unconditional(site, body) {
            return None;
        }
        if let Some(&count) = self.loop_counts.get(&statement) {
            return count;
        }
        let count = (!jumps(tree, body))
            .then(|| match tree.kind(statement) {
                "ForOfStatement" if !tree.flag(statement, "await") => tree
                    .child(statement, "right")
                    .and_then(|right| literal_length(tree, right, true)),
                "ForStatement" => self.counted(statement),
                _ => None,
            })
            .flatten();
        self.loop_counts.insert(statement, count);
        count
    }

    /// A `.forEach`/`.map` callback's call count: the receiver literal's present elements.
    fn callback_count(&mut self, function: NodeId, site: NodeId) -> Option<u64> {
        let tree = self.tree;
        let body = tree.child(function, "body")?;
        if !self.unconditional(site, body) {
            return None;
        }
        if let Some(&count) = self.loop_counts.get(&function) {
            return count;
        }
        let call = tree.outer_parent(function)?;
        let callee = tree.inner(tree.child(call, "callee")?);
        let count = (!tree.flag(call, "optional")
            && !tree.flag(callee, "optional")
            && tree
                .list(call, "arguments")
                .first()
                .copied()
                .flatten()
                .is_some_and(|first| tree.inner(first) == function)
            && blind_to_receiver(tree, function)
            && !jumps(tree, body))
        .then(|| {
            tree.child(callee, "object")
                .and_then(|receiver| literal_length(tree, receiver, false))
        })
        .flatten();
        self.loop_counts.insert(function, count);
        count
    }

    /// `for (let i = A; i < B; i++)` with integer literal bounds and a counter only the update
    /// writes. `<=` counts the bound; `++i` is `i++` here. Only `let`: a `var` counter can be
    /// redeclared, and so reassigned, by a declaration that is not a reference.
    fn counted(&self, statement: NodeId) -> Option<u64> {
        let tree = self.tree;
        let init = tree.child(statement, "init")?;
        if tree.kind(init) != "VariableDeclaration" || tree.text(init, "kind") != Some("let") {
            return None;
        }
        let [Some(declarator)] = tree.list(init, "declarations")[..] else {
            return None;
        };
        let counter = tree.child(declarator, "id")?;
        if tree.kind(counter) != "Identifier" {
            return None;
        }
        let start = integer(tree, tree.child(declarator, "init")?)?;
        let test = tree.child(statement, "test")?;
        let inclusive = match (tree.kind(test), tree.text(test, "operator")) {
            ("BinaryExpression", Some("<")) => false,
            ("BinaryExpression", Some("<=")) => true,
            _ => return None,
        };
        let bound = integer(tree, tree.child(test, "right")?)?;
        let update = tree.child(statement, "update")?;
        let symbol = self.scopes.symbol_at(tree.span(counter).start)?;
        let same = |node: NodeId| {
            tree.kind(node) == "Identifier"
                && self.scopes.symbol_at(tree.span(node).start) == Some(symbol)
        };
        if !same(tree.inner(tree.child(test, "left")?))
            || tree.kind(update) != "UpdateExpression"
            || tree.text(update, "operator") != Some("++")
            || !tree
                .child(update, "argument")
                .is_some_and(|argument| same(tree.inner(argument)))
        {
            return None;
        }
        // Only the update writes the counter: not the body, the test, or a closure anywhere.
        let update_span = tree.span(update);
        let binding = tree.span(counter);
        if self
            .scopes
            .symbol(symbol)
            .references
            .iter()
            .any(|reference| {
                reference.write
                    && reference.span != binding
                    && !update_span.contains(reference.span)
            })
        {
            return None;
        }
        let end = if inclusive {
            bound.checked_add(1)?
        } else {
            bound
        };
        u64::try_from(end.saturating_sub(start).max(0)).ok()
    }
}

/// Where a node sits in a loop statement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoopPart {
    /// The body, run once per iteration and counted by [`Normalizer::loop_count`].
    Body,
    /// A part evaluated once each time the loop is entered: a `for` initializer, or the iterable
    /// of a `for...of` or `for...in`.
    Once,
    /// A part evaluated a number of times not proved: a condition, an update, a loop's binding.
    Repeated,
}

/// Which [`LoopPart`] of `statement` its direct child `child` is.
fn loop_part(tree: &Tree, statement: NodeId, child: NodeId) -> LoopPart {
    if tree.child(statement, "body") == Some(child) {
        return LoopPart::Body;
    }
    let once = match tree.kind(statement) {
        "ForStatement" => "init",
        "ForOfStatement" | "ForInStatement" => "right",
        _ => return LoopPart::Repeated,
    };
    if tree.child(statement, once) == Some(child) {
        LoopPart::Once
    } else {
        LoopPart::Repeated
    }
}

/// The element count of an array literal Vitest iterates: `for...of` visits holes, the array
/// methods skip them. A spread hides the count.
fn literal_length(tree: &Tree, node: NodeId, holes: bool) -> Option<u64> {
    let array = tree.inner(node);
    if tree.kind(array) != "ArrayExpression" {
        return None;
    }
    let elements = tree.list(array, "elements");
    if elements
        .iter()
        .flatten()
        .any(|&element| tree.kind(element) == "SpreadElement")
    {
        return None;
    }
    let count = if holes {
        elements.len()
    } else {
        elements.iter().flatten().count()
    };
    u64::try_from(count).ok()
}

/// An integer numeric literal, negated at most once.
fn integer(tree: &Tree, node: NodeId) -> Option<i64> {
    let node = tree.inner(node);
    if tree.kind(node) == "UnaryExpression" && tree.text(node, "operator") == Some("-") {
        return integer(tree, tree.child(node, "argument")?)?.checked_neg();
    }
    if tree.kind(node) != "Literal" {
        return None;
    }
    let Field::Value(Value::Number(number)) = tree.field_of(node, "value") else {
        return None;
    };
    let value = number.as_f64()?;
    (value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0).then_some(value as i64)
}

impl Normalizer<'_> {
    /// Whether `site` runs exactly once each time `body` runs: the path between them is straight
    /// line. Only blocks, expression statements, calls, wrappers, a nested loop's body (counted
    /// on its own) or a part it evaluates once, and the body of a callback that cannot return or
    /// throw early are allowed; any other node on the path (a branch, a parameter default, a
    /// declared function) refuses the count.
    fn unconditional(&mut self, site: NodeId, body: NodeId) -> bool {
        let tree = self.tree;
        // A concise arrow's body is the registration itself.
        if site == body {
            return true;
        }
        let mut child = site;
        // The body itself is on the path: an unbraced `if` or a concise `&&` body is a branch.
        for node in tree.ancestors(site) {
            let straight = match tree.kind(node) {
                "BlockStatement" | "ExpressionStatement" | "ParenthesizedExpression" => true,
                kind if kind.starts_with("TS") && tree.inner(node) != node => true,
                // Callee and arguments both evaluate; an optional call is a `ChainExpression`.
                "CallExpression" => true,
                // [`Normalizer::population`] has already refused a site in a repeated loop part
                // below this one, and counts a nested loop's body itself.
                "ForStatement" | "ForInStatement" | "ForOfStatement" | "WhileStatement"
                | "DoWhileStatement" => true,
                // Only a suite callback (run once by its registration) or an iteration callback
                // (counted on its own); any other callee decides how often a callback runs. A
                // parameter default sits in a pattern, which the last arm refuses.
                "ArrowFunctionExpression" | "FunctionExpression" => {
                    (self.structure.suites.contains_key(&node)
                        || self.registration_time_callback(node))
                        && !*self
                            .early_exits
                            .entry(node)
                            .or_insert_with(|| jumps(tree, child))
                }
                _ => false,
            };
            if !straight {
                return false;
            }
            if node == body {
                return true;
            }
            child = node;
        }
        false
    }
}

/// Whether a `.forEach`/`.map` callback cannot reach the array it iterates, so cannot change how
/// many times it runs: at most two parameters, none a rest element, and no `arguments` read.
fn blind_to_receiver(tree: &Tree, function: NodeId) -> bool {
    let parameters = tree.list(function, "params");
    parameters.len() <= 2
        && parameters
            .iter()
            .flatten()
            .all(|&parameter| tree.kind(parameter) != "RestElement")
        && (tree.kind(function) == "ArrowFunctionExpression"
            || !tree.descendants(function).any(|node| {
                tree.kind(node) == "Identifier" && tree.text(node, "name") == Some("arguments")
            }))
}

/// Whether `body` holds a jump outside its nested functions that could end an iteration early.
fn jumps(tree: &Tree, body: NodeId) -> bool {
    #[cfg(test)]
    JUMP_SCANS.with(|scans| scans.set(scans.get() + 1));
    tree.descendants(body).any(|node| {
        matches!(
            tree.kind(node),
            "BreakStatement" | "ContinueStatement" | "ReturnStatement" | "ThrowStatement"
        ) && !tree
            .ancestors(node)
            .take_while(|&ancestor| ancestor != body)
            .any(|ancestor| tree.is_function(ancestor))
    })
}
