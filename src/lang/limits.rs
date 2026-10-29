//! Limits checked before a source reaches the parser, and the stack the parser runs on.
//!
//! oxc recurses on nesting, and a stack overflow aborts the process rather than unwinding, so
//! every parser entry, in both cores, refuses a source past these bounds first.

/// Explicit stack headroom for parsing, semantic construction, AST visitors and helper frames.
/// The independent raw syntax budget below is essential: this stack is finite, not crash recovery.
pub(crate) const EXTRACTION_STACK_BYTES: usize = 128 * 1024 * 1024;

/// Maximum lexical bracket depth, retaining the existing tighter limit on ordinary source.
/// This scan is an admission heuristic, not a safety bound: regex/JSX can confuse its masking.
pub(crate) const MAX_NESTING_DEPTH: usize = 256;

/// Independent upper bound on recursion-bearing raw syntax units. It deliberately also counts
/// text in strings/comments/regexes, so lexical ambiguity cannot hide recursive syntax. Wide or
/// punctuation-heavy generated inputs may be refused even when their actual syntax is shallow.
///
/// Set three times below the shallowest chain that overflows [`EXTRACTION_STACK_BYTES`] on a debug
/// build, the larger frames of the two: an assignment chain overflows from about 114,800 units
/// (release from about 245,000). At this value every chain class completes, and the slowest, a
/// call chain, which the parser handles in quadratic time, takes a few seconds.
pub(crate) const MAX_RAW_SYNTAX_UNITS: usize = 36_864;

/// Checks every production parser entry before Oxc sees source. No JS tokenization or recursion.
/// Keywords cover bracket-free unary/control/type chains; punctuation covers calls, member and
/// label chains, arrows, JSX, operators, nested templates and bracket nesting. Commas, digits and
/// ordinary identifier text are uncharged, retaining flat data without pretending to parse JS.
pub(crate) fn parse_refusal(source: &str) -> Option<String> {
    if source.len() > crate::discovery::MAX_SOURCE_FILE_BYTES as usize {
        return Some(
            "file exceeds the maximum supported source size; skipped before parsing".to_owned(),
        );
    }
    let bytes = source.as_bytes();
    let mut units = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || matches!(bytes[i], b'_' | b'$') {
            let start = i;
            i += 1;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'$'))
            {
                i += 1;
            }
            if matches!(
                &bytes[start..i],
                b"if"
                    | b"else"
                    | b"for"
                    | b"while"
                    | b"do"
                    | b"new"
                    | b"void"
                    | b"typeof"
                    | b"delete"
                    | b"await"
                    | b"yield"
                    | b"return"
                    | b"throw"
                    | b"in"
                    | b"instanceof"
                    | b"as"
                    | b"satisfies"
                    | b"keyof"
                    | b"readonly"
                    | b"infer"
                    | b"unique"
                    | b"extends"
                    | b"declare"
            ) {
                units += 1;
            }
        } else {
            if matches!(
                bytes[i],
                b'(' | b'['
                    | b'{'
                    | b'<'
                    | b'>'
                    | b'='
                    | b'.'
                    | b':'
                    | b'?'
                    | b'!'
                    | b'+'
                    | b'-'
                    | b'*'
                    | b'/'
                    | b'%'
                    | b'&'
                    | b'|'
                    | b'^'
                    | b'~'
                    | b'`'
                    | b'@'
                    | b'\\'
            ) {
                units += 1;
            }
            i += 1;
        }
        if units > MAX_RAW_SYNTAX_UNITS {
            return Some(format!("file exceeds the maximum supported raw syntax budget ({MAX_RAW_SYNTAX_UNITS} units); skipped before parsing to avoid a stack overflow"));
        }
    }
    let depth = max_nesting_depth(source);
    (depth > MAX_NESTING_DEPTH).then(|| format!(
        "file exceeds the maximum supported bracket nesting depth ({depth} > {MAX_NESTING_DEPTH}); skipped without parsing to avoid a parser stack overflow"
    ))
}

