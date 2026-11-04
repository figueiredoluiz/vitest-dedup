//! Rendering the supported literal table-title arguments; an unknown rendering stays unknown.

use crate::framework::{TitleMode, TitleValue};
use crate::lang::limits::MAX_TEXT_BYTES;
use crate::lang::Scalar;
use serde_json::{Map, Value};
use std::borrow::Cow;
use std::collections::HashMap;

/// Renders what Vitest's `formatTitle` renders for scalar items, or nothing: unknown cells are
/// never undefined, and composite inspection, dotted fields and percent escaping are refused. A
/// `$` value past 40 UTF-16 units is refused too: a project may change that display limit.
pub(super) fn render(
    pattern: &str,
    arguments: &[Option<Value>],
    fields: Option<&Map<String, Value>>,
    index: usize,
) -> Option<String> {
    render_with(pattern, arguments, fields, index, false)
}

/// [`render`] for display: a long `$` value is cut as Vitest's default 40-unit limit cuts it.
pub(super) fn render_display(
    pattern: &str,
    arguments: &[Option<Value>],
    fields: Option<&Map<String, Value>>,
    index: usize,
) -> Option<String> {
    render_with(pattern, arguments, fields, index, true)
}

/// [`render`], cutting long `$` values when `truncate` holds and refusing them otherwise.
fn render_with(
    pattern: &str,
    arguments: &[Option<Value>],
    fields: Option<&Map<String, Value>>,
    index: usize,
    truncate: bool,
) -> Option<String> {
    let arguments = arguments
        .iter()
        .map(|v| v.clone().map(TitleValue::Json))
        .collect::<Vec<_>>();
    let fields = fields.map(|f| {
        f.iter()
            .map(|(k, v)| (k.clone(), TitleValue::Json(v.clone())))
            .collect()
    });
    render_values(
        pattern,
        &arguments,
        fields.as_ref(),
        index,
        if truncate {
            TitleMode::Display
        } else {
            TitleMode::Comparison
        },
    )
}

/// Renders typed row inputs without conflating unknown, undefined, number or bigint values.
pub(super) fn render_values(
    pattern: &str,
    arguments: &[Option<TitleValue>],
    fields: Option<&HashMap<String, TitleValue>>,
    index: usize,
    mode: TitleMode,
) -> Option<String> {
    let truncate = mode == TitleMode::Display;
    if mode == TitleMode::Row
        && (pattern.contains('\u{fffd}')
            || (pattern.contains("%i")
                && arguments.iter().flatten().any(|value| {
                    matches!(
                        value,
                        TitleValue::Json(Value::String(_)) | TitleValue::Scalar(Scalar::String(_))
                    )
                })))
    {
        return None;
    }
    if pattern.len() > MAX_TEXT_BYTES || pattern.contains("%%") {
        return None;
    }
    // Substituted first, so these can spell a `$` key.
    let pattern = pattern
        .replace("%#", &index.to_string())
        .replace("%$", &(index + 1).to_string());
    let chars: Vec<char> = pattern.chars().collect();
    let mut text = String::new();
    let mut cursor = 0;
    let mut float_cursor = 0;
    let mut position = 0;
    while position < chars.len() {
        // The text only grows, so a rendering past the limit is refused as soon as it passes it.
        if text.len() > MAX_TEXT_BYTES {
            return None;
        }
        let c = chars[position];
        if c == '%' && position + 1 < chars.len() {
            let directive = chars[position + 1];
            let rendered = match directive {
                'O' | 'c' => return None,
                's' | 'd' | 'i' | 'f' | 'j' | 'o' => {
                    let argument = match arguments.get(cursor) {
                        Some(value) => Some(value.as_ref()?),
                        None => None,
                    };
                    cursor += 1;
                    let mut rendered = format_typed(argument, directive)?;
                    if directive == 'f' {
                        // Vitest's prefix pass indexes float occurrences, not printf arguments.
                        let prefix = match arguments.get(float_cursor) {
                            Some(None) => return None,
                            Some(Some(TitleValue::Scalar(Scalar::Number(number)))) => {
                                if number.is_nan() {
                                    return None;
                                }
                                *number == 0.0 && number.is_sign_negative()
                            }
                            Some(Some(TitleValue::Json(Value::Number(number)))) => number
                                .as_f64()
                                .is_some_and(|n| n == 0.0 && n.is_sign_negative()),
                            _ => false,
                        };
                        float_cursor += 1;
                        if prefix {
                            rendered.insert(0, '-');
                        }
                    }
                    Some(rendered)
                }
                _ => None,
            };
            if let Some(rendered) = rendered {
                text.push_str(&rendered);
                position += 2;
                continue;
            }
        }
        if c == '$' {
            let length = chars[position + 1..]
                .iter()
                .take_while(|c| {
                    c.is_alphanumeric() || matches!(**c, '_' | '.' | '$') || !c.is_ascii()
                })
                .count();
            if length > 0 {
                let field: String = chars[position + 1..position + 1 + length].iter().collect();
                if field.contains('.') || !field.is_ascii() {
                    return None;
                }
                let value = if field.bytes().all(|byte| byte.is_ascii_digit()) {
                    // `$01` is a key no item has; an object first item answers before position.
                    if (field.len() > 1 && field.starts_with('0'))
                        || matches!(
                            arguments.first(),
                            Some(None | Some(TitleValue::Json(Value::Object(_))))
                        )
                    {
                        return None;
                    }
                    arguments.get(field.parse::<usize>().ok()?)?.as_ref()?
                } else {
                    fields?.get(&field)?
                };
                let rendered = scalar_typed(value)?;
                if !truncate && rendered.encode_utf16().nth(40).is_some() {
                    return None;
                }
                text.push_str(&truncated(&rendered));
                position += 1 + length;
                continue;
            }
        }
        text.push(c);
        position += 1;
    }
    (text.len() <= MAX_TEXT_BYTES).then_some(text)
}

