//! Compile-spike proving the pinned oxc 0.149.0 API shapes the frontend will depend on.
//! Verifies: parse, semantic build, Visit trait, string-literal extraction, static member chains.
use oxc_allocator::Allocator;
use oxc_ast::ast::{Argument, CallExpression, Expression, Program};
use oxc_ast_visit::{walk, Visit};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use std::path::Path;

#[derive(Default)]
struct Collector {
    calls: Vec<(String, Option<String>)>,
}

/// Flattens a callee into a dotted path: `test.each` -> "test.each".
fn callee_path(expression: &Expression<'_>) -> Option<String> {
    match expression {
        Expression::Identifier(identifier) => Some(identifier.name.as_str().to_owned()),
        Expression::StaticMemberExpression(member) => {
            let object = callee_path(&member.object)?;
            Some(format!("{object}.{}", member.property.name.as_str()))
        }
        _ => None,
    }
}

/// Recovers a statically known string argument.
fn static_string(argument: &Argument<'_>) -> Option<String> {
    match argument {
        // A value holding a lone surrogate has no UTF-8 text; the spike treats it as unknown.
        Argument::StringLiteral(literal) => literal.value.as_str().map(str::to_owned),
        Argument::TemplateLiteral(template) if template.expressions.is_empty() => template
            .quasis
            .first()
            .and_then(|quasi| quasi.value.cooked)
            .and_then(|cooked| cooked.as_str())
            .map(str::to_owned),
        _ => None,
    }
}

impl<'a> Visit<'a> for Collector {
    fn visit_call_expression(&mut self, it: &CallExpression<'a>) {
        if let Some(path) = callee_path(&it.callee) {
            let title = it.arguments.first().and_then(static_string);
            self.calls.push((path, title));
        }
        walk::walk_call_expression(self, it);
    }
}

fn main() {
    let source = r#"
import { describe, test, it } from "vitest";
describe("math", () => {
  test("adds", () => { expect(1 + 1).toBe(2); });
  it.skip(`subtracts`, () => { expect(2 - 1).toBe(1); });
  test.each([[1, 2]])("case %i", (a, b) => { expect(a).toBeLessThan(b); });
});
"#;
    let allocator = Allocator::new();
    let source_type = SourceType::from_path(Path::new("demo.test.ts")).unwrap();
    let parsed = Parser::new(&allocator, source, source_type).parse();
    println!("fatal_error: {:?}", parsed.fatal_error);
    println!("diagnostics: {}", parsed.diagnostics.len());

    let program: &Program = &parsed.program;
    let semantic = SemanticBuilder::new().build(program);
    println!("semantic errors: {}", semantic.diagnostics.len());

    let scoping = semantic.semantic.scoping();
    let unresolved: Vec<_> = scoping
        .root_unresolved_references()
        .iter()
        .map(|(name, _)| name.to_string())
        .collect();
    println!("unresolved(globals) at root: {unresolved:?}");

    let mut collector = Collector::default();
    collector.visit_program(program);
    for (path, title) in &collector.calls {
        println!("call {path:<12} title={title:?}");
    }
}
