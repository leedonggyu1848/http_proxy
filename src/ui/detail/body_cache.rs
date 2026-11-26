//! Prepares captured text for display and checks which body changes need a rebuild.

use http_inspector_analysis::search::Query;
use http_inspector_content::decode::{self, BodyText};
use http_inspector_content::detect;
use http_inspector_content::format::{
    self as body_format, BodyFormat, FormatError, Notice, Styled, ViewMode,
    tree::{self, TreeModel},
};
use http_inspector_exchange::{BodyLifecycle, BodyRetention, BodySide, BodySnapshot, MediaType};
use std::sync::Arc;

use crate::ui::i18n::{Language, domain};

/// A captured body preview to show.
pub(super) struct BodySource<'a> {
    /// [`http_inspector_exchange::RecordId::value`] of the exchange the body belongs to.
    pub(super) record: u64,
    pub(super) side: BodySide,
    pub(super) bytes: &'a [u8],
    pub(super) encodings: &'a [&'a str],
    pub(super) media_type: Option<&'a MediaType>,
    /// How much of the body the retained bytes cover.
    pub(super) capture: Capture,
    /// The search whose matches are marked, when it covers this body.
    pub(super) search: Option<&'a Query>,
}

/// How much of a body the retained bytes cover, and why not all of it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Capture {
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

impl Capture {
    /// Reads the capture state of a body snapshot. A preview cut at the
    /// retention limit stays cut whatever happens to the stream, so
    /// truncation wins over the lifecycle.
    pub(super) fn of(snapshot: &BodySnapshot) -> Self {
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

    const fn is_whole(self) -> bool {
        matches!(self, Self::Whole)
    }

    /// Why the view ends before the body does; `None` for a whole body.
    fn notice(self, lang: Language) -> Option<Notice> {
        let preview_kib = http_inspector_exchange::capture::BODY_PREVIEW_BYTES / 1024;
        let text = match self {
            Self::Whole => return None,
            Self::Truncated => match lang {
                Language::English => format!(
                    "Only part of the body was captured (at most {preview_kib} KiB is kept); the view ends where the capture ends."
                ),
                Language::Korean => format!(
                    "본문의 일부만 캡처되었습니다(최대 {preview_kib} KiB 보관). 캡처된 지점까지만 표시됩니다."
                ),
            },
            Self::Streaming => lang
                .pick(
                    "The body is still being received; the view shows what has arrived so far.",
                    "본문을 아직 수신 중입니다. 지금까지 도착한 내용만 표시됩니다.",
                )
                .to_owned(),
            Self::Ended => lang
                .pick(
                    "The body ended early (interrupted or cancelled); the view shows what was received.",
                    "본문이 중간에 종료되었습니다(중단 또는 취소). 수신된 내용만 표시됩니다.",
                )
                .to_owned(),
        };
        Some(Notice::warning(text))
    }
}

/// Previews only grow, so the retained length identifies one record's body bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct BodyKey {
    record: u64,
    retained: usize,
    capture: Capture,
}

#[derive(PartialEq, Eq)]
struct Key {
    body: BodyKey,
    mode: ViewMode,
    search: Option<u64>,
    lang: Language,
}

pub(super) enum PreparedBody {
    Unavailable(String),
    Tree {
        format: BodyFormat,
        notices: Vec<Notice>,
        model: Arc<TreeModel>,
    },
    Text {
        format: BodyFormat,
        notices: Vec<Notice>,
        styled: Styled,
    },
}

pub(super) struct BodyUpdate {
    pub(super) content: PreparedBody,
    pub(super) reset_selection: bool,
}

struct TreeEntry {
    key: BodyKey,
    model: Result<Arc<TreeModel>, FormatError>,
}

/// Tracks the last rendered view of one body side. The renderer supplies
/// its style key, so style changes follow the same invalidation rule.
pub(super) struct BodyCache<S> {
    shown: Option<(Key, S)>,
    tree: Option<TreeEntry>,
    #[cfg(test)]
    pub(super) builds: u64,
    #[cfg(test)]
    pub(super) tree_builds: u64,
}

impl<S> Default for BodyCache<S> {
    fn default() -> Self {
        Self {
            shown: None,
            tree: None,
            #[cfg(test)]
            builds: 0,
            #[cfg(test)]
            tree_builds: 0,
        }
    }
}

