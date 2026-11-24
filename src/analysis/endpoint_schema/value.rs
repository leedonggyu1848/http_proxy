//! Value type and string format classification.
//!
//! Classification looks at a value only long enough to name its type and
//! format; the value itself is never returned or stored.

use std::collections::BTreeMap;
use std::fmt;

use crate::analysis::url::{IdKind, classify_id_segment};

/// JSON-compatible type observed for one value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ValueKind {
    /// JSON `null`.
    Null,
    /// `true` or `false`.
    Boolean,
    /// Whole number.
    Integer,
    /// Number with a fraction or exponent.
    Number,
    /// Text.
    String,
    /// JSON object.
    Object,
    /// JSON array.
    Array,
}

impl ValueKind {
    /// Lowercase label shown in the Map detail panel.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::String => "string",
            Self::Object => "object",
            Self::Array => "array",
        }
    }
}

impl fmt::Display for ValueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Recognizable shape of a string value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum StringFormat {
    /// Empty string.
    Empty,
    /// Digits only, sent as text.
    Numeric,
    /// RFC 4122 UUID.
    Uuid,
    /// Date and time such as `2024-01-31T10:00:00Z`.
    DateTime,
    /// Calendar date such as `2024-01-31`.
    Date,
    /// Email address.
    Email,
    /// Absolute `http`/`https` URL.
    Url,
    /// Opaque identifier or token-like text (letters mixed with digits).
    Identifier,
}

impl StringFormat {
    /// Lowercase label shown in the Map detail panel.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Numeric => "numeric",
            Self::Uuid => "uuid",
            Self::DateTime => "date-time",
            Self::Date => "date",
            Self::Email => "email",
            Self::Url => "url",
            Self::Identifier => "id",
        }
    }
}

impl fmt::Display for StringFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Type and optional string format of one observed value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueShape {
    /// Observed type.
    pub kind: ValueKind,
    /// Format of a string value, when one is recognized.
    pub format: Option<StringFormat>,
}

impl ValueShape {
    /// A shape with no string format.
    #[must_use]
    pub const fn plain(kind: ValueKind) -> Self {
        Self { kind, format: None }
    }

    /// Classifies a JSON string value.
    #[must_use]
    pub fn of_json_string(text: &str) -> Self {
        Self {
            kind: ValueKind::String,
            format: string_format(text),
        }
    }

    /// Classifies a text value from a query string or form field, where
    /// every value arrives as text and `1`, `true` or `2.5` carry the type
    /// a server will most likely parse them into.
    #[must_use]
    pub fn of_text(text: &str) -> Self {
        if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") {
            return Self::plain(ValueKind::Boolean);
        }
        if is_integer(text) {
            return Self::plain(ValueKind::Integer);
        }
        if is_decimal(text) {
            return Self::plain(ValueKind::Number);
        }
        Self::of_json_string(text)
    }
}

/// Counts of the types and string formats observed for one field or parameter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TypeCounts {
    kinds: BTreeMap<ValueKind, u64>,
    formats: BTreeMap<StringFormat, u64>,
}

impl TypeCounts {
    /// Adds one observed value.
    pub fn record(&mut self, shape: ValueShape) {
        bump(&mut self.kinds, shape.kind, 1);
        if let Some(format) = shape.format {
            bump(&mut self.formats, format, 1);
        }
    }

    /// Adds every count of `other`.
    pub fn merge(&mut self, other: &Self) {
        for (&kind, &count) in &other.kinds {
            bump(&mut self.kinds, kind, count);
        }
        for (&format, &count) in &other.formats {
            bump(&mut self.formats, format, count);
        }
    }

    /// Observed types with their counts, in [`ValueKind`] order.
    #[must_use]
    pub const fn kinds(&self) -> &BTreeMap<ValueKind, u64> {
        &self.kinds
    }

    /// Observed string formats with their counts.
    #[must_use]
    pub const fn formats(&self) -> &BTreeMap<StringFormat, u64> {
        &self.formats
    }

    /// Number of values observed as `kind`.
    #[must_use]
    pub fn count(&self, kind: ValueKind) -> u64 {
        self.kinds.get(&kind).copied().unwrap_or(0)
    }

