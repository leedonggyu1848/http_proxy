//! Path segment ID classification and folding policy used by the URL map
//! and endpoint schema inference.

use std::fmt;

/// Minimum number of distinct ID-like sibling segments under the same parent
/// required before folding them into a single `{id}` template node.
pub const ID_FOLD_THRESHOLD: usize = 2;

/// Observed value shape for a folded dynamic path segment.
///
/// Endpoint schemas use this classification to report the observed formats
/// of path parameters without retaining raw values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IdKind {
    /// Unsigned ASCII integer (e.g. `42`, `109283`).
    Integer,
    /// RFC 4122 hyphenated or 32-character hexadecimal UUID.
    Uuid,
    /// Hexadecimal identifier of at least 8 characters (e.g. `ObjectId`, SHA prefix).
    Hex,
    /// Mixed opaque identifier with digits and letters/separators (e.g. `usr_9a8b7c`).
    Opaque,
}

impl IdKind {
    /// Human-readable label shown in node details.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Integer => "integer",
            Self::Uuid => "uuid",
            Self::Hex => "hex",
            Self::Opaque => "opaque id",
        }
    }
}

impl fmt::Display for IdKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Classifies a normalized path segment as a dynamic ID candidate, or `None`
/// for static path names, version prefixes (`v1`), and dotted file names.
#[must_use]
pub fn classify_id_segment(segment: &str) -> Option<IdKind> {
    if segment.is_empty() || segment.contains('.') || is_api_version(segment) {
        return None;
    }
    if segment.bytes().all(|b| b.is_ascii_digit()) {
        return Some(IdKind::Integer);
    }
    if is_uuid(segment) {
        return Some(IdKind::Uuid);
    }
    if is_hex_token(segment) {
        return Some(IdKind::Hex);
    }
    if is_opaque_id(segment) {
        return Some(IdKind::Opaque);
    }
    None
}

/// A path segment in a view or filter: either an exact literal segment or a
/// folded `{id}` wildcard.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PathSegmentPattern {
    /// Exact raw path segment.
    Literal(String),
    /// Any path segment classified as an ID by [`classify_id_segment`].
    FoldedId,
}

impl PathSegmentPattern {
    /// Reports whether `segment` matches this pattern.
    #[must_use]
    pub fn matches(&self, segment: &str) -> bool {
        match self {
            Self::Literal(expected) => expected == segment,
            Self::FoldedId => classify_id_segment(segment).is_some(),
        }
    }

    #[must_use]
    pub fn display_text(&self) -> String {
        match self {
            Self::Literal(raw) => super::normalize::percent_decode_display(raw),
            Self::FoldedId => "{id}".to_owned(),
        }
    }
}

/// Matches API version prefixes like `v1`, `v2`, `v10` (kept literal, not IDs).
fn is_api_version(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    matches!(bytes.first(), Some(b'v' | b'V'))
        && (2..=3).contains(&bytes.len())
        && bytes
            .get(1..)
            .is_some_and(|rest| !rest.is_empty() && rest.iter().all(u8::is_ascii_digit))
}

/// Matches standard 36-char hyphenated UUIDs (`8-4-4-4-12`) or 32-char hex UUIDs.
fn is_uuid(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    if bytes.len() == 32 {
        return bytes.iter().all(u8::is_ascii_hexdigit);
    }
    if bytes.len() != 36 {
        return false;
    }
    for (index, &byte) in bytes.iter().enumerate() {
        if matches!(index, 8 | 13 | 18 | 23) {
            if byte != b'-' {
                return false;
            }
        } else if !byte.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// Matches hexadecimal IDs of length >= 8 (containing at least one digit so
/// plain English words such as `feedback` are never folded).
fn is_hex_token(segment: &str) -> bool {
    segment.len() >= 8
        && segment.bytes().all(|b| b.is_ascii_hexdigit())
        && segment.bytes().any(|b| b.is_ascii_digit())
}

/// Matches mixed alphanumeric opaque identifiers (e.g. `usr_9a8b7c`, `ord-2026-00192`,
/// base64url tokens) of length >= 8 containing at least one digit and one letter.
fn is_opaque_id(segment: &str) -> bool {
    if segment.len() < 8 {
        return false;
    }
    let mut digits = 0usize;
    let mut letters = 0usize;
    for byte in segment.bytes() {
        if byte.is_ascii_digit() {
            digits += 1;
        } else if byte.is_ascii_alphabetic() {
            letters += 1;
        } else if !matches!(byte, b'-' | b'_') {
            return false;
        }
    }
    digits >= 2 && letters >= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_id_formats() {
        assert_eq!(classify_id_segment("123"), Some(IdKind::Integer));
        assert_eq!(classify_id_segment("0"), Some(IdKind::Integer));
        assert_eq!(
            classify_id_segment("550e8400-e29b-41d4-a716-446655440000"),
            Some(IdKind::Uuid)
        );
        assert_eq!(
            classify_id_segment("550e8400e29b41d4a716446655440000"),
            Some(IdKind::Uuid)
        );
        assert_eq!(
            classify_id_segment("507f1f77bcf86cd799439011"),
            Some(IdKind::Hex)
        );
        assert_eq!(classify_id_segment("deadbeef1"), Some(IdKind::Hex));
        assert_eq!(classify_id_segment("usr_9a8b7c6d"), Some(IdKind::Opaque));
        assert_eq!(classify_id_segment("ord-2026-0019"), Some(IdKind::Opaque));
    }

    #[test]
    fn preserves_static_segments_versions_and_files() {
        for static_segment in [
            "",
            "users",
            "orders",
            "v1",
            "v2",
            "v10",
            "app.js",
            "bundle.12345678.js",
            "index.html",
            "deadbeef",
            "feedback",
            "oauth2",
        ] {
            assert_eq!(
                classify_id_segment(static_segment),
                None,
                "segment {static_segment:?} should remain literal"
            );
        }
    }

    #[test]
    fn pattern_matches_literals_and_folded_ids() {
        assert!(PathSegmentPattern::Literal("users".to_owned()).matches("users"));
        assert!(!PathSegmentPattern::Literal("users".to_owned()).matches("orders"));
        assert!(PathSegmentPattern::FoldedId.matches("456"));
        assert!(!PathSegmentPattern::FoldedId.matches("users"));
    }
}