impl<S: Clone + PartialEq> BodyCache<S> {
    pub(super) fn update(
        &mut self,
        source: &BodySource<'_>,
        mode: ViewMode,
        style: &S,
        lang: Language,
    ) -> Option<BodyUpdate> {
        let key = Key {
            body: BodyKey {
                record: source.record,
                retained: source.bytes.len(),
                capture: source.capture,
            },
            mode,
            search: source.search.map(|query| query.generation),
            lang,
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|(cached, cached_style)| *cached == key && cached_style == style)
        {
            return None;
        }
        let reset_selection = self.shown.as_ref().is_some_and(|(cached, _)| {
            cached.body.record != key.body.record || cached.mode != key.mode
        });
        #[cfg(test)]
        {
            self.builds += 1;
        }
        let content = self.prepare(source, key.body, mode, lang);
        self.shown = Some((key, style.clone()));
        Some(BodyUpdate {
            content,
            reset_selection,
        })
    }

    fn prepare(
        &mut self,
        source: &BodySource<'_>,
        key: BodyKey,
        mode: ViewMode,
        lang: Language,
    ) -> PreparedBody {
        let (text, decoding) =
            match decode::decode(source.bytes, source.encodings, source.capture.is_whole()) {
                BodyText::Ready { text, notice } => (text, notice),
                BodyText::Unavailable(message) => {
                    return PreparedBody::Unavailable(domain::localize_content_notice(
                        lang, &message,
                    ));
                }
            };
        let format = detect::settle(source.media_type, text.as_bytes());
        let mut notices = capture_notices(decoding, source.capture, lang);
        if mode == ViewMode::Tree {
            match self.tree(key, format, &text) {
                Some(Ok(model)) => {
                    notices.extend(
                        model
                            .incomplete
                            .as_deref()
                            .map(|msg| Notice::warning(domain::localize_content_notice(lang, msg))),
                    );
                    if source.search.is_some() {
                        notices.push(Notice::info(lang.pick(
                            "Search matches are marked in Pretty and Raw.",
                            "검색 일치 항목은 정렬(Pretty) 및 원문(Raw) 보기에서 표시됩니다.",
                        )));
                    }
                    return PreparedBody::Tree {
                        format,
                        notices,
                        model: Arc::clone(model),
                    };
                }
                Some(Err(reason)) => {
                    let message = match lang {
                        Language::English => {
                            format!("Showing Pretty: could not read a tree: {reason}.")
                        }
                        Language::Korean => {
                            format!("정렬 보기로 표시: 트리를 읽을 수 없습니다({reason}).")
                        }
                    };
                    notices.push(Notice::warning(message));
                }
                None => {
                    let format_name = domain::body_format(lang, format);
                    let message = match lang {
                        Language::English => {
                            format!("Tree view is not available for {format_name}; showing Pretty.")
                        }
                        Language::Korean => format!(
                            "{format_name} 형식에는 트리 보기를 사용할 수 없어 정렬 보기로 표시합니다."
                        ),
                    };
                    notices.push(Notice::warning(message));
                }
            }
        }
        let formatted = body_format::format(format, mode, &text);
        notices.extend(formatted.notices.into_iter().map(|notice| Notice {
            level: notice.level,
            text: domain::localize_content_notice(lang, &notice.text),
        }));
        PreparedBody::Text {
            format,
            notices,
            styled: formatted.styled,
        }
    }

    // A style, view mode or search change leaves the parsed tree reusable.
    fn tree(
        &mut self,
        key: BodyKey,
        format: BodyFormat,
        text: &str,
    ) -> Option<Result<&Arc<TreeModel>, &FormatError>> {
        if self.tree.as_ref().is_none_or(|entry| entry.key != key) {
            let model = tree::build(format, text)?.map(Arc::new);
            #[cfg(test)]
            {
                self.tree_builds += 1;
            }
            self.tree = Some(TreeEntry { key, model });
        }
        Some(self.tree.as_ref()?.model.as_ref())
    }
}

/// What every format says about how the body was captured: the decoding
/// used, and why the view ends early when only part of the body was kept.
fn capture_notices(decoding: Option<String>, capture: Capture, lang: Language) -> Vec<Notice> {
    decoding
        .map(|text| Notice::info(domain::localize_content_notice(lang, &text)))
        .into_iter()
        .chain(capture.notice(lang))
        .collect()
}

#[cfg(test)]
pub(super) mod fixtures {
    use super::*;

    pub(in crate::ui::detail) fn partial_capture() -> Notice {
        Notice::warning(
            "Only part of the body was captured (at most 8 KiB is kept); the view ends where the capture ends.",
        )
    }