/// Estimates bracket nesting with a lexical masking heuristic, without parsing `source`.
///
/// A naive byte-level scan counting `(`/`[`/`{` against `)`/`]`/`}` cannot tell a real bracket
/// from one that is only *text* -- inside a string literal, a comment, or a template literal's
/// literal-text portions -- and decrementing on one of those is unsound: a string containing a
/// bare `]` (or any other closer) drags the running counter down without a matching real open,
/// so genuinely deep nesting that follows can under-count all the way back below the limit. That
/// is not merely imprecise, it defeats the one purpose this guard exists for (see
/// [`MAX_NESTING_DEPTH`]'s doc) -- oxc's stack overflow on deep nesting aborts the whole process
/// and is not something `catch_unwind` can recover from.
///
/// So this scan masks the same three regions `crate::vitest_config`'s `mask_non_code` does (read
/// for the approach; this is a separate, self-contained implementation, not a shared one, per
/// the parser-containment boundary): line comments, block comments, `'`/`"` strings,
/// and a template literal's literal-text portions, while still counting brackets that are real
/// code, including a template literal's `${ ... }` holes, recursively. *Opening* brackets found
/// inside one of those regions are masked too, not just closing ones -- the goal is to never let
/// non-code content move the counter in either direction, since a mask that only suppressed
/// decrements while still counting opens would let a string inflate `depth` past the limit and
/// reject a file that never actually nests that deep (a false rejection, which is safe, but an
/// unnecessary one this scan can just as easily avoid).
///
/// For actual quoted text, an unterminated string masks to the end of its line; an unterminated
/// block comment or template masks to EOF. Regex literals and JSX are not recognized, however,
/// so this heuristic can undercount. The independent raw syntax budget in [`parse_refusal`]
/// bounds admission even when masking mistakes code for text.
pub(crate) fn max_nesting_depth(source: &str) -> usize {
    /// One entry of the template-literal context stack.
    enum Frame {
        /// Between a template literal's backticks and its next `${` (or closing backtick):
        /// literal text, which can never contain a real bracket.
        TemplateText,
        /// Inside a `${ ... }` hole, which is ordinary code. Carries the brace depth already
        /// opened within this specific hole (starting at `0` right after `${` is seen), so the
        /// hole's own closing `}` can be told apart from one belonging to a nested literal inside
        /// it.
        Hole(u32),
    }

    let bytes = source.as_bytes();
    let n = bytes.len();
    let mut depth: usize = 0;
    let mut max_depth: usize = 0;
    let mut frames: Vec<Frame> = Vec::new();
    let mut i = 0usize;

    while i < n {
        if matches!(frames.last(), Some(Frame::TemplateText)) {
            match bytes[i] {
                b'\\' if i + 1 < n => i += 2,
                b'`' => {
                    frames.pop();
                    i += 1;
                }
                b'$' if i + 1 < n && bytes[i + 1] == b'{' => {
                    // Mask only the `$`; the `{` is real code and is left for the ordinary
                    // `b'(' | b'[' | b'{'` arm below to count on the next iteration -- consuming
                    // it here too would double-count it.
                    frames.push(Frame::Hole(0));
                    i += 1;
                }
                _ => i += 1,
            }
            continue;
        }

        // Ordinary code: top level, or inside a `${ ... }` hole.
        match bytes[i] {
            b'/' if i + 1 < n && bytes[i + 1] == b'/' => {
                i += 2;
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < n && bytes[i + 1] == b'*' => {
                i += 2;
                while i < n && !(bytes[i] == b'*' && i + 1 < n && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = if i < n { i + 2 } else { n };
            }
            quote @ (b'\'' | b'"') => {
                i += 1;
                while i < n {
                    if bytes[i] == b'\\' && i + 1 < n {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        break;
                    }
                    if bytes[i] == b'\n' {
                        // An unescaped newline never legally appears inside a single/double-
                        // quoted JS string; resume ordinary scanning right here rather than mask
                        // further (possibly real) content.
                        break;
                    }
                    i += 1;
                }
            }
            b'`' => {
                frames.push(Frame::TemplateText);
                i += 1;
            }
            open @ (b'(' | b'[' | b'{') => {
                depth += 1;
                max_depth = max_depth.max(depth);
                if open == b'{' {
                    if let Some(Frame::Hole(hole_depth)) = frames.last_mut() {
                        *hole_depth += 1;
                    }
                }
                i += 1;
            }
            close @ (b')' | b']' | b'}') => {
                depth = depth.saturating_sub(1);
                if close == b'}' {
                    if let Some(Frame::Hole(hole_depth)) = frames.last_mut() {
                        *hole_depth = hole_depth.saturating_sub(1);
                        if *hole_depth == 0 {
                            frames.pop();
                        }
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }

    max_depth
}
