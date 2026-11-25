//! `application/x-www-form-urlencoded` bodies and URL query strings.

use super::{Styled, TokenKind};
use http_inspector_exchange::http_semantics::urlencoded::{self, Plus};

/// Percent-decodes each `key=value` pair of a form body or query string.
/// Empty pairs are skipped and malformed escapes are kept as written.
#[must_use]
pub fn pairs(text: &str) -> Vec<(String, String)> {
    decoded_pairs(text).collect()
}

/// Iterates over decoded names and values using lossy UTF-8 for display and type inference.
pub fn decoded_pairs(text: &str) -> impl Iterator<Item = (String, String)> {
    urlencoded::raw_pairs(text)
        .map(|pair| (decode(pair.name), decode(pair.value.unwrap_or_default())))
}

/// Shows one decoded `key = value` pair per line.
pub(super) fn pretty(text: &str) -> Styled {
    let mut out = Styled::default();
    for (index, (key, value)) in pairs(text.trim_end_matches(['\r', '\n']))
        .iter()
        .enumerate()
    {
        if index > 0 {
            out.push("\n", TokenKind::Text);
        }
        out.push(&visible(key), TokenKind::Key);
        out.push(" = ", TokenKind::Punctuation);
        out.push(&visible(value), TokenKind::String);
    }
    out
}

/// Escapes control characters so a decoded line break cannot look like the
/// start of another pair.
#[must_use]
pub fn visible(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

/// Decodes one form component to text, replacing invalid UTF-8.
#[must_use]
pub fn decode(text: &str) -> String {
    String::from_utf8_lossy(&urlencoded::decode(text, Plus::Space)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_one_pair_per_line() {
        let styled = pretty("name=J%C3%BCrgen+K&empty=&flag&&q=a%26b%3Dc\n");
        assert_eq!(
            styled.text(),
            "name = Jürgen K\nempty = \nflag = \nq = a&b=c"
        );
        assert_eq!(
            styled.pieces().take(3).collect::<Vec<_>>(),
            vec![
                ("name", TokenKind::Key),
                (" = ", TokenKind::Punctuation),
                ("Jürgen K", TokenKind::String),
            ]
        );
    }

    #[test]
    fn keeps_malformed_and_truncated_escapes() {
        assert_eq!(
            pairs("a=%zz&b=%4"),
            vec![
                ("a".to_owned(), "%zz".to_owned()),
                ("b".to_owned(), "%4".to_owned())
            ]
        );
        assert_eq!(pretty("a=%FF").text(), "a = \u{FFFD}");
    }

    #[test]
    fn escapes_decoded_line_breaks() {
        assert_eq!(pretty("a=1%0Ab%3D2").text(), "a = 1\\nb=2");
    }
}
