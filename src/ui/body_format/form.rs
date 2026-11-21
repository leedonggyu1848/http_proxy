//! `application/x-www-form-urlencoded` bodies and URL query strings.

use super::{Styled, TokenKind};

/// Percent-decodes each `key=value` pair of a form body or query string.
/// Empty pairs are skipped and malformed escapes are kept as written.
pub(in crate::ui) fn pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(key), decode(value))
        })
        .collect()
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
pub(in crate::ui) fn visible(text: &str) -> String {
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

fn decode(text: &str) -> String {
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let Some((&first, tail)) = rest.split_first() {
        let escaped = match (first, tail) {
            (b'%', [high, low, ..]) => hex(*high).zip(hex(*low)).map(|(h, l)| h << 4 | l),
            _ => None,
        };
        if let Some(byte) = escaped {
            bytes.push(byte);
            rest = tail.get(2..).unwrap_or_default();
        } else {
            bytes.push(if first == b'+' { b' ' } else { first });
            rest = tail;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
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