    /// Parses a test media type that lives for the whole test run.
    pub(in crate::ui::detail) fn media(value: &str) -> Option<&'static MediaType> {
        MediaType::parse(value).map(|media_type| &*Box::leak(Box::new(media_type)))
    }

    pub(in crate::ui::detail) fn source(record: u64, bytes: &[u8]) -> BodySource<'_> {
        BodySource {
            record,
            side: BodySide::Response,
            bytes,
            encodings: &[],
            media_type: media("application/json"),
            capture: Capture::Whole,
            search: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{media, partial_capture, source};
    use super::*;
    use http_inspector_content::format::NoticeLevel;

    fn notices(update: Option<BodyUpdate>) -> Vec<Notice> {
        match update.map(|update| update.content) {
            Some(PreparedBody::Text { notices, .. } | PreparedBody::Tree { notices, .. }) => {
                notices
            }
            _ => Vec::new(),
        }
    }

    #[test]
    fn rebuilds_for_view_inputs_and_resets_selection_on_record_or_mode_change()
    -> Result<(), http_inspector_analysis::search::MatcherError> {
        use http_inspector_analysis::search::{MatchOptions, Matcher, SearchScope};

        let mut cache = BodyCache::default();
        let body = source(1, b"abc");
        let query = Query {
            generation: 1,
            matcher: Matcher::new("abc", MatchOptions::default())?,
            scope: SearchScope::All,
        };
        let longer = BodySource {
            bytes: b"abcd",
            ..body
        };
        let partial = BodySource {
            capture: Capture::Streaming,
            ..longer
        };
        let searching = BodySource {
            search: Some(&query),
            ..partial
        };
        let other_record = BodySource {
            record: 2,
            ..searching
        };
        for (source, mode, style, reset) in [
            (&body, ViewMode::Pretty, 1, Some(false)),
            (&body, ViewMode::Pretty, 1, None),
            (&longer, ViewMode::Pretty, 1, Some(false)),
            (&partial, ViewMode::Pretty, 1, Some(false)),
            (&searching, ViewMode::Pretty, 1, Some(false)),
            (&searching, ViewMode::Pretty, 2, Some(false)),
            (&searching, ViewMode::Raw, 2, Some(true)),
            (&other_record, ViewMode::Raw, 2, Some(true)),
        ] {
            assert_eq!(
                cache
                    .update(source, mode, &style, Language::English)
                    .map(|update| update.reset_selection),
                reset
            );
        }
        Ok(())
    }

    #[test]
    fn complete_compressed_css_uses_decoder_notice_without_incomplete_warning()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write;

        let text = format!("{}{}", "a{color:red}".repeat(600), " ".repeat(2000));
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(text.as_bytes())?;
        let compressed = encoder.finish()?;
        let css = BodySource {
            media_type: media("text/css"),
            encodings: &["gzip"],
            ..source(1, &compressed)
        };
        let mut cache = BodyCache::<()>::default();
        let Some(BodyUpdate {
            content: PreparedBody::Text {
                notices, styled, ..
            },
            ..
        }) = cache.update(&css, ViewMode::Pretty, &(), Language::English)
        else {
            return Err("expected prepared CSS content".into());
        };
        assert_eq!(
            notices,
            [Notice::info(
                "Content-Encoding: gzip · first 8 KiB of decoded bytes"
            )]
        );
        assert!(styled.text().contains("color:red"));
        Ok(())
    }

    #[test]
    fn pretty_shows_indented_text_and_raw_the_original() {
        let mut cache = BodyCache::<()>::default();
        let body = br#"{"a":true}"#;
        let shown = |update: Option<BodyUpdate>| match update.map(|update| update.content) {
            Some(PreparedBody::Text { styled, .. }) => styled.text().to_owned(),
            _ => String::new(),
        };
        let body = source(1, body);
        assert_eq!(
            shown(cache.update(&body, ViewMode::Pretty, &(), Language::English)),
            "{\n  \"a\": true\n}"
        );
        assert_eq!(
            shown(cache.update(&body, ViewMode::Raw, &(), Language::English)).as_bytes(),
            body.bytes
        );
    }

    #[test]
    fn tree_mode_builds_an_xml_tree_and_falls_back_for_other_formats() {
        let mut cache = BodyCache::<()>::default();
        let xml = BodySource {
            media_type: media("application/rss+xml"),
            ..source(1, b"<rss><channel><title>A</title></channel></rss>")
        };
        assert!(matches!(
            cache.update(&xml, ViewMode::Tree, &(), Language::English),
            Some(BodyUpdate {
                content: PreparedBody::Tree { .. },
                ..
            })
        ));
        let text = BodySource {
            media_type: media("text/plain"),
            ..source(2, b"plain text")
        };
        let fallback = match cache
            .update(&text, ViewMode::Tree, &(), Language::English)
            .map(|update| update.content)
        {
            Some(PreparedBody::Text {
                notices, styled, ..
            }) => Some((notices, styled.text().to_owned())),
            _ => None,
        };
        assert_eq!(
            fallback,
            Some((
                vec![Notice::warning(
                    "Tree view is not available for Text; showing Pretty."
                )],
                "plain text".to_owned()
            ))
        );
    }

    #[test]
    fn every_format_shows_the_same_partial_capture_notice() {
        for (content_type, body) in [
            ("application/json", &br#"{"a":[1,"#[..]),
            ("text/css", b"a{color:red}"),
            ("application/javascript", b"a();"),
            ("text/html", b"<p>a</p>"),
            ("application/xml", b"<a><b>1</b></a>"),
            ("application/x-www-form-urlencoded", b"a=1"),
            ("text/plain", b"plain"),
        ] {
            for mode in ViewMode::ALL {
                let mut cache = BodyCache::<()>::default();
                let partial = BodySource {
                    media_type: media(content_type),
                    capture: Capture::Truncated,
                    ..source(1, body)
                };
                let shown = notices(cache.update(&partial, mode, &(), Language::English));
                assert_eq!(
                    shown
                        .iter()
                        .filter(|notice| **notice == partial_capture())
                        .count(),
                    1,
                    "{content_type} {mode:?}: {shown:?}"
                );
                let whole = BodySource {
                    capture: Capture::Whole,
                    ..partial
                };
                assert!(
                    !notices(cache.update(&whole, mode, &(), Language::English))
                        .contains(&partial_capture()),
                    "{content_type} {mode:?}"
                );
            }
        }
    }

    #[test]
    fn lists_notices_in_display_order() {
        let mut cache = BodyCache::<()>::default();
        let script = BodySource {
            media_type: media("application/javascript"),
            capture: Capture::Truncated,
            ..source(1, b"f(\"unfinished")
        };
        let shown = notices(cache.update(&script, ViewMode::Tree, &(), Language::English));
        let levels: Vec<_> = shown.iter().map(|notice| notice.level).collect();
        assert_eq!(levels, [NoticeLevel::Warning; 3], "{shown:?}");
        assert_eq!(shown.first(), Some(&partial_capture()));
        assert_eq!(
            shown.get(1),
            Some(&Notice::warning(
                "Tree view is not available for JavaScript; showing Pretty."
            ))
        );
        assert!(
            shown
                .get(2)
                .is_some_and(|notice| notice.text.contains("unfinished string"))
        );

        let xml = BodySource {
            media_type: media("application/xml"),
            capture: Capture::Truncated,
            ..source(2, b"<a><b>1</b>")
        };
        let update = cache.update(&xml, ViewMode::Tree, &(), Language::English);
        assert!(matches!(
            update.as_ref(),
            Some(BodyUpdate {
                content: PreparedBody::Tree { .. },
                ..
            })
        ));
        let shown = notices(update);
        assert_eq!(shown.first(), Some(&partial_capture()));
        assert!(
            shown
                .get(1)
                .is_some_and(|notice| notice.text.starts_with("XML is incomplete")),
            "{shown:?}"
        );
    }

    #[test]
    fn reads_the_capture_reason_from_the_snapshot() -> Result<(), &'static str> {
        use BodyLifecycle::{Cancelled, Complete, Interrupted, Streaming};
        for (lifecycle, limit, expected) in [
            (Complete, 3, Capture::Whole),
            (Streaming, 3, Capture::Streaming),
            (Interrupted, 3, Capture::Ended),
            (Cancelled, 3, Capture::Ended),
            (Complete, 2, Capture::Truncated),
            (Streaming, 2, Capture::Truncated),
            (Interrupted, 2, Capture::Truncated),
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
            assert_eq!(Capture::of(&snapshot), expected, "{snapshot:?}");
        }
        Ok(())
    }

    #[test]
    fn names_why_the_view_ends_early() {
        let mut cache = BodyCache::<()>::default();
        for (record, capture, expected) in [
            (1, Capture::Whole, None),
            (2, Capture::Truncated, Some(partial_capture())),
            (
                3,
                Capture::Streaming,
                Some(Notice::warning(
                    "The body is still being received; the view shows what has arrived so far.",
                )),
            ),
            (
                4,
                Capture::Ended,
                Some(Notice::warning(
                    "The body ended early (interrupted or cancelled); the view shows what was received.",
                )),
            ),
        ] {
            let body = BodySource {
                capture,
                ..source(record, br#"{"a":1}"#)
            };
            assert_eq!(
                notices(cache.update(&body, ViewMode::Pretty, &(), Language::English)),
                expected.into_iter().collect::<Vec<_>>(),
                "{capture:?}"
            );
        }
    }
}