/// The scalar display shared by positional and named typed values, borrowing a string.
fn scalar_typed(value: &TitleValue) -> Option<Cow<'_, str>> {
    match value {
        TitleValue::Json(value) => scalar(value),
        TitleValue::Scalar(Scalar::String(text)) => Some(Cow::Borrowed(text)),
        TitleValue::Scalar(Scalar::BigInt(text)) => Some(Cow::Owned(format!("{text}n"))),
        TitleValue::Scalar(value) => Some(Cow::Owned(value.source())),
    }
}

/// Formatting for primitives JSON cannot represent; ordinary JSON uses the existing formatter.
fn format_typed(value: Option<&TitleValue>, directive: char) -> Option<String> {
    let Some(value) = value else {
        return format(None, directive).map(Cow::into_owned);
    };
    let scalar = match value {
        TitleValue::Json(value) => return format(Some(value), directive).map(Cow::into_owned),
        TitleValue::Scalar(value) => value,
    };
    match scalar {
        Scalar::Undefined => format(None, directive).map(Cow::into_owned),
        Scalar::BigInt(text) => matches!(directive, 's' | 'o').then(|| format!("{text}n")),
        Scalar::Number(number)
            if !number.is_finite() || (*number == 0.0 && number.is_sign_negative()) =>
        {
            match directive {
                's' | 'o' => Some(scalar.source()),
                'd' => Some(if *number == 0.0 {
                    "0".into()
                } else {
                    scalar.source()
                }),
                'j' => Some(if number.is_finite() { "0" } else { "null" }.into()),
                'i' => Some(if number.is_finite() { "0" } else { "NaN" }.into()),
                'f' if !number.is_nan() => Some(if *number == 0.0 {
                    "0".into()
                } else {
                    scalar.source()
                }),
                _ => None,
            }
        }
        _ => {
            let json = match scalar {
                Scalar::Null => Value::Null,
                Scalar::Bool(v) => Value::Bool(*v),
                Scalar::Number(v) => Value::Number(serde_json::Number::from_f64(*v)?),
                Scalar::String(v) => Value::String(v.clone()),
                _ => return None,
            };
            format(Some(&json), directive).map(Cow::into_owned)
        }
    }
}

/// A `$` substitution as Vitest displays it: past 40 UTF-16 units, the first 39 and `…`, never
/// keeping half of a surrogate pair. Reads at most 41 units, however long the value.
fn truncated(value: &str) -> Cow<'_, str> {
    if value.encode_utf16().nth(40).is_none() {
        return Cow::Borrowed(value);
    }
    let mut kept: Vec<u16> = value.encode_utf16().take(39).collect();
    if kept
        .last()
        .is_some_and(|unit| (0xD800..0xDC00).contains(unit))
    {
        kept.pop();
    }
    Cow::Owned(format!("{}…", String::from_utf16_lossy(&kept)))
}

/// The plain spelling shared by scalar string and named-field substitutions, borrowing a string.
fn scalar(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::String(text) => Some(Cow::Borrowed(text)),
        Value::Number(number) => {
            let number = number.as_f64()?;
            if number == 0.0 && number.is_sign_negative() {
                return None;
            }
            Some(Cow::Owned(ryu_js::Buffer::new().format(number).to_owned()))
        }
        Value::Bool(value) => Some(Cow::Borrowed(if *value { "true" } else { "false" })),
        Value::Null => Some(Cow::Borrowed("null")),
        _ => None,
    }
}

/// One supported substitution. Numeric prefix parsing is accepted only when the whole numeric
/// spelling is ordinary decimal; unsupported coercions must not manufacture a title collision.
fn format(value: Option<&Value>, directive: char) -> Option<Cow<'_, str>> {
    let Some(value) = value else {
        return matches!(directive, 's' | 'j' | 'o').then_some(Cow::Borrowed("undefined"));
    };
    match directive {
        's' => scalar(value),
        'j' if value.is_string() => Some(Cow::Owned(value.to_string())),
        'j' => scalar(value),
        'o' if !value.is_string() => scalar(value),
        'd' | 'i' | 'f' => {
            let text = scalar(value)?;
            let number = if directive == 'd' {
                match value {
                    Value::Null => 0.0,
                    Value::Bool(value) => f64::from(u8::from(*value)),
                    Value::Number(number) => number.as_f64()?,
                    Value::String(text) if text.is_empty() => 0.0,
                    _ => text.parse::<f64>().ok()?,
                }
            } else {
                if text.contains(['e', 'E']) {
                    return None;
                }
                if directive == 'i'
                    && !text
                        .trim_start_matches(['+', '-'])
                        .starts_with(|c: char| c.is_ascii_digit())
                {
                    return None;
                }
                // `parseInt` stops at the point: what follows it cannot round the result.
                let digits = if directive == 'i' {
                    text.split('.').next()?
                } else {
                    &*text
                };
                digits.parse::<f64>().ok()?
            };
            (number.is_finite() && !(number == 0.0 && number.is_sign_negative())).then(|| {
                Cow::Owned(
                    ryu_js::Buffer::new()
                        .format(if number == 0.0 { 0.0 } else { number })
                        .to_owned(),
                )
            })
        }
        _ => None,
    }
}
