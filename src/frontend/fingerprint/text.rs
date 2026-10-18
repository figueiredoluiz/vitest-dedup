use super::*;

/// Escapes `\`, `<`, and `>` in a literal's raw text so it can never be mistaken for the
/// `<kind:raw>` framing [`Serializer::literal`] wraps it in.
///
/// `\` must be escaped too, not just the two delimiters: without it, a payload ending in a raw
/// `\` immediately followed by an escaped `\<` (from a *different*, adjacent piece of text) would
/// let the backslash pair up with the wrong following character when read back informally, the
/// same ambiguity classic C-string/regex escaping schemes exist to avoid. Escaping all three with
/// the same backslash prefix keeps the scheme unambiguous: after escaping, the only unescaped `<`
/// or `>` bytes in the output are the ones this module inserts itself as real structural
/// delimiters, so no literal's contents can ever imitate the boundary between two serialized
/// nodes. This function does not need to be reversible (nothing ever parses a fingerprint back
/// into a tree -- fingerprints are only ever compared for equality), only injective, which
/// escaping the three special bytes guarantees.
pub(crate) fn escape_literal_delimiters(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '<' => escaped.push_str("\\<"),
            '>' => escaped.push_str("\\>"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Normalizes a JSX text child to the string a renderer actually places in the DOM.
///
/// JSX source text is laid out for readability, not for content: an element written across lines
/// carries `\n  Save\n` as its text child, and the transform strips the whitespace-only lines and
/// the indentation before any of it renders -- which is why this exists. Without it, the *same*
/// element written on one line and across lines serialized differently, and a repository-wide
/// formatter run broke every match between two bodies that render identically (the
/// `duplicate-test-jsx-formatting-only` recall case). The rule is the one Babel's
/// `cleanJSXElementLiteralChild` applies, because that is what decides the rendered text:
///
/// - split on line breaks (`\r\n`, `\r`, `\n` all normalized to `\n` first);
/// - tabs become spaces, then a line's leading spaces are stripped unless it is the first line,
///   and its trailing spaces are stripped unless it is both the last line and the last non-empty
///   line;
/// - surviving non-empty lines are joined with a single space appended to all but the last
///   non-empty one;
/// - a result with no non-empty lines is empty, and an empty text child is not a child at all.
///
/// Known under-match, recorded rather than fixed: HTML entities (`&amp;`) are NOT decoded -- the
/// parser hands JSX text over verbatim, so `&amp;` and `&` stay different texts and two elements
/// using different spellings compare unequal. The safe direction, and vanishingly rare in test
/// bodies.
pub(crate) fn normalize_jsx_text(raw: &str) -> String {
    let normalized_breaks = raw.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized_breaks.split('\n').collect();
    // The last line holding anything other than spaces/tabs; none means the child is whitespace
    // only, which renders nothing.
    let Some(last_non_empty) = lines
        .iter()
        .rposition(|line| line.chars().any(|ch| ch != ' ' && ch != '\t'))
    else {
        return String::new();
    };
    let mut rendered = String::new();
    for (index, line) in lines.iter().enumerate() {
        let is_first = index == 0;
        let is_last = index + 1 == lines.len();
        let is_last_non_empty = index == last_non_empty;
        let mut trimmed = line.replace('\t', " ");
        if !is_first {
            trimmed = trimmed.trim_start_matches(' ').to_owned();
        }
        if !is_last || !is_last_non_empty {
            trimmed = trimmed.trim_end_matches(' ').to_owned();
        }
        if !trimmed.is_empty() {
            rendered.push_str(&trimmed);
            if !is_last_non_empty {
                rendered.push(' ');
            }
        }
    }
    rendered
}

/// Whether `ch` can appear in a TypeScript identifier, for [`normalize_type_argument_text`]'s
/// "keep a separator only where removing it would weld two words together" rule.
fn is_type_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '$'
}

/// Canonicalizes a type-argument list's raw source text so that formatting alone never changes a
/// fingerprint.
///
/// The type-level AST is not modeled by this module (nothing above `src/frontend/` consumes a
/// type), so type arguments are compared as source text -- the same "opaque raw text" bias every
/// other unmodeled construct gets (see the module doc's "Deliberate under-matching"). Raw text
/// alone would be wrong in the *other* direction though: `Array<string>` and `Array< string >`
/// name the same type, and reporting two tests as different because a formatter put a space
/// somewhere is a defect of its own. So whitespace is normalized away, with exactly two carve-outs
/// that keep the normalization from over-matching:
///
/// - A whitespace run is deleted unless it separates two identifier characters, in which case it
///   collapses to a single space. `Record< string , number >` and `Record<string,number>` become
///   the same text, while `keyof T` keeps its space and can never collide with an identifier
///   spelled `keyofT`.
/// - Text inside a string literal type (`'a b'`, `"a b"`, and template literal types) is copied
///   verbatim, escapes included. `Record<'a b', 1>` and `Record<'ab', 1>` are genuinely different
///   types and the whitespace inside the quotes is content, not formatting.
///
/// Comments inside a type argument are deliberately *not* stripped: they are vanishingly rare
/// there, and keeping them only ever makes two type arguments compare as different, which is the
/// safe direction.
pub(crate) fn normalize_type_argument_text(raw: &str) -> String {
    let mut normalized = String::with_capacity(raw.len());
    let mut characters = raw.chars().peekable();
    let mut quote: Option<char> = None;
    let mut pending_separator = false;
    while let Some(ch) = characters.next() {
        if let Some(delimiter) = quote {
            normalized.push(ch);
            if ch == '\\' {
                if let Some(escaped) = characters.next() {
                    normalized.push(escaped);
                }
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if ch.is_whitespace() {
            pending_separator = true;
            continue;
        }
        if pending_separator {
            pending_separator = false;
            let previous_is_word = normalized
                .chars()
                .next_back()
                .is_some_and(is_type_word_char);
            if previous_is_word && is_type_word_char(ch) {
                normalized.push(' ');
            }
        }
        normalized.push(ch);
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    normalized
}

/// Renders an explicit type-argument list as a fingerprint fragment, or the empty string when the
/// construct carries none.
///
/// Returned without a leading separator so that a call, `new`, or tagged template with no type
/// arguments serializes exactly as it did before type arguments were modeled -- the fragment is
/// concatenated straight onto the callee/tag/path, the same way `ser_call` already concatenates
/// its `?.` marker onto `call`.
///
/// The payload goes through [`escape_literal_delimiters`] for the same reason a literal's does:
/// it is arbitrary source text being framed by `<...>`, and unescaped type text could otherwise
/// imitate the delimiter grammar this serializer uses between sibling nodes.
pub(crate) fn type_argument_fragment(
    source: &str,
    type_arguments: Option<&TSTypeParameterInstantiation<'_>>,
) -> String {
    let Some(type_arguments) = type_arguments else {
        return String::new();
    };
    let raw = source
        .get(type_arguments.span.start as usize..type_arguments.span.end as usize)
        .unwrap_or_default();
    format!(
        "<targs:{}>",
        escape_literal_delimiters(&normalize_type_argument_text(raw))
    )
}
