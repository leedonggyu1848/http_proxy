//! The one place that decides which [`BodyFormat`] a body is.
//!
//! The Body tab and the Map detail panel both classify bodies here, so the
//! same body never reads as JSON in one and as text in the other. Callers
//! still parse the body for their own purpose; this module only says what
//! to parse it as.
//!
//! The `Content-Type` decides when it names a known format. When it is
//! missing, not a valid media type, or `text/plain`, the leading bytes
//! decide: a body that starts with `{` or `[` is JSON by [`Basis::Sniffed`].
//! A sniffed guess is confirmed only by the whole body parsing as JSON
//! ([`settle`]); a declared format is kept even when the body is malformed,
//! so the view can say why it could not be read.

use super::format::BodyFormat;
use crate::exchange::MediaType;

/// Most leading bytes read to sniff a body.
pub const SNIFF_BYTES: usize = 512;

/// What a body was detected as and on what grounds.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Detection {
    pub format: BodyFormat,
    pub basis: Basis,
}

/// What a [`Detection`] rests on.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Basis {
    /// The `Content-Type` named the format; the body bytes were not read, so
    /// the result holds for any body with this header.
    ContentType,
    /// The `Content-Type` did not name a format and the leading bytes were
    /// read instead.
    Sniffed,
}

/// Detects the format from the `Content-Type` alone, or `None` when it does
/// not name one and the body has to be sniffed.
#[must_use]
pub fn from_content_type(media_type: Option<&MediaType>) -> Option<Detection> {
    let media_type = media_type?;
    let format = match media_type.essence() {
        _ if media_type.is_json() => BodyFormat::Json,
        "text/plain" => return None,
        "text/css" => BodyFormat::Css,
        "application/x-www-form-urlencoded" => BodyFormat::Form,
        "multipart/form-data" => BodyFormat::Multipart,
        "text/html" | "application/xhtml+xml" => BodyFormat::Markup,
        "application/xml" | "text/xml" => BodyFormat::Xml,
        _ if media_type.has_xml_suffix() => BodyFormat::Xml,
        "text/javascript"
        | "application/javascript"
        | "application/x-javascript"
        | "application/ecmascript"
        | "text/ecmascript" => BodyFormat::JavaScript,
        _ => BodyFormat::Text,
    };
    Some(Detection {
        format,
        basis: Basis::ContentType,
    })
}

/// Detects the format from the `Content-Type` and the body's leading bytes.
/// Only the first [`SNIFF_BYTES`] of `head` are read.
#[must_use]
pub fn detect(media_type: Option<&MediaType>, head: &[u8]) -> Detection {
    from_content_type(media_type).unwrap_or_else(|| sniff(head))
}

/// The format to show a whole body as: [`detect`], except that a sniffed
/// JSON guess becomes [`BodyFormat::Text`] when `body` does not parse as
/// JSON.
#[must_use]
pub fn settle(media_type: Option<&MediaType>, body: &[u8]) -> BodyFormat {
    let detection = detect(media_type, body);
    match detection {
        Detection {
            format: BodyFormat::Json,
            basis: Basis::Sniffed,
        } if serde_json::from_slice::<serde_json::Value>(body).is_err() => BodyFormat::Text,
        _ => detection.format,
    }
}

fn sniff(head: &[u8]) -> Detection {
    let head = head.get(..SNIFF_BYTES).unwrap_or(head);
    let first = head
        .iter()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
    let format = match first {
        Some(b'{' | b'[') => BodyFormat::Json,
        _ => BodyFormat::Text,
    };
    Detection {
        format,
        basis: Basis::Sniffed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_formats_from_media_type() {
        for (value, expected) in [
            (Some("application/json"), BodyFormat::Json),
            (Some("Text/CSS; charset=utf-8"), BodyFormat::Css),
            (Some("Application/JSON; charset=utf-8"), BodyFormat::Json),
            (Some("application/problem+json"), BodyFormat::Json),
            (Some("application/x-www-form-urlencoded"), BodyFormat::Form),
            (
                Some("multipart/form-data; boundary=B"),
                BodyFormat::Multipart,
            ),
            (Some("text/html; charset=UTF-8"), BodyFormat::Markup),
            (Some("application/xhtml+xml"), BodyFormat::Markup),
            (Some("application/xml"), BodyFormat::Xml),
            (Some("text/xml; charset=utf-8"), BodyFormat::Xml),
            (Some("application/atom+xml"), BodyFormat::Xml),
            (Some("application/soap+xml"), BodyFormat::Xml),
            (Some("image/svg+xml"), BodyFormat::Xml),
            (Some("text/plain"), BodyFormat::Text),
            (Some("application/javascript"), BodyFormat::JavaScript),
            (
                Some("text/javascript; charset=utf-8"),
                BodyFormat::JavaScript,
            ),
            (Some("application/x-javascript"), BodyFormat::JavaScript),
            (Some("application/ecmascript"), BodyFormat::JavaScript),
            (Some("Text/ECMAScript"), BodyFormat::JavaScript),
            (Some("jsonish"), BodyFormat::Text),
            (None, BodyFormat::Text),
        ] {
            let media_type = value.and_then(MediaType::parse);
            assert_eq!(
                detect(media_type.as_ref(), b"").format,
                expected,
                "{value:?}"
            );
        }
    }

    #[test]
    fn names_the_basis() {
        let json = MediaType::parse("application/json");
        let plain = MediaType::parse("text/plain; charset=utf-8");
        assert_eq!(detect(json.as_ref(), b"not json").basis, Basis::ContentType);
        assert_eq!(detect(plain.as_ref(), b"{}").basis, Basis::Sniffed);
        assert_eq!(detect(None, b"hello").basis, Basis::Sniffed);
        assert_eq!(from_content_type(plain.as_ref()), None);
        assert_eq!(from_content_type(None), None);
    }

    #[test]
    fn sniffs_json_objects_and_arrays_only() {
        for (head, expected) in [
            (&b"{\"a\":1}"[..], BodyFormat::Json),
            (b" \r\n\t[1,2]", BodyFormat::Json),
            (b"{not json", BodyFormat::Json),
            (b"42", BodyFormat::Text),
            (b"\"ok\"", BodyFormat::Text),
            (b"<a/>", BodyFormat::Text),
            (b"a=1&b=2", BodyFormat::Text),
            (b"", BodyFormat::Text),
        ] {
            assert_eq!(detect(None, head).format, expected, "{head:?}");
        }
        let late = [vec![b' '; SNIFF_BYTES], b"{}".to_vec()].concat();
        assert_eq!(detect(None, &late).format, BodyFormat::Text);
    }

    #[test]
    fn settles_sniffed_json_only_when_it_parses() {
        let json = MediaType::parse("application/json");
        assert_eq!(settle(None, br#"{"a":1}"#), BodyFormat::Json);
        assert_eq!(settle(None, b"{not json"), BodyFormat::Text);
        assert_eq!(settle(None, br#"{"a":"#), BodyFormat::Text);
        assert_eq!(settle(json.as_ref(), b"{not json"), BodyFormat::Json);
    }
}
