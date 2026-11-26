//! The text view of one captured body: `Content-Encoding` removed, the
//! format settled, and the text prepared for Pretty, Raw, or Tree, with the
//! notices that say how much of the body the view covers.
//!
//! The desktop body tab and the command line share this module so both show
//! the same text and the same notices for the same capture.

use http_inspector_exchange::capture::BODY_PREVIEW_BYTES;
use http_inspector_exchange::{BodyLifecycle, BodyRetention, BodySnapshot, MediaType};

use crate::decode::{self, BodyText};
use crate::detect;
use crate::format::{self, BodyFormat, FormatError, Notice, Styled, ViewMode, tree::TreeModel};

/// How much of a body the retained bytes cover, and why not all of it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BodyCapture {
    /// The retained bytes are the whole body.
    Whole,
    /// The body was longer than the preview keeps.
    Truncated,
    /// More of the body may still arrive.
    Streaming,
    /// The body stopped before its end: the sender interrupted it or the
    /// receiver cancelled it.
    Ended,
}

impl BodyCapture {
    /// Reads the capture state of a body snapshot. A preview cut at the
    /// retention limit stays cut whatever happens to the stream, so
    /// truncation wins over the lifecycle.
    #[must_use]
    pub fn of(snapshot: &BodySnapshot) -> Self {
        if snapshot.is_complete() {
            return Self::Whole;
        }
        match (snapshot.retention(), snapshot.lifecycle()) {
            (BodyRetention::Prefix, BodyLifecycle::Streaming) => Self::Streaming,
            (BodyRetention::Prefix, BodyLifecycle::Interrupted | BodyLifecycle::Cancelled) => {
                Self::Ended
            }
            (BodyRetention::Truncated | BodyRetention::Omitted, _)
            | (BodyRetention::Prefix, BodyLifecycle::Complete) => Self::Truncated,
        }
    }

    /// Reports whether the retained bytes are the whole body.
    #[must_use]
    pub const fn is_whole(self) -> bool {
        matches!(self, Self::Whole)
    }

    /// Stable lowercase name, used by machine-readable output.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Whole => "whole",
            Self::Truncated => "truncated",
            Self::Streaming => "streaming",
            Self::Ended => "ended",
        }
    }

    /// Why the view ends before the body does; `None` for a whole body.
    #[must_use]
    pub fn notice(self) -> Option<Notice> {
        let text = match self {
            Self::Whole => return None,
            Self::Truncated => body_truncated(BODY_PREVIEW_BYTES / 1024),
            Self::Streaming => BODY_STREAMING.to_owned(),
            Self::Ended => BODY_ENDED.to_owned(),
        };
        Some(Notice::warning(text))
    }
}

/// Body notice: the preview kept only the first `kib` KiB.
#[must_use]
pub fn body_truncated(kib: usize) -> String {
    format!("Showing the first {kib} KiB captured")
}

/// Body notice: the body is still arriving.
pub const BODY_STREAMING: &str = "Still receiving; showing what arrived";

/// Body notice: the body stopped before its end.
pub const BODY_ENDED: &str = "Ended early; showing what arrived";

/// A captured body preview to prepare.
#[derive(Clone, Copy, Debug)]
pub struct BodyInput<'a> {
    /// Retained bytes, still `Content-Encoding` encoded.
    pub bytes: &'a [u8],
    /// `Content-Encoding` tokens in the order they were applied.
    pub encodings: &'a [&'a str],
    /// The declared media type, if any.
    pub media_type: Option<&'a MediaType>,
    /// How much of the body the bytes cover.
    pub capture: BodyCapture,
}

/// Body text with `Content-Encoding` removed and its format settled.
#[derive(Debug, Eq, PartialEq)]
pub struct DecodedBody {
    /// Decoded text, cut to the preview limit.
    pub text: String,
    /// Format detected from the media type and the text.
    pub format: BodyFormat,
    /// The decoding used and why the text may end early, in display order.
    pub notices: Vec<Notice>,
}

/// Removes `Content-Encoding` and settles the format. The error explains why
/// no text can be shown.
pub fn decode_body(input: &BodyInput<'_>) -> Result<DecodedBody, String> {
    let (text, decoding) =
        match decode::decode(input.bytes, input.encodings, input.capture.is_whole()) {
            BodyText::Ready { text, notice } => (text, notice),
            BodyText::Unavailable(message) => return Err(message),
        };
    let format = detect::settle(input.media_type, text.as_bytes());
    let notices = decoding
        .map(Notice::info)
        .into_iter()
        .chain(input.capture.notice())
        .collect();
    Ok(DecodedBody {
        text,
        format,
        notices,
    })
}

/// Notice shown when Tree was asked for and Pretty is shown instead.
/// `error` is the reason the tree could not be read, or `None` when the
/// format has no tree.
#[must_use]
pub fn tree_fallback_notice(format: BodyFormat, error: Option<&FormatError>) -> Notice {
    error.map_or_else(
        || {
            Notice::warning(format!(
                "Tree view is not available for {}; showing Pretty.",
                format.label()
            ))
        },
        |reason| Notice::warning(format!("Showing Pretty: could not read a tree: {reason}.")),
    )
}

/// Body text formatted for Pretty or Raw.
#[derive(Debug, Eq, PartialEq)]
pub struct TextView {
    /// Settled body format.
    pub format: BodyFormat,
    /// Everything the reader should know about the view, in display order.
    pub notices: Vec<Notice>,
    /// Formatted text and its token kinds.
    pub styled: Styled,
}

