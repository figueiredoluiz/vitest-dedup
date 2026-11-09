//! Lossless owned JavaScript text in UTF-8 storage.
//!
//! Semantic strings use U+FFFD followed by four lowercase hex digits for a lone surrogate
//! or U+FFFD itself. Escaping the introducer on *every* input makes this injective. Valid pairs
//! use their ordinary Unicode scalar. This representation is never report or filesystem text.
//! Raw source, paths and diagnostics remain ordinary Rust strings. Concatenation must pass
//! through [`canonical`], since two lone surrogates can become a pair at a join.

use std::borrow::Cow;
use std::fmt::Write as _;

/// Encode ordinary Unicode text, borrowing when no introducer needs escaping.
pub fn from_utf8(text: &str) -> Cow<'_, str> {
    if text.contains('\u{fffd}') {
        Cow::Owned(text.replace('\u{fffd}', "\u{fffd}fffd"))
    } else {
        Cow::Borrowed(text)
    }
}

/// Encode UTF-16 without replacing unpaired code units.
pub fn from_utf16(units: impl IntoIterator<Item = u16>) -> String {
    let mut text = String::new();
    for value in char::decode_utf16(units) {
        match value {
            Ok('\u{fffd}') => text.push_str("\u{fffd}fffd"),
            Ok(character) => text.push(character),
            Err(error) => write!(text, "\u{fffd}{:04x}", error.unpaired_surrogate()).unwrap(),
        }
    }
    text
}

/// The original code units of encoded semantic text, without an intermediate allocation.
pub fn units(text: &str) -> impl Iterator<Item = u16> + '_ {
    let mut chars = text.chars();
    let mut pending = None;
    std::iter::from_fn(move || {
        if let Some(low) = pending.take() {
            return Some(low);
        }
        let character = chars.next()?;
        if character == '\u{fffd}' {
            if let Some(unit) = chars
                .as_str()
                .get(..4)
                .and_then(|hex| u16::from_str_radix(hex, 16).ok())
                .filter(|unit| (0xd800..=0xdfff).contains(unit) || *unit == 0xfffd)
            {
                for _ in 0..4 {
                    chars.next();
                }
                return Some(unit);
            }
        }
        let mut buffer = [0; 2];
        let encoded = character.encode_utf16(&mut buffer);
        pending = encoded.get(1).copied();
        Some(encoded[0])
    })
}

/// Normalize a concatenation: a high and low surrogate at a join become one scalar.
pub fn canonical(text: String) -> String {
    if text.contains('\u{fffd}') {
        from_utf16(units(&text))
    } else {
        text
    }
}

/// Decode a filesystem/configuration string; lone surrogates have no supported UTF-8 path.
pub fn to_utf8(text: &str) -> Option<Cow<'_, str>> {
    if !text.contains('\u{fffd}') {
        return Some(Cow::Borrowed(text));
    }
    char::decode_utf16(units(text))
        .collect::<Result<String, _>>()
        .ok()
        .map(Cow::Owned)
}

/// Human-readable text, spelling lone surrogates explicitly instead of replacing them.
pub fn display(text: &str) -> Cow<'_, str> {
    if !text.contains('\u{fffd}') {
        return Cow::Borrowed(text);
    }
    let mut out = String::new();
    for value in char::decode_utf16(units(text)) {
        match value {
            Ok(character) => out.push(character),
            Err(error) => write!(out, "\\u{:04x}", error.unpaired_surrogate()).unwrap(),
        }
    }
    Cow::Owned(out)
}

/// A JSON/JavaScript string literal preserving every code unit, including lone surrogates.
pub fn json(text: &str) -> String {
    let mut out = String::from("\"");
    for value in char::decode_utf16(units(text)) {
        match value {
            Err(error) => write!(out, "\\u{:04x}", error.unpaired_surrogate()).unwrap(),
            Ok('"') => out.push_str("\\\""),
            Ok('\\') => out.push_str("\\\\"),
            Ok('\u{08}') => out.push_str("\\b"),
            Ok('\t') => out.push_str("\\t"),
            Ok('\n') => out.push_str("\\n"),
            Ok('\u{0c}') => out.push_str("\\f"),
            Ok('\r') => out.push_str("\\r"),
            Ok(character) if character <= '\u{1f}' => {
                write!(out, "\\u{:04x}", u32::from(character)).unwrap();
            }
            Ok(character) => out.push(character),
        }
    }
    out.push('"');
    out
}

/// Serialize encoded semantic strings as actual JSON strings, not display escapes.
pub(crate) fn serialize<S: serde::Serializer>(
    text: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::Serialize;
    serde_json::value::RawValue::from_string(json(text))
        .map_err(serde::ser::Error::custom)?
        .serialize(serializer)
}

