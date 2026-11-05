//! Owned JavaScript primitive values used by bounded row proofs.

/// A primitive value, retaining number sign and type independently of display formatting.
#[derive(Clone, Debug)]
pub enum Scalar {
    /// No supplied value.
    Undefined,
    /// The null value.
    Null,
    /// A boolean.
    Bool(bool),
    /// An IEEE number, including signed zero and non-finite values.
    Number(f64),
    /// A decimal bigint, with an optional minus sign.
    BigInt(String),
    /// Owned text; callers retain the lossless-text guard.
    String(String),
}

impl Scalar {
    /// A canonical expression spelling this primitive without executing project code.
    pub fn source(&self) -> String {
        match self {
            Self::Undefined => "undefined".into(),
            Self::Null => "null".into(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) if value.is_nan() => "NaN".into(),
            Self::Number(value) if *value == f64::INFINITY => "Infinity".into(),
            Self::Number(value) if *value == f64::NEG_INFINITY => "-Infinity".into(),
            Self::Number(value) if *value == 0.0 && value.is_sign_negative() => "-0".into(),
            Self::Number(value) => ryu_js::Buffer::new().format(*value).to_owned(),
            Self::BigInt(value) => format!("{value}n"),
            Self::String(value) => serde_json::to_string(value).expect("strings serialize"),
        }
    }

    /// A type-preserving equality token; number zero keeps its sign.
    pub fn identity(&self) -> String {
        format!(
            "{}:{}",
            match self {
                Self::Undefined => "undefined",
                Self::Null => "null",
                Self::Bool(_) => "boolean",
                Self::Number(_) => "number",
                Self::BigInt(_) => "bigint",
                Self::String(_) => "string",
            },
            self.source()
        )
    }
}

impl crate::memory::HeapBytes for Scalar {
    /// Only text and bigint spellings allocate outside the inline value.
    fn heap_bytes(&self) -> usize {
        match self {
            Self::BigInt(text) | Self::String(text) => text.capacity(),
            _ => 0,
        }
    }
}