    /// Reports whether more than one non-`null` type was observed.
    #[must_use]
    pub fn is_mixed(&self) -> bool {
        self.kinds
            .keys()
            .filter(|kind| **kind != ValueKind::Null)
            .count()
            > 1
    }

    /// Formats the observed types as `integer | string (uuid) | null`.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        for kind in self.kinds.keys() {
            if *kind == ValueKind::String && !self.formats.is_empty() {
                let formats: Vec<&str> = self.formats.keys().map(|format| format.label()).collect();
                parts.push(format!("string ({})", formats.join(", ")));
            } else {
                parts.push(kind.label().to_owned());
            }
        }
        if parts.is_empty() {
            return "—".to_owned();
        }
        parts.join(" | ")
    }
}

pub(super) fn bump<K: Ord>(map: &mut BTreeMap<K, u64>, key: K, count: u64) {
    let entry = map.entry(key).or_insert(0);
    *entry = entry.saturating_add(count);
}

fn string_format(text: &str) -> Option<StringFormat> {
    if text.is_empty() {
        return Some(StringFormat::Empty);
    }
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(StringFormat::Numeric);
    }
    if is_date(text) {
        return Some(StringFormat::Date);
    }
    if is_date_time(text) {
        return Some(StringFormat::DateTime);
    }
    if is_email(text) {
        return Some(StringFormat::Email);
    }
    if is_url(text) {
        return Some(StringFormat::Url);
    }
    match classify_id_segment(text) {
        Some(IdKind::Uuid) => Some(StringFormat::Uuid),
        Some(_) => Some(StringFormat::Identifier),
        None => None,
    }
}

fn is_integer(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.len() <= 19 && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_decimal(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let Some((whole, fraction)) = digits.split_once('.') else {
        return false;
    };
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
}

/// `YYYY-MM-DD`.
fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

/// `YYYY-MM-DD` followed by `T` or a space and a time.
fn is_date_time(text: &str) -> bool {
    let (Some(date), Some(separator), Some(time)) =
        (text.get(..10), text.get(10..11), text.get(11..))
    else {
        return false;
    };
    is_date(date)
        && (separator == "T" || separator == "t" || separator == " ")
        && time.len() >= 5
        && time.as_bytes().get(2) == Some(&b':')
        && time.bytes().take(2).all(|byte| byte.is_ascii_digit())
}

fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.contains(char::is_whitespace)
        && !domain.contains('@')
}

fn is_url(text: &str) -> bool {
    let lower = text.get(..8).map_or_default(str::to_ascii_lowercase);
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && !text.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_values_keep_only_their_type_and_format() {
        assert_eq!(ValueShape::of_text("42").kind, ValueKind::Integer);
        assert_eq!(ValueShape::of_text("-3.5").kind, ValueKind::Number);
        assert_eq!(ValueShape::of_text("TRUE").kind, ValueKind::Boolean);
        assert_eq!(ValueShape::of_text("").format, Some(StringFormat::Empty));
        assert_eq!(
            ValueShape::of_text("2024-01-31").format,
            Some(StringFormat::Date)
        );
        assert_eq!(
            ValueShape::of_text("2024-01-31T10:00:00Z").format,
            Some(StringFormat::DateTime)
        );
        assert_eq!(
            ValueShape::of_text("someone@example.test").format,
            Some(StringFormat::Email)
        );
        assert_eq!(
            ValueShape::of_text("https://example.test/a").format,
            Some(StringFormat::Url)
        );
        assert_eq!(
            ValueShape::of_text("550e8400-e29b-41d4-a716-446655440000").format,
            Some(StringFormat::Uuid)
        );
        assert_eq!(ValueShape::of_text("hello world").format, None);
        assert_eq!(
            ValueShape::of_json_string("123").format,
            Some(StringFormat::Numeric)
        );
    }

    #[test]
    fn counts_summarize_mixed_and_null_types() {
        let mut counts = TypeCounts::default();
        counts.record(ValueShape::plain(ValueKind::Integer));
        counts.record(ValueShape::plain(ValueKind::Null));
        assert!(!counts.is_mixed());
        counts.record(ValueShape::of_json_string("2024-01-31"));
        assert!(counts.is_mixed());
        assert_eq!(counts.summary(), "null | integer | string (date)");
        assert_eq!(counts.count(ValueKind::Integer), 1);
    }
}
