use super::*;

/// Recognize a literal true, the subject and expected value of the expect(true).toBe(true)
/// stub assertion.
fn is_stub_boolean_true(expr: &Expression<'_>) -> bool {
    matches!(expr, Expression::BooleanLiteral(literal) if literal.value)
}

/// Recognizes `expect(true).toBe(true)`, the canonical no-op assertion used as a placeholder.
fn is_stub_assertion(expr: &Expression<'_>) -> bool {
    let Expression::CallExpression(call) = expr else {
        return false;
    };
    let Expression::StaticMemberExpression(member) = &call.callee else {
        return false;
    };
    if member.property.name.as_str() != "toBe" {
        return false;
    }
    let Expression::CallExpression(inner) = &member.object else {
        return false;
    };
    let Expression::Identifier(ident) = &inner.callee else {
        return false;
    };
    if ident.name.as_str() != "expect" {
        return false;
    }
    let subject_is_true = inner
        .arguments
        .first()
        .and_then(|argument| argument.as_expression())
        .is_some_and(is_stub_boolean_true);
    let expected_is_true = call
        .arguments
        .first()
        .and_then(|argument| argument.as_expression())
        .is_some_and(is_stub_boolean_true);
    subject_is_true && expected_is_true
}

/// Recognizes a stub expression: `null`, `undefined`, `void 0`, or a call to a
/// pending/not-implemented marker (`todo()`, `pending()`, `Promise.resolve()` with no arguments).
/// Unwraps `await`/parens first.
fn is_stub_expression(expr: &Expression<'_>) -> bool {
    let mut current = expr;
    loop {
        match current {
            Expression::AwaitExpression(await_expression) => current = &await_expression.argument,
            Expression::ParenthesizedExpression(parenthesized) => {
                current = &parenthesized.expression
            }
            _ => break,
        }
    }
    match current {
        Expression::NullLiteral(_) => true,
        Expression::Identifier(ident) => ident.name.as_str() == "undefined",
        Expression::UnaryExpression(unary) => {
            unary.operator == UnaryOperator::Void
                && matches!(&unary.argument, Expression::NumericLiteral(literal) if literal.value == 0.0)
        }
        Expression::CallExpression(call) => {
            let name = match &call.callee {
                Expression::Identifier(ident) => Some(ident.name.as_str()),
                Expression::StaticMemberExpression(member) => Some(member.property.name.as_str()),
                _ => None,
            };
            let normalized = name.map(|name| {
                name.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase()
            });
            if matches!(
                normalized.as_deref(),
                Some("pending" | "todo" | "notimplemented")
            ) {
                return true;
            }
            matches!(&call.callee, Expression::StaticMemberExpression(member)
                if matches!(&member.object, Expression::Identifier(id) if id.name.as_str() == "Promise")
                    && member.property.name.as_str() == "resolve")
                && call.arguments.is_empty()
        }
        _ => false,
    }
}

/// True when a body is empty or a recognized no-op stub. Trivial bodies are excluded from
/// comparison upstream, so this is purely a false-positive guard: getting it wrong in the
/// direction of "not trivial" is safe (a real body just gets compared normally), so every branch
/// here requires a specific, narrow shape rather than guessing.
pub(super) fn is_trivial(body: &FunctionLike<'_, '_>) -> bool {
    let statements: &[Statement<'_>] = match body {
        FunctionLike::Arrow(arrow) => match &arrow.body {
            ArrowFunctionBody::FunctionBody(function_body) => &function_body.statements,
            other => {
                return other.as_expression().is_some_and(|expression| {
                    is_stub_expression(expression) || is_stub_assertion(expression)
                });
            }
        },
        FunctionLike::Function(function) => match &function.body {
            Some(function_body) => &function_body.statements,
            // No body at all (an overload signature or `declare function`): nothing to compare.
            None => return true,
        },
    };
    match statements {
        [] => true,
        [Statement::ReturnStatement(return_statement)] => match &return_statement.argument {
            Some(argument) => is_stub_expression(argument) || is_stub_assertion(argument),
            None => true,
        },
        [Statement::ExpressionStatement(expression_statement)] => {
            is_stub_expression(&expression_statement.expression)
                || is_stub_assertion(&expression_statement.expression)
        }
        _ => false,
    }
}
