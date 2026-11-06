//! The text of a JavaScript string value.
//!
//! oxc stores string literal values and template cooked text as WTF-8, which can hold lone
//! surrogates that a Rust `str` cannot. Every reader of those values goes through this module, so
//! all of them use the lossless representation in [`super::text`].

use oxc_ast::ast::StringLiteral;
use std::borrow::Cow;

/// The text of a string literal's value.
pub(crate) fn string_text<'a>(literal: &StringLiteral<'a>) -> Cow<'a, str> {
    match literal.value.as_str() {
        Some(value) => super::text::from_utf8(value),
        None => Cow::Owned(super::text::from_utf16(literal.value.encode_utf16())),
    }
}

#[cfg(test)]
mod tests {
    use super::string_text;
    use oxc_allocator::Allocator;
    use oxc_ast::ast::{Expression, Statement};
    use oxc_parser::Parser;
    use oxc_span::SourceType;
    use std::borrow::Cow;

    /// The expression `source` spells, passed as the argument of a call so a lone string is not
    /// read as a directive, handed to `read`.
    fn with_expression<T>(source: &str, read: impl FnOnce(&Expression) -> T) -> T {
        let allocator = Allocator::default();
        let wrapped = format!("f({source})");
        let parsed = Parser::new(&allocator, &wrapped, SourceType::mjs()).parse();
        assert!(
            parsed.diagnostics.is_empty(),
            "{source}: {:?}",
            parsed.diagnostics
        );
        let Some(Statement::ExpressionStatement(statement)) = parsed.program.body.first() else {
            panic!("{source}: not one expression statement");
        };
        let Expression::CallExpression(call) = &statement.expression else {
            panic!("{source}: not a call");
        };
        let Some(argument) = call
            .arguments
            .first()
            .and_then(|argument| argument.as_expression())
        else {
            panic!("{source}: no argument");
        };
        read(argument)
    }

    /// The text of the string literal `source` spells, and whether it was borrowed unchanged.
    fn string(source: &str) -> (String, bool) {
        with_expression(source, |expression| {
            let Expression::StringLiteral(literal) = expression else {
                panic!("{source}: not a string literal");
            };
            let text = string_text(literal);
            (text.to_string(), matches!(text, Cow::Borrowed(_)))
        })
    }

    #[test]
    /// Plain text is borrowed; the encoding introducer is always escaped.
    fn plain_text_is_borrowed_unchanged() {
        assert_eq!(string("'plain'"), ("plain".to_owned(), true));
        assert_eq!(string("'\\uFFFD'"), ("\u{FFFD}fffd".to_owned(), false));
        assert_eq!(string("'\\uD83D\\uDE00'"), ("\u{1F600}".to_owned(), true));
    }

    #[test]
    /// Lone surrogate tokens preserve their code unit and cannot consume a literal introducer.
    fn lone_surrogates_keep_their_code_units() {
        assert_eq!(string("'\\uD800'"), ("\u{FFFD}d800".to_owned(), false));
        assert_eq!(string("'a\\uDC01b'"), ("a\u{FFFD}dc01b".to_owned(), false));
        assert_eq!(
            string("'\\uD800\\uFFFD'"),
            ("\u{FFFD}d800\u{FFFD}fffd".to_owned(), false)
        );
        assert_ne!(string("'\\uD800'").0, string("'\\uD801'").0);
    }
}
