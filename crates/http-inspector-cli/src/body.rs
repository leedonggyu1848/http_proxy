//! The `records body` document: a body prepared by the same code as the
//! desktop Body tab, cut to the requested size.

use http_inspector_analysis::search::is_binary_media_type;
use http_inspector_content::format::{BodyFormat, Notice, NoticeLevel, ViewMode, tree::TreeModel};
use http_inspector_content::hex::{self, HEX_VIEW_BYTES};
use http_inspector_content::view::{self, BodyCapture, BodyInput, BodyView as Prepared};
use http_inspector_exchange::{
    BodyPreview, BodyRetention, BodySide, BodySnapshot, Exchange, HeaderPreview, MediaType, Outcome,
};
use serde_json::{Map, Value, json};

use crate::error::{CliError, ErrorCode};
use crate::protocol::{BodyRequest, BodyView, TreePath, side_name, view_name};

/// Leading decoded bytes checked for NUL, as list search does.
const SNIFF_BYTES: usize = 1024;

/// The selected body of one record.
struct Selected<'a> {
    body: &'a BodyPreview,
    headers: &'a HeaderPreview,
    media_type: Option<&'a MediaType>,
}

fn select(record: &Exchange, side: BodySide) -> Result<Selected<'_>, CliError> {
    match (side, record.outcome()) {
        (BodySide::Request, _) => {
            let request = record.request();
            let body = request.body().ok_or_else(|| {
                CliError::new(ErrorCode::BodyUnavailable, "The request has no body")
            })?;
            Ok(Selected {
                body,
                headers: request.headers(),
                media_type: request.media_type(),
            })
        }
        (BodySide::Response, Outcome::Response(response)) => Ok(Selected {
            body: response.body(),
            headers: response.headers(),
            media_type: response.media_type(),
        }),
        (BodySide::Response, outcome) => Err(CliError::new(
            ErrorCode::BodyUnavailable,
            format!("The record has no response ({outcome})"),
        )),
    }
}

/// Builds the `records body` document for `request`.
pub(crate) fn render(record: &Exchange, request: &BodyRequest) -> Result<Value, CliError> {
    let selected = select(record, request.side)?;
    let snapshot = selected.body.snapshot().ok_or_else(|| {
        CliError::new(ErrorCode::Busy, "The body is being written").with_hint("retry shortly")
    })?;
    if snapshot.retention() == BodyRetention::Omitted {
        return Err(CliError::new(
            ErrorCode::BodyUnavailable,
            "The body was not retained in memory",
        )
        .with_hint("save the full session log with `http-proxy log save`"));
    }
    let encodings: Vec<&str> = selected.headers.get("content-encoding").collect();
    let view = if request.path.is_some() {
        BodyView::Tree
    } else {
        request.view.unwrap_or(BodyView::Pretty)
    };
    let mut document = json!({
        "id": record.id().value(),
        "side": side_name(request.side),
        "view": view_name(view),
        "media_type": selected.media_type.map(MediaType::essence),
        "content_encoding": encodings,
        "observed_bytes": snapshot.observed_bytes(),
        "retained_bytes": snapshot.bytes().len(),
        "capture": BodyCapture::of(&snapshot).name(),
    });
    let content = match view {
        BodyView::Hex => hex_view(&snapshot, request.max_bytes),
        BodyView::Pretty | BodyView::Raw | BodyView::Tree => {
            if selected.media_type.is_some_and(is_binary_media_type) {
                return Err(binary());
            }
            let input = BodyInput {
                bytes: snapshot.bytes(),
                encodings: &encodings,
                media_type: selected.media_type,
                capture: BodyCapture::of(&snapshot),
            };
            text_view(&input, view, request)?
        }
    };
    if let (Some(document), Value::Object(content)) = (document.as_object_mut(), content) {
        document.extend(content);
    }
    Ok(document)
}

fn binary() -> CliError {
    CliError::new(ErrorCode::BinaryBody, "The body is binary")
        .with_hint("use `--view hex`, or save the session log with `http-proxy log save`")
}

/// Cuts `text` to at most `max` bytes at a character boundary.
fn cut(text: &str, max: Option<usize>) -> (&str, bool) {
    let Some(max) = max.filter(|max| *max < text.len()) else {
        return (text, false);
    };
    let end = (0..=max)
        .rev()
        .find(|end| text.is_char_boundary(*end))
        .unwrap_or(0);
    (text.get(..end).unwrap_or_default(), true)
}

fn text_document(text: &str, max: Option<usize>) -> Map<String, Value> {
    let (shown, truncated) = cut(text, max);
    let mut map = Map::new();
    map.insert("text".to_owned(), json!(shown));
    map.insert("text_bytes".to_owned(), json!(text.len()));
    map.insert("truncated".to_owned(), json!(truncated));
    map
}

