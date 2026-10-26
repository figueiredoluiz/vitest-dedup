//! The text of a JavaScript string value.
//!
//! oxc stores string literal values and template cooked text as WTF-8, which can hold lone
//! surrogates that a Rust `str` cannot. Every reader of those values goes through this module, so
//! all of them agree on one text: a value without a lone surrogate is its UTF-8, and a value with one
//! is spelled as earlier oxc releases spelled it, each lone surrogate as `\u{FFFD}` followed by its
//! code unit in four lowercase hex digits, and each literal U+FFFD as `\u{FFFD}fffd`. Titles,
//! fingerprints and specifiers therefore stay exactly what they were.

use oxc_ast::ast::{StringLiteral, TemplateElementValue};
use std::borrow::Cow;
use std::fmt::Write as _;

/// Spells a WTF-8 value as text (see the module documentation). One macro so both entry points
/// share the encoding without naming `oxc_str`'s type, which no direct dependency exports.
macro_rules! wtf8_text {
    ($value:expr) => {{
        let value = $value;
        match value.as_str() {
            Some(text) => Cow::Borrowed(text),
            None => {
                let mut text = String::with_capacity(value.len());
                for code_point in value.chars() {
                    match code_point.to_char() {
                        Some('\u{FFFD}') => text.push_str("\u{FFFD}fffd"),
                        Some(character) => text.push(character),
                        None => {
                            let _ = write!(text, "\u{FFFD}{:04x}", code_point.to_u32());
                        }
                    }
                }
                Cow::Owned(text)
            }
        }
    }};
}

/// The text of a string literal's value.
pub(crate) fn string_text<'a>(literal: &StringLiteral<'a>) -> Cow<'a, str> {
    wtf8_text!(literal.value)
}

/// The text of a template element's cooked value, or `None` when the element has an invalid
/// escape sequence (and so no cooked value).
pub(crate) fn cooked_text<'a>(value: &TemplateElementValue<'a>) -> Option<Cow<'a, str>> {
    value.cooked.map(|cooked| wtf8_text!(cooked))
}

#[cfg(test)]
mod tests {
    use super::{cooked_text, string_text};
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

    fn string(source: &str) -> (String, bool) {
        with_expression(source, |expression| {
            let Expression::StringLiteral(literal) = expression else {
                panic!("{source}: not a string literal");
            };
            let text = string_text(literal);
            (text.to_string(), matches!(text, Cow::Borrowed(_)))
        })
    }

    fn cooked(source: &str) -> Option<String> {
        with_expression(source, |expression| {
            let template = match expression {
                Expression::TemplateLiteral(template) => template,
                Expression::TaggedTemplateExpression(tagged) => &tagged.quasi,
                _ => panic!("{source}: not a template"),
            };
            cooked_text(&template.quasis[0].value).map(Cow::into_owned)
        })
    }

    #[test]
    /// Text without a lone surrogate is the value itself, borrowed, U+FFFD included.
    fn plain_text_is_borrowed_unchanged() {
        assert_eq!(string("'plain'"), ("plain".to_owned(), true));
        assert_eq!(string("'\\uFFFD'"), ("\u{FFFD}".to_owned(), true));
        assert_eq!(string("'\\uD83D\\uDE00'"), ("\u{1F600}".to_owned(), true));
    }

    #[test]
    /// A lone surrogate is spelled `\u{FFFD}` plus its code unit in lowercase hex, and in such a
    /// string a literal U+FFFD is spelled `\u{FFFD}fffd`, as oxc 0.154 spelled both.
    fn lone_surrogates_keep_the_earlier_spelling() {
        assert_eq!(string("'\\uD800'"), ("\u{FFFD}d800".to_owned(), false));
        assert_eq!(string("'a\\uDC01b'"), ("a\u{FFFD}dc01b".to_owned(), false));
        assert_eq!(
            string("'\\uD800\\uFFFD'"),
            ("\u{FFFD}d800\u{FFFD}fffd".to_owned(), false)
        );
        assert_ne!(string("'\\uD800'").0, string("'\\uD801'").0);
    }

    #[test]
    /// Template text follows the same spelling, and an invalid escape still has no cooked value.
    fn template_cooked_text_follows_the_same_rule() {
        assert_eq!(cooked("`x`"), Some("x".to_owned()));
        assert_eq!(cooked("`\\uDC00x`"), Some("\u{FFFD}dc00x".to_owned()));
        assert_eq!(cooked("tag`\\unicode`"), None);
    }
}
