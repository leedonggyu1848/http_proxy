//! Prepares captured text for display and checks which body changes need a rebuild.

use http_inspector_analysis::search::Query;
use http_inspector_content::format::{
    BodyFormat, FormatError, Notice, Styled, ViewMode,
    tree::{self, TreeModel},
};
use http_inspector_content::view::{self, BodyInput};
use http_inspector_exchange::{BodySide, MediaType};
use std::sync::Arc;

/// How much of a body the retained bytes cover; shared with the command line.
pub(super) use http_inspector_content::view::BodyCapture as Capture;

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

impl BodySource<'_> {
    const fn input(&self) -> BodyInput<'_> {
        BodyInput {
            bytes: self.bytes,
            encodings: self.encodings,
            media_type: self.media_type,
            capture: self.capture,
        }
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
    ) -> Option<BodyUpdate> {
        let key = Key {
            body: BodyKey {
                record: source.record,
                retained: source.bytes.len(),
                capture: source.capture,
            },
            mode,
            search: source.search.map(|query| query.generation),
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
        let content = self.prepare(source, key.body, mode);
        self.shown = Some((key, style.clone()));
        Some(BodyUpdate {
            content,
            reset_selection,
        })
    }

    fn prepare(&mut self, source: &BodySource<'_>, key: BodyKey, mode: ViewMode) -> PreparedBody {
        let mut decoded = match view::decode_body(&source.input()) {
            Ok(decoded) => decoded,
            Err(message) => return PreparedBody::Unavailable(message),
        };
        if mode == ViewMode::Tree {
            match self.tree(key, decoded.format, &decoded.text) {
                Some(Ok(model)) => {
                    let mut notices = decoded.notices;
                    notices.extend(model.incomplete.as_deref().map(Notice::warning));
                    if source.search.is_some() {
                        notices.push(Notice::info("Search matches are marked in Pretty and Raw."));
                    }
                    return PreparedBody::Tree {
                        format: decoded.format,
                        notices,
                        model: Arc::clone(model),
                    };
                }
                Some(Err(reason)) => decoded
                    .notices
                    .push(view::tree_fallback_notice(decoded.format, Some(reason))),
                None => decoded
                    .notices
                    .push(view::tree_fallback_notice(decoded.format, None)),
            }
        }
        let shown = view::text_view(decoded, mode);
        PreparedBody::Text {
            format: shown.format,
            notices: shown.notices,
            styled: shown.styled,
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

#[cfg(test)]
pub(super) mod fixtures {
    use super::*;

    pub(in crate::ui::detail) fn partial_capture() -> Notice {
        Notice::warning("Showing the first 8 KiB captured")
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
            terms: http_inspector_analysis::search::TextTerms::single(Matcher::new(
                "abc",
                MatchOptions::default(),
            )?),
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
                    .update(source, mode, &style)
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
        }) = cache.update(&css, ViewMode::Pretty, &())
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
            shown(cache.update(&body, ViewMode::Pretty, &())),
            "{\n  \"a\": true\n}"
        );
        assert_eq!(
            shown(cache.update(&body, ViewMode::Raw, &())).as_bytes(),
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
            cache.update(&xml, ViewMode::Tree, &()),
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
            .update(&text, ViewMode::Tree, &())
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
                let shown = notices(cache.update(&partial, mode, &()));
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
                    !notices(cache.update(&whole, mode, &())).contains(&partial_capture()),
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
        let shown = notices(cache.update(&script, ViewMode::Tree, &()));
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
        let update = cache.update(&xml, ViewMode::Tree, &());
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
    fn names_why_the_view_ends_early() {
        let mut cache = BodyCache::<()>::default();
        for (record, capture, expected) in [
            (1, Capture::Whole, None),
            (2, Capture::Truncated, Some(partial_capture())),
            (
                3,
                Capture::Streaming,
                Some(Notice::warning("Still receiving; showing what arrived")),
            ),
            (
                4,
                Capture::Ended,
                Some(Notice::warning("Ended early; showing what arrived")),
            ),
        ] {
            let body = BodySource {
                capture,
                ..source(record, br#"{"a":1}"#)
            };
            assert_eq!(
                notices(cache.update(&body, ViewMode::Pretty, &())),
                expected.into_iter().collect::<Vec<_>>(),
                "{capture:?}"
            );
        }
    }

    fn as_shared(content: PreparedBody) -> Result<view::BodyView, String> {
        Ok(match content {
            PreparedBody::Unavailable(message) => view::BodyView::Unavailable(message),
            PreparedBody::Text {
                format,
                notices,
                styled,
            } => view::BodyView::Text(view::TextView {
                format,
                notices,
                styled,
            }),
            PreparedBody::Tree {
                format,
                notices,
                model,
            } => view::BodyView::Tree {
                format,
                notices,
                model: Arc::try_unwrap(model).map_err(|_| "the tree is still shared")?,
            },
        })
    }

    /// The body tab shows the same text and notices as the shared
    /// [`view::prepare`] used by the command line.
    #[test]
    fn body_tab_matches_the_shared_body_view() -> Result<(), String> {
        let fixtures: [(&[u8], &[&str], &str, Capture); 6] = [
            (
                br#"{"items":[{"id":7,"name":"a"}]}"#,
                &[],
                "application/json",
                Capture::Whole,
            ),
            (
                br#"{"items":[1,"#,
                &[],
                "application/json",
                Capture::Streaming,
            ),
            (
                b"<rss><channel><item>x</item></channel></rss>",
                &[],
                "application/xml",
                Capture::Whole,
            ),
            (
                b"a=1&b=two%20words",
                &[],
                "application/x-www-form-urlencoded",
                Capture::Truncated,
            ),
            (b"plain text body", &[], "text/plain", Capture::Ended),
            (b"abc", &["compress"], "text/plain", Capture::Whole),
        ];
        for (record, (bytes, encodings, media_type, capture)) in (1..).zip(fixtures) {
            for mode in ViewMode::ALL {
                let source = BodySource {
                    record,
                    side: BodySide::Response,
                    bytes,
                    encodings,
                    media_type: media(media_type),
                    capture,
                    search: None,
                };
                let mut cache = BodyCache::<()>::default();
                let update = cache
                    .update(&source, mode, &())
                    .ok_or("the first update always builds")?;
                drop(cache);
                let shown = as_shared(update.content)?;
                let shared = view::prepare(&source.input(), mode);
                assert_eq!(
                    format!("{shown:?}"),
                    format!("{shared:?}"),
                    "{media_type} {mode:?}"
                );
            }
        }
        Ok(())
    }
}