/// Formats decoded text for `mode`. Tree has no text of its own and is
/// formatted as Pretty.
#[must_use]
pub fn text_view(decoded: DecodedBody, mode: ViewMode) -> TextView {
    let DecodedBody {
        text,
        format,
        mut notices,
    } = decoded;
    let formatted = format::format(format, mode, &text);
    notices.extend(formatted.notices);
    TextView {
        format,
        notices,
        styled: formatted.styled,
    }
}

/// A body prepared for one view mode.
#[derive(Debug)]
pub enum BodyView {
    /// No text can be shown, with the reason.
    Unavailable(String),
    /// A JSON or XML tree.
    Tree {
        /// Settled body format.
        format: BodyFormat,
        /// Everything the reader should know about the view, in display order.
        notices: Vec<Notice>,
        /// The parsed tree.
        model: TreeModel,
    },
    /// Pretty or Raw text, also used when Tree is not available.
    Text(TextView),
}

/// Prepares a body for `mode` in one step. The desktop body tab runs the
/// same steps with its own tree cache.
#[must_use]
pub fn prepare(input: &BodyInput<'_>, mode: ViewMode) -> BodyView {
    let mut decoded = match decode_body(input) {
        Ok(decoded) => decoded,
        Err(message) => return BodyView::Unavailable(message),
    };
    if mode == ViewMode::Tree {
        match format::tree::build(decoded.format, &decoded.text) {
            Some(Ok(model)) => {
                let mut notices = decoded.notices;
                notices.extend(model.incomplete.as_deref().map(Notice::warning));
                return BodyView::Tree {
                    format: decoded.format,
                    notices,
                    model,
                };
            }
            Some(Err(reason)) => decoded
                .notices
                .push(tree_fallback_notice(decoded.format, Some(&reason))),
            None => decoded
                .notices
                .push(tree_fallback_notice(decoded.format, None)),
        }
    }
    BodyView::Text(text_view(decoded, mode))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::NoticeLevel;

    type TestResult = Result<(), String>;

    fn json() -> Option<MediaType> {
        MediaType::parse("application/json")
    }

    fn text(view: BodyView) -> Result<TextView, String> {
        match view {
            BodyView::Text(view) => Ok(view),
            other => Err(format!("expected text, got {other:?}")),
        }
    }

    #[test]
    fn pretty_formats_and_reports_partial_captures() -> TestResult {
        let media = json();
        let input = BodyInput {
            bytes: br#"{"a":[1,2]}"#,
            encodings: &[],
            media_type: media.as_ref(),
            capture: BodyCapture::Streaming,
        };
        let view = text(prepare(&input, ViewMode::Pretty))?;
        assert_eq!(view.format, BodyFormat::Json);
        assert!(view.styled.text().contains('\n'));
        assert_eq!(view.notices, vec![Notice::warning(BODY_STREAMING)]);
        Ok(())
    }

    #[test]
    fn tree_falls_back_to_pretty_with_a_reason() -> TestResult {
        let input = BodyInput {
            bytes: b"plain words",
            encodings: &[],
            media_type: None,
            capture: BodyCapture::Whole,
        };
        let view = text(prepare(&input, ViewMode::Tree))?;
        assert!(matches!(
            view.notices.as_slice(),
            [Notice { level: NoticeLevel::Warning, text }] if text.starts_with("Tree view is not available")
        ));
        Ok(())
    }

    #[test]
    fn unsupported_encoding_is_unavailable() {
        let input = BodyInput {
            bytes: b"abc",
            encodings: &["compress"],
            media_type: None,
            capture: BodyCapture::Whole,
        };
        assert!(matches!(
            prepare(&input, ViewMode::Raw),
            BodyView::Unavailable(message) if message.contains("compress")
        ));
    }

    #[test]
    fn json_trees_keep_their_paths() -> TestResult {
        let media = json();
        let input = BodyInput {
            bytes: br#"{"items":[{"id":7}]}"#,
            encodings: &[],
            media_type: media.as_ref(),
            capture: BodyCapture::Whole,
        };
        let BodyView::Tree { model, notices, .. } = prepare(&input, ViewMode::Tree) else {
            return Err("expected a tree".to_owned());
        };
        assert_eq!(notices, []);
        assert!(
            model
                .nodes()
                .iter()
                .any(|node| node.path.as_deref() == Some("$.items[0].id"))
        );
        Ok(())
    }

    #[test]
    fn reads_the_capture_reason_from_the_snapshot() -> Result<(), &'static str> {
        use http_inspector_exchange::BodyLifecycle::{Cancelled, Complete, Interrupted, Streaming};
        for (lifecycle, limit, expected) in [
            (Complete, 3, BodyCapture::Whole),
            (Streaming, 3, BodyCapture::Streaming),
            (Interrupted, 3, BodyCapture::Ended),
            (Cancelled, 3, BodyCapture::Ended),
            (Complete, 2, BodyCapture::Truncated),
            (Streaming, 2, BodyCapture::Truncated),
            (Interrupted, 2, BodyCapture::Truncated),
        ] {
            let body = http_inspector_exchange::BodyPreviewWriter::new(limit);
            body.observe(b"abc");
            match lifecycle {
                Complete => body.complete(),
                Streaming => {}
                Interrupted => body.interrupt(),
                Cancelled => body.cancel(),
            }
            let snapshot = body.preview().snapshot().ok_or("missing snapshot")?;
            assert_eq!(BodyCapture::of(&snapshot), expected, "{snapshot:?}");
        }
        Ok(())
    }
}
