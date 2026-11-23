//! HAR `content` and `postData` built from in-memory body previews.
//!
//! A body is written as text only when the retained bytes can stand for it:
//! an unencoded body, whole or cut, or a whole body whose `Content-Encoding`
//! could be removed. Otherwise the text is left out. Every body that is not
//! the whole body gets a `comment` saying why.

use http_proxy::{BodyLifecycle, BodyPreview, BodyRetention, BodySnapshot, HeaderPreview};
use serde_json::{Map, Value, json};

use http_proxy::content::decode as body_view;

/// Most decoded bytes one compressed body may expand to in the export.
const DECODED_LIMIT: usize = 16 * 1024 * 1024;
/// HAR's `mimeType` when the message has no `Content-Type`, as Chrome writes it.
const UNKNOWN_MIME_TYPE: &str = "x-unknown";
pub(super) const OMITTED_BY_SETTINGS: &str = "Body omitted by the export settings.";

/// Body text ready for HAR, or why it was left out.
#[derive(Debug, Eq, PartialEq)]
enum BodyText {
    Text {
        text: String,
        base64: bool,
        /// Decoded length, which HAR `content.size` reports.
        size: u64,
    },
    Omitted {
        /// Observed length, the best known size when the text is left out.
        size: u64,
        reason: String,
    },
}

/// Returns the first `Content-Type` value, or HAR's unknown marker.
pub(super) fn mime_type(headers: &HeaderPreview) -> String {
    headers
        .get("content-type")
        .next()
        .unwrap_or(UNKNOWN_MIME_TYPE)
        .to_owned()
}

/// Returns observed body bytes, or `-1` when the proxy never read the body.
pub(super) fn body_size(body: Option<&BodyPreview>) -> Value {
    body.map_or_else(|| json!(-1), |body| json!(body.observed_bytes()))
}

/// Builds the response `content` object.
pub(super) fn response_content(
    body: &BodyPreview,
    headers: &HeaderPreview,
    include_body: bool,
) -> Value {
    let mut content = Map::new();
    let _ = content.insert("mimeType".into(), json!(mime_type(headers)));
    if !include_body {
        let _ = content.insert("size".into(), json!(body.observed_bytes()));
        let _ = content.insert("comment".into(), json!(OMITTED_BY_SETTINGS));
        return Value::Object(content);
    }
    let snapshot = body.snapshot();
    match body_text(snapshot.as_ref(), &encodings(headers)) {
        BodyText::Text { text, base64, size } => {
            let _ = content.insert("size".into(), json!(size));
            let _ = content.insert("text".into(), json!(text));
            if base64 {
                let _ = content.insert("encoding".into(), json!("base64"));
            }
            let compression = body.observed_bytes().checked_sub(size).filter(|_| {
                snapshot.as_ref().is_some_and(|snapshot| {
                    snapshot.lifecycle() == BodyLifecycle::Complete
                        && !encodings(headers).is_empty()
                })
            });
            if let Some(compression) = compression {
                let _ = content.insert("compression".into(), json!(compression));
            }
        }
        BodyText::Omitted { size, reason } => {
            let _ = content.insert("size".into(), json!(size));
            let _ = content.insert("comment".into(), json!(reason));
        }
    }
    if let Some(note) = snapshot.as_ref().and_then(partial_note) {
        append_comment(&mut content, &note);
    }
    Value::Object(content)
}

/// Builds the request `postData`, or `None` when the export leaves bodies
/// out or the request had no body bytes.
pub(super) fn post_data(
    body: Option<&BodyPreview>,
    headers: &HeaderPreview,
    include_body: bool,
) -> Option<Value> {
    let body = body.filter(|body| include_body && body.observed_bytes() > 0)?;
    let mut data = Map::new();
    let _ = data.insert("mimeType".into(), json!(mime_type(headers)));
    let snapshot = body.snapshot();
    match body_text(snapshot.as_ref(), &encodings(headers)) {
        BodyText::Text {
            text,
            base64: false,
            ..
        } => {
            let _ = data.insert("text".into(), json!(text));
        }
        BodyText::Text { base64: true, .. } => {
            let _ = data.insert("text".into(), json!(""));
            let _ = data.insert(
                "comment".into(),
                json!("Binary request body omitted: HAR 1.2 postData holds text only."),
            );
        }
        BodyText::Omitted { reason, .. } => {
            let _ = data.insert("text".into(), json!(""));
            let _ = data.insert("comment".into(), json!(reason));
        }
    }
    if let Some(note) = snapshot.as_ref().and_then(partial_note) {
        append_comment(&mut data, &note);
    }
    Some(Value::Object(data))
}

fn encodings(headers: &HeaderPreview) -> Vec<&str> {
    headers.get("content-encoding").collect()
}

fn is_encoded(encodings: &[&str]) -> bool {
    encodings
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|encoding| !encoding.is_empty() && !encoding.eq_ignore_ascii_case("identity"))
}