/// Serialize a sequence of encoded titles at a JSON boundary.
pub(crate) fn serialize_titles<S: serde::Serializer>(
    titles: &[String],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    #[derive(serde::Serialize)]
    struct Title<'a>(#[serde(serialize_with = "serialize")] &'a str);
    let mut sequence = serializer.serialize_seq(Some(titles.len()))?;
    for title in titles {
        sequence.serialize_element(&Title(title))?;
    }
    sequence.end()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// All individual code units round trip, including every surrogate and the introducer.
    #[test]
    fn every_code_unit_round_trips() {
        for unit in 0..=u16::MAX {
            let encoded = from_utf16([unit]);
            assert_eq!(units(&encoded).collect::<Vec<_>>(), vec![unit]);
        }
        assert_ne!(from_utf16([0xd800]), from_utf8("�d800"));
        assert_ne!(from_utf16([0xfffd]), from_utf8("�fffd"));
    }

    /// Joining split pairs preserves the identity of their complete UTF-16 sequence.
    #[test]
    fn joins_and_output_preserve_code_units() {
        let controls = "a\n\t\r\u{08}\u{0c}\0\"\\";
        assert_eq!(json(controls), serde_json::to_string(controls).unwrap());
        assert_eq!(
            canonical(from_utf16([0xd83d]) + &from_utf16([0xde00])),
            "😀"
        );
        let text = from_utf16([0xd800, 0xfffd, 0xdc00]);
        assert_eq!(json(&text), "\"\\ud800�\\udc00\"");
        assert_eq!(display(&text), "\\ud800�\\udc00");
        assert!(to_utf8(&text).is_none());
        assert_eq!(to_utf8(&from_utf8("�d800")), Some(Cow::Borrowed("�d800")));
        #[derive(serde::Serialize)]
        struct Record(#[serde(serialize_with = "serialize")] String);
        assert_eq!(
            serde_json::to_string(&Record(text.clone())).unwrap(),
            json(&text)
        );
    }

    /// Cases each property below runs.
    const CASES: u32 = 2048;

    /// A UTF-16 code unit, weighted toward the units the encoding treats specially: surrogates, the
    /// introducer, and the hex digits that can follow it.
    pub(crate) fn code_unit() -> impl proptest::strategy::Strategy<Value = u16> {
        use proptest::prelude::*;
        let hex: Vec<u16> = "0123456789abcdefABCDEF".encode_utf16().collect();
        prop_oneof![
            any::<u16>(),
            0xd800_u16..=0xdfff,
            Just(0xfffd_u16),
            proptest::sample::select(hex),
        ]
    }

    /// A sequence of [`code_unit`]s, short enough that adjacent special units are common.
    pub(crate) fn code_units() -> impl proptest::strategy::Strategy<Value = Vec<u16>> {
        proptest::collection::vec(code_unit(), 0..24)
    }

    /// Ordinary Unicode text, weighted toward the introducer, hex digits, the characters JSON
    /// escapes and astral scalars.
    fn unicode_text() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::prelude::*;
        let character = prop_oneof![
            any::<char>(),
            Just('\u{fffd}'),
            proptest::sample::select("0123456789abcdefABCDEF".chars().collect::<Vec<_>>()),
            proptest::char::range('\0', '\u{20}'),
            proptest::sample::select(vec!['"', '\\', '/', '\u{7f}', '\u{2028}', '\u{2029}']),
            proptest::char::range('\u{10000}', '\u{10ffff}'),
        ];
        proptest::collection::vec(character, 0..24)
            .prop_map(|characters| characters.into_iter().collect())
    }

    /// Encoding UTF-16 and reading its units back is the identity, so the encoding is injective.
    #[test]
    fn code_units_round_trip() {
        crate::testing::property(CASES, code_units(), |input| {
            proptest::prop_assert_eq!(
                units(&from_utf16(input.iter().copied())).collect::<Vec<_>>(),
                input
            );
            Ok(())
        });
    }

    /// Both encoders give ordinary text the same encoding, whichever one a reader takes.
    #[test]
    fn utf8_and_utf16_encoders_agree() {
        crate::testing::property(CASES, unicode_text(), |input| {
            proptest::prop_assert_eq!(
                from_utf8(&input).into_owned(),
                from_utf16(input.encode_utf16())
            );
            Ok(())
        });
    }

    /// Joining two encodings and normalizing equals encoding the joined units.
    #[test]
    fn canonical_join_equals_joined_units() {
        crate::testing::property(CASES, (code_units(), code_units()), |(left, right)| {
            let joined =
                canonical(from_utf16(left.iter().copied()) + &from_utf16(right.iter().copied()));
            let units_joined = from_utf16(left.into_iter().chain(right));
            proptest::prop_assert_eq!(joined, units_joined);
            Ok(())
        });
    }

    /// Text decodes to UTF-8 exactly when its units are valid UTF-16, and to the same string.
    #[test]
    fn to_utf8_decodes_exactly_the_valid_units() {
        crate::testing::property(CASES, code_units(), |input| {
            let encoded = from_utf16(input.iter().copied());
            let decoded = to_utf8(&encoded).map(Cow::into_owned);
            proptest::prop_assert_eq!(decoded, String::from_utf16(&input).ok());
            Ok(())
        });
    }

    /// For ordinary text, the JSON literal is the one `serde_json` writes.
    #[test]
    fn json_matches_serde_json_on_unicode_text() {
        crate::testing::property(CASES, unicode_text(), |input| {
            proptest::prop_assert_eq!(
                json(&from_utf8(&input)),
                serde_json::to_string(&input).unwrap()
            );
            Ok(())
        });
    }
}
