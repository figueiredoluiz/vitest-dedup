//! Rendering the supported literal table-title arguments; an unknown rendering stays unknown.

use serde_json::{Map, Value};

/// Renders scalar printf and named fields without interpreting unknown cells as undefined.
/// Composite inspection, dotted fields and percent escaping are deliberately refused.
pub(super) fn render(
    pattern: &str,
    arguments: &[Option<Value>],
    fields: Option<&Map<String, Value>>,
    index: usize,
) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut text = String::new();
    let mut cursor = 0;
    let mut position = 0;
    while position < chars.len() {
        let c = chars[position];
        if c == '%' && position + 1 < chars.len() {
            let directive = chars[position + 1];
            let rendered = match directive {
                '%' | 'O' | 'c' => return None,
                '#' => Some(index.to_string()),
                '$' => Some((index + 1).to_string()),
                's' | 'd' | 'i' | 'f' | 'j' | 'o' => {
                    let argument = match arguments.get(cursor) {
                        Some(value) => Some(value.as_ref()?),
                        None => None,
                    };
                    cursor += 1;
                    Some(format(argument, directive)?)
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
                let value = if let Ok(index) = field.parse::<usize>() {
                    arguments.get(index)?.as_ref()?
                } else {
                    fields?.get(&field)?
                };
                let rendered = scalar(value)?;
                if rendered.encode_utf16().count() > 40 {
                    return None;
                }
                text.push_str(&rendered);
                position += 1 + length;
                continue;
            }
        }
        text.push(c);
        position += 1;
    }
    (text.len() <= 65_536).then_some(text)
}

/// The plain spelling shared by scalar string and named-field substitutions.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => {
            let number = number.as_f64()?;
            if number == 0.0 && number.is_sign_negative() {
                return None;
            }
            Some(ryu_js::Buffer::new().format(number).to_owned())
        }
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some("null".to_owned()),
        _ => None,
    }
}

/// One supported substitution. Numeric prefix parsing is accepted only when the whole numeric
/// spelling is ordinary decimal; unsupported coercions must not manufacture a title collision.
fn format(value: Option<&Value>, directive: char) -> Option<String> {
    let Some(value) = value else {
        return matches!(directive, 's' | 'j' | 'o').then(|| "undefined".to_owned());
    };
    match directive {
        's' => scalar(value),
        'j' if value.is_string() => Some(value.to_string()),
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
                let number = text.parse::<f64>().ok()?;
                if directive == 'i' {
                    number.trunc()
                } else {
                    number
                }
            };
            (number.is_finite() && !(number == 0.0 && number.is_sign_negative())).then(|| {
                ryu_js::Buffer::new()
                    .format(if number == 0.0 { 0.0 } else { number })
                    .to_owned()
            })
        }
        _ => None,
    }
}

/// Refuses the unresolved named-field and string integer conversions for row equality.
/// The name rule retains its existing rendering until its separate formatter correction.
pub(super) fn render_row(
    pattern: &str,
    arguments: &[Option<Value>],
    fields: Option<&Map<String, Value>>,
    index: usize,
) -> Option<String> {
    if pattern.contains(['$', '\u{FFFD}'])
        || (pattern.contains("%i") && arguments.iter().flatten().any(Value::is_string))
    {
        return None;
    }
    render(pattern, arguments, fields, index)
}