fn hex_view(snapshot: &BodySnapshot, max_bytes: Option<usize>) -> Value {
    let bytes = snapshot.bytes();
    let limit = max_bytes.unwrap_or(HEX_VIEW_BYTES);
    let (shown, _) = bytes.split_at(bytes.len().min(limit));
    json!({
        "format": "Hex",
        "notices": notices(&[Notice::info(format!(
            "First {} of {} kept bytes, as received (before Content-Encoding is removed).",
            shown.len(),
            bytes.len()
        ))]),
        "text": hex::dump(shown),
        "shown_bytes": shown.len(),
        "truncated": shown.len() < bytes.len(),
    })
}

fn text_view(
    input: &BodyInput<'_>,
    view: BodyView,
    request: &BodyRequest,
) -> Result<Value, CliError> {
    let mode = match view {
        BodyView::Raw => ViewMode::Raw,
        BodyView::Tree => ViewMode::Tree,
        _ => ViewMode::Pretty,
    };
    let prepared = view::prepare(input, mode);
    let (format, notice_list, content) = match prepared {
        Prepared::Unavailable(message) => {
            return Err(CliError::new(ErrorCode::ViewUnavailable, message));
        }
        Prepared::Text(text) => {
            let sniffed = text
                .styled
                .text()
                .get(..SNIFF_BYTES)
                .unwrap_or_else(|| text.styled.text());
            if sniffed.contains('\0') {
                return Err(binary());
            }
            if request.path.is_some() {
                return Err(CliError::new(
                    ErrorCode::ViewUnavailable,
                    format!("{} bodies have no tree", text.format.label()),
                ));
            }
            let content = text_document(text.styled.text(), request.max_bytes);
            (text.format, text.notices, content)
        }
        Prepared::Tree {
            format,
            notices,
            model,
        } => (format, notices, tree_document(format, &model, request)?),
    };
    let mut document = Map::new();
    document.insert("format".to_owned(), json!(format.label()));
    document.insert("notices".to_owned(), notices(&notice_list));
    document.extend(content);
    Ok(Value::Object(document))
}

fn tree_document(
    format: BodyFormat,
    model: &TreeModel,
    request: &BodyRequest,
) -> Result<Map<String, Value>, CliError> {
    if let Some(path) = &request.path {
        let wanted_json = matches!(path, TreePath::Json(_));
        if wanted_json != (format == BodyFormat::Json) {
            return Err(CliError::new(
                ErrorCode::ViewUnavailable,
                if wanted_json {
                    "--json-path needs a JSON body"
                } else {
                    "--xpath needs an XML body"
                },
            ));
        }
        let node = model
            .nodes()
            .iter()
            .find(|node| node.path.as_deref() == Some(path.text()))
            .ok_or_else(|| {
                CliError::new(
                    ErrorCode::PathNotFound,
                    format!("No tree row has the path {}", path.text()),
                )
                .with_hint("list the paths with `--view tree`")
            })?;
        let mut map = text_document(&node.value, request.max_bytes);
        map.insert("path".to_owned(), json!(path.text()));
        return Ok(map);
    }
    let nodes: Vec<Value> = model
        .nodes()
        .iter()
        .map(|node| {
            json!({
                "depth": node.depth,
                "label": node.label.text(),
                "path": node.path,
            })
        })
        .collect();
    let mut map = Map::new();
    map.insert("nodes".to_owned(), Value::Array(nodes));
    Ok(map)
}

