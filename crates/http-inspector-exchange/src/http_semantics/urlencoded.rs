//! Raw query boundaries and name/value pairs shared with URL-encoded forms.

use std::borrow::Cow;

/// Slices of a request target. The fragment includes its leading `#`.
pub struct TargetParts<'a> {
    pub base: &'a str,
    pub query: Option<&'a str>,
    pub fragment: &'a str,
}

/// Locates the first query delimiter before the fragment without validating or decoding the URL.
#[must_use]
pub fn split_target(target: &str) -> TargetParts<'_> {
    let (before_fragment, fragment) = target.split_at(target.find('#').unwrap_or(target.len()));
    let (base, query) = before_fragment
        .split_once('?')
        .map_or((before_fragment, None), |(base, query)| (base, Some(query)));
    TargetParts {
        base,
        query,
        fragment,
    }
}

/// An encoded pair. A missing `=` leaves `value` absent, even if the name is empty.
pub struct RawPair<'a> {
    pub raw: &'a str,
    pub name: &'a str,
    pub value: Option<&'a str>,
}

impl<'a> RawPair<'a> {
    /// Splits a single pair at its first `=`.
    #[must_use]
    pub fn parse(raw: &'a str) -> Self {
        let (name, value) = raw
            .split_once('=')
            .map_or((raw, None), |(name, value)| (name, Some(value)));
        Self { raw, name, value }
    }
}

/// Visits nonempty pairs in wire order, retaining their original spelling.
pub fn raw_pairs(text: &str) -> impl Iterator<Item = RawPair<'_>> {
    text.split('&')
        .filter(|part| !part.is_empty())
        .map(RawPair::parse)
}

/// How to interpret a literal `+` while decoding percent escapes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Plus {
    Literal,
    Space,
}

/// Decodes octets, leaving malformed escapes intact. UTF-8 handling belongs to the caller.
#[must_use]
pub fn decode(raw: &str, plus: Plus) -> Cow<'_, [u8]> {
    let mut rest = raw.as_bytes();
    if !rest.contains(&b'%') && (plus == Plus::Literal || !rest.contains(&b'+')) {
        return Cow::Borrowed(rest);
    }
    let mut bytes = Vec::with_capacity(rest.len());
    while let Some((&first, tail)) = rest.split_first() {
        if let (b'%', [high, low, remaining @ ..]) = (first, tail)
            && let (Some(h), Some(l)) = (hex(*high), hex(*low))
        {
            bytes.push(h << 4 | l);
            rest = remaining;
        } else {
            bytes.push(if first == b'+' && plus == Plus::Space {
                b' '
            } else {
                first
            });
            rest = tail;
        }
    }
    Cow::Owned(bytes)
}

const fn hex(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_ends_at_the_first_fragment() {
        assert_eq!(split_target("/p#frag?x=1").query, None);
        let parts = split_target("/p?x=1?y=2#frag?z");
        assert_eq!(
            (parts.base, parts.query, parts.fragment),
            ("/p", Some("x=1?y=2"), "#frag?z")
        );
        assert_eq!(split_target("/p?#").query, Some(""));
    }

    #[test]
    fn preserves_duplicates_separators_and_raw_values() {
        let pairs: Vec<_> = raw_pairs("a=1&a=2&flag&empty=&q=x%26y%3Dz&bad=%ZZ&&=a=b")
            .map(|p| (p.raw, p.name, p.value))
            .collect();
        assert_eq!(
            pairs,
            [
                ("a=1", "a", Some("1")),
                ("a=2", "a", Some("2")),
                ("flag", "flag", None),
                ("empty=", "empty", Some("")),
                ("q=x%26y%3Dz", "q", Some("x%26y%3Dz")),
                ("bad=%ZZ", "bad", Some("%ZZ")),
                ("=a=b", "", Some("a=b")),
            ]
        );
    }

    #[test]
    fn decodes_bytes_without_rescanning_escapes() {
        assert_eq!(
            &*decode("a+b%2Bc%26%3D%25FF%FF%ZZ%4", Plus::Space),
            b"a b+c&=%FF\xff%ZZ%4"
        );
        assert_eq!(&*decode("a+b%2Bc", Plus::Literal), b"a+b+c");
    }
}