fn body_text(snapshot: Option<&BodySnapshot>, encodings: &[&str]) -> BodyText {
    let Some(snapshot) = snapshot else {
        return BodyText::Omitted {
            size: 0,
            reason: "Body omitted: the preview was being written when the export ran.".into(),
        };
    };
    let observed = snapshot.observed_bytes();
    if snapshot.retention() == BodyRetention::Omitted {
        return BodyText::Omitted {
            size: observed,
            reason: "Body omitted: the proxy skipped the preview to avoid delaying forwarding."
                .into(),
        };
    }
    let whole = snapshot.lifecycle() == BodyLifecycle::Complete
        && snapshot.retention() == BodyRetention::Prefix;
    if !is_encoded(encodings) {
        let (text, base64) = encode_text(snapshot.bytes(), !whole);
        return BodyText::Text {
            text,
            base64,
            size: observed,
        };
    }
    if !whole {
        return BodyText::Omitted {
            size: observed,
            reason: format!(
                "Body omitted: only part of the {} body was retained, and a partial compressed body cannot be decoded reliably. size is the encoded length.",
                encodings.join(", ")
            ),
        };
    }
    match body_view::decode_complete(snapshot.bytes(), encodings, DECODED_LIMIT) {
        Ok(decoded) => {
            let (text, base64) = encode_text(&decoded, false);
            BodyText::Text {
                text,
                base64,
                size: u64::try_from(decoded.len()).unwrap_or(u64::MAX),
            }
        }
        Err(error) => BodyText::Omitted {
            size: observed,
            reason: format!("Body omitted: {error}. size is the encoded length."),
        },
    }
}

/// Explains a body whose retained bytes are not the whole body, or `None`
/// for a whole body.
fn partial_note(snapshot: &BodySnapshot) -> Option<String> {
    let mut parts = Vec::new();
    if snapshot.retention() == BodyRetention::Truncated {
        parts.push(format!(
            "Truncated: only the first {} of {} observed bytes were kept in memory",
            snapshot.bytes().len(),
            snapshot.observed_bytes()
        ));
    }
    match snapshot.lifecycle() {
        BodyLifecycle::Complete => {}
        BodyLifecycle::Streaming => parts.push("Partial: still streaming when exported".into()),
        BodyLifecycle::Interrupted => {
            parts.push("Partial: the sender stopped before the end of the body".into());
        }
        BodyLifecycle::Cancelled => {
            parts.push("Partial: the receiver stopped reading before the end of the body".into());
        }
    }
    (!parts.is_empty()).then(|| format!("{}.", parts.join(". ")))
}

fn append_comment(object: &mut Map<String, Value>, note: &str) {
    let comment = object
        .get("comment")
        .and_then(Value::as_str)
        .map_or_else(|| note.to_owned(), |existing| format!("{existing} {note}"));
    let _ = object.insert("comment".into(), json!(comment));
}

/// Returns UTF-8 text as it is, or base64 for other bytes. A prefix may end
/// inside a character, so its incomplete last character is dropped first.
fn encode_text(bytes: &[u8], prefix: bool) -> (String, bool) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), false),
        Err(error) if prefix && error.error_len().is_none() => {
            let (valid, _) = bytes.split_at(error.valid_up_to());
            (String::from_utf8_lossy(valid).into_owned(), false)
        }
        Err(_) => (base64(bytes), true),
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let symbol = |index: u32| {
        let index = usize::try_from(index & 63).unwrap_or_default();
        char::from(ALPHABET.get(index).copied().unwrap_or(b'A'))
    };
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let [first, second, third] = [0, 1, 2].map(|index| chunk.get(index).copied());
        let group = (u32::from(first.unwrap_or_default()) << 16)
            | (u32::from(second.unwrap_or_default()) << 8)
            | u32::from(third.unwrap_or_default());
        text.push(symbol(group >> 18));
        text.push(symbol(group >> 12));
        text.push(if second.is_some() {
            symbol(group >> 6)
        } else {
            '='
        });
        text.push(if third.is_some() { symbol(group) } else { '=' });
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xff, 0xfe, 0x00, 0x80]), "//4AgA==");
    }

    #[test]
    fn text_keeps_utf8_and_trims_a_cut_character() {
        assert_eq!(
            encode_text("héllo".as_bytes(), false),
            ("héllo".into(), false)
        );
        let cut = "héllo".as_bytes().split_at(2).0;
        assert_eq!(encode_text(cut, true), ("h".into(), false));
        assert_eq!(encode_text(cut, false), (base64(cut), true));
        assert!(encode_text(&[0xff, 0x00], true).1);
    }

    #[test]
    fn identity_is_not_an_encoding() {
        assert!(!is_encoded(&[]));
        assert!(!is_encoded(&["identity", " "]));
        assert!(is_encoded(&["identity, gzip"]));
    }
}
