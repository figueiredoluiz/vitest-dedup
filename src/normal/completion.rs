//! Statement completions for suite-body and module statements; the evaluator is the owned tree's
//! (`crate::lang::completes_inertly`), shared with the template reader.

use super::Normalizer;
use crate::lang::NodeId;

impl Normalizer<'_> {
    /// Whether `statement` declares nothing, evaluates only primitive values, and ends normally on
    /// every path: each `return`, `break` and `continue` it reaches is consumed inside it, after
    /// any `finally` that overrides it. Its loops end: a `for...of` over an array literal, or a loop
    /// whose every iteration leaves it.
    pub fn completes_inertly(&self, statement: NodeId) -> bool {
        crate::lang::completes_inertly(self.tree, self.scopes, statement)
    }
}

#[cfg(test)]
mod tests {
    use crate::lang::NodeId;

    /// Whether each labeled statement of `source`, in source order, completes inertly.
    fn labeled(source: &str) -> Vec<bool> {
        crate::testing::project(source, &[], false, |_, parsed, _, normalizer| {
            let tree = &parsed.tree;
            let mut statements: Vec<NodeId> = tree
                .nodes()
                .filter(|&node| {
                    tree.kind(node) == "LabeledStatement"
                        && !tree
                            .parent(node)
                            .is_some_and(|parent| tree.kind(parent) == "LabeledStatement")
                })
                .collect();
            statements.sort_by_key(|&node| tree.span(node).start);
            statements
                .into_iter()
                .map(|statement| normalizer.completes_inertly(statement))
                .collect()
        })
    }

    /// A function declaration is hoisted, so a call above a `const` can run its read first: the
    /// read is proven only outside one.
    #[test]
    fn a_constant_read_in_a_function_declaration_is_not_proven() {
        let source =
            "later()\nconst on = true\nfunction later() { stop: { if (on) break stop } }\n\
                      const after = () => { stop: { if (on) break stop } }\n";
        assert_eq!(labeled(source), [false, true]);
    }
}