fn notices(list: &[Notice]) -> Value {
    Value::Array(
        list.iter()
            .map(|notice| {
                json!({
                    "level": match notice.level {
                        NoticeLevel::Info => "info",
                        NoticeLevel::Warning => "warning",
                    },
                    "text": notice.text,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "indexing a serde_json value by key returns null instead of panicking"
)]
mod tests {
    use std::error::Error;

    use http::header::CONTENT_TYPE;
    use http::{HeaderValue, Method, StatusCode};
    use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder, ResponseFixture};

    use crate::protocol::RecordRef;

    use super::*;

    fn response(media: &'static str, body: &[u8]) -> Result<Exchange, Box<dyn Error>> {
        Ok(
            ExchangeBuilder::http(Method::GET, "http://api.example.test/feed")?.respond(
                ResponseFixture::new(StatusCode::OK)
                    .header(CONTENT_TYPE, HeaderValue::from_static(media))
                    .body(body.to_vec(), BodyLifecycle::Complete),
            )?,
        )
    }

    fn request(id: http_inspector_exchange::RecordId) -> BodyRequest {
        BodyRequest {
            id: RecordRef::new(id.value()),
            side: BodySide::Response,
            view: None,
            max_bytes: None,
            path: None,
        }
    }

    #[test]
    fn prints_pretty_text_like_the_body_tab() -> Result<(), Box<dyn Error>> {
        let record = response("application/json", br#"{"items":[{"id":7}]}"#)?;
        let document = render(&record, &request(record.id()))?;
        let media = MediaType::parse("application/json");
        let input = BodyInput {
            bytes: br#"{"items":[{"id":7}]}"#,
            encodings: &[],
            media_type: media.as_ref(),
            capture: BodyCapture::Whole,
        };
        let Prepared::Text(shared) = view::prepare(&input, ViewMode::Pretty) else {
            return Err("expected text".into());
        };
        assert_eq!(document["text"], shared.styled.text());
        assert_eq!(document["format"], "JSON");
        assert_eq!(document["capture"], "whole");

        let cut = render(
            &record,
            &BodyRequest {
                max_bytes: Some(5),
                ..request(record.id())
            },
        )?;
        assert_eq!(cut["truncated"], true);
        assert_eq!(cut["text"].as_str().map(str::len), Some(5));
        Ok(())
    }

    #[test]
    fn selects_json_paths_and_xpaths() -> Result<(), Box<dyn Error>> {
        let json_record = response("application/json", br#"{"items":[{"id":7}]}"#)?;
        let found = render(
            &json_record,
            &BodyRequest {
                path: Some(TreePath::Json("$.items[0].id".to_owned())),
                ..request(json_record.id())
            },
        )?;
        assert_eq!(found["text"], "7");
        let missing = render(
            &json_record,
            &BodyRequest {
                path: Some(TreePath::Json("$.nothing".to_owned())),
                ..request(json_record.id())
            },
        );
        assert_eq!(
            missing.err().map(|error| error.code),
            Some(ErrorCode::PathNotFound)
        );

        let xml = response(
            "application/xml",
            b"<rss><channel><title>News</title></channel></rss>",
        )?;
        let tree = render(
            &xml,
            &BodyRequest {
                view: Some(BodyView::Tree),
                ..request(xml.id())
            },
        )?;
        let paths: Vec<&str> = tree["nodes"].as_array().map_or_else(Vec::new, |nodes| {
            nodes
                .iter()
                .filter_map(|node| node["path"].as_str())
                .collect()
        });
        let title = paths
            .iter()
            .find(|path| path.ends_with("title"))
            .ok_or("no title row")?;
        let selected = render(
            &xml,
            &BodyRequest {
                path: Some(TreePath::Xml((*title).to_owned())),
                ..request(xml.id())
            },
        )?;
        assert!(
            selected["text"]
                .as_str()
                .is_some_and(|text| text.contains("News"))
        );
        Ok(())
    }

    #[test]
    fn refuses_binary_text_and_prints_hex() -> Result<(), Box<dyn Error>> {
        let gif = include_bytes!("../../../tests/fixtures/images/animated.gif");
        let record = response("image/gif", gif)?;
        let default = render(&record, &request(record.id()));
        assert_eq!(
            default.err().map(|error| error.code),
            Some(ErrorCode::BinaryBody)
        );
        let text = render(
            &record,
            &BodyRequest {
                view: Some(BodyView::Raw),
                ..request(record.id())
            },
        );
        assert_eq!(
            text.err().map(|error| error.code),
            Some(ErrorCode::BinaryBody)
        );
        let hex = render(
            &record,
            &BodyRequest {
                view: Some(BodyView::Hex),
                max_bytes: Some(16),
                ..request(record.id())
            },
        )?;
        assert!(
            hex["text"]
                .as_str()
                .is_some_and(|text| text.starts_with("00000000  47 49 46"))
        );
        assert_eq!(hex["truncated"], true);

        let octets = response("application/octet-stream", b"\x00\x01")?;
        assert_eq!(
            render(&octets, &request(octets.id()))
                .err()
                .map(|error| error.code),
            Some(ErrorCode::BinaryBody)
        );
        let empty = render(
            &record,
            &BodyRequest {
                side: BodySide::Request,
                ..request(record.id())
            },
        )?;
        assert_eq!(empty["text"], "");
        let failed = ExchangeBuilder::http(Method::GET, "http://api.example.test/down")?
            .fail(http_inspector_exchange::FailureStage::Connect, "refused")?;
        assert_eq!(
            render(&failed, &request(failed.id()))
                .err()
                .map(|error| error.code),
            Some(ErrorCode::BodyUnavailable)
        );
        Ok(())
    }
}
