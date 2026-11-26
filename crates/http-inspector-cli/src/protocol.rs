//! Messages on the control socket. Each message is one line of JSON.
//!
//! The client sends one request, `{"schema":1,"command":…,"args":{…}}`. The
//! daemon answers with `{"ok":true,"data":{…}}` or
//! `{"ok":false,"error":{…}}`; streaming commands answer with one such line
//! per event. Requests are parsed into domain types here, at the boundary,
//! before the daemon acts on them.

use std::path::PathBuf;
use std::time::Duration;

use http_inspector_analysis::filter::RecordFilter;
use http_inspector_analysis::search::{MatchOptions, SearchScope};
use http_inspector_exchange::{BodySide, RecordId};
use serde_json::{Map, Value, json};

use crate::error::{CliError, ErrorCode};

/// Version of the request, response, and output documents.
pub(crate) const SCHEMA_VERSION: u64 = 1;
/// Longest request line the daemon reads.
pub(crate) const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
/// Longest `records wait` timeout.
pub(crate) const MAX_WAIT: Duration = Duration::from_secs(3600);

/// A record ID typed by the user. Record identities are allocated only by
/// the capture; a reference just names one to look up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RecordRef(u64);

impl RecordRef {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn value(self) -> u64 {
        self.0
    }

    /// Whether `id` is the record this reference names.
    pub(crate) const fn names(self, id: RecordId) -> bool {
        id.value() == self.0
    }

    /// Whether `id` comes after this reference.
    pub(crate) const fn precedes(self, id: RecordId) -> bool {
        id.value() > self.0
    }
}

/// Options for starting the proxy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StartOptions {
    /// Listener port; `None` uses the saved or default port.
    pub(crate) port: Option<u16>,
    pub(crate) https_inspection: bool,
}

/// Filter text, where it is searched, and which outcomes are shown.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct FilterSpec {
    pub(crate) text: String,
    pub(crate) scope: SearchScope,
    pub(crate) options: MatchOptions,
    pub(crate) kind: Kind,
}

/// Outcome kind filter, as [`RecordFilter`] with a default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Kind(pub(crate) RecordFilter);

impl Default for Kind {
    fn default() -> Self {
        Self(RecordFilter::All)
    }
}

/// Which part of a record `records show` prints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Part {
    All,
    Request,
    Response,
}

/// How `records body` shows a body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BodyView {
    Pretty,
    Raw,
    Tree,
    Hex,
}

/// A tree row to select instead of printing the whole body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TreePath {
    Json(String),
    Xml(String),
}

impl TreePath {
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Json(path) | Self::Xml(path) => path,
        }
    }
}

/// `records replay`: changes to the captured request and how to send it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReplayRequest {
    pub(crate) id: RecordRef,
    pub(crate) method: Option<String>,
    pub(crate) url: Option<String>,
    /// Headers that replace every captured header of the same name.
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) remove_headers: Vec<String>,
    /// Absolute path of a complete replacement body.
    pub(crate) body_file: Option<PathBuf>,
    /// Confirms a request that may change server state.
    pub(crate) yes: bool,
    /// Prints the request without sending it.
    pub(crate) dry_run: bool,
    /// How long to wait for the replayed exchange to finish.
    pub(crate) timeout: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BodyRequest {
    pub(crate) id: RecordRef,
    pub(crate) side: BodySide,
    /// `None` picks Pretty.
    pub(crate) view: Option<BodyView>,
    pub(crate) max_bytes: Option<usize>,
    pub(crate) path: Option<TreePath>,
}

/// One validated daemon request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Request {
    Status,
    Start(StartOptions),
    Stop,
    SetHttpsInspection(bool),
    List {
        filter: FilterSpec,
        since: Option<RecordRef>,
        limit: Option<usize>,
        full: bool,
    },
    Show {
        id: RecordRef,
        part: Part,
    },
    Body(BodyRequest),
    Watch {
        filter: FilterSpec,
        since: Option<RecordRef>,
        full: bool,
    },
    Wait {
        filter: FilterSpec,
        since: Option<RecordRef>,
        timeout: Duration,
    },
    Clear,
    SaveLog(PathBuf),
    Replay(ReplayRequest),
    Curl {
        id: RecordRef,
        placeholders: bool,
    },
    Export(ExportRequest),
    Endpoints {
        filter: FilterSpec,
        openapi: bool,
    },
    Hls(HlsRequest),
}

/// `records export`: which records go into the HAR and where it goes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExportRequest {
    pub(crate) filter: FilterSpec,
    /// Absolute destination; `None` returns the document.
    pub(crate) output: Option<PathBuf>,
    pub(crate) include_bodies: bool,
    pub(crate) include_redacted_headers: bool,
}

/// `records hls`: inspect a playlist, or download one variant to `output`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HlsRequest {
    pub(crate) id: RecordRef,
    pub(crate) variant: Option<usize>,
    /// Absolute destination of the merged `.ts` file; `None` only inspects.
    pub(crate) output: Option<PathBuf>,
}

impl Request {
    /// Command name on the wire.
    const fn command(&self) -> &'static str {
        match self {
            Self::Status => "proxy.status",
            Self::Start(_) => "proxy.start",
            Self::Stop => "proxy.stop",
            Self::SetHttpsInspection(_) => "proxy.set",
            Self::List { .. } => "records.list",
            Self::Show { .. } => "records.show",
            Self::Body(_) => "records.body",
            Self::Watch { .. } => "records.watch",
            Self::Wait { .. } => "records.wait",
            Self::Clear => "records.clear",
            Self::SaveLog(_) => "log.save",
            Self::Replay(_) => "records.replay",
            Self::Curl { .. } => "records.curl",
            Self::Export(_) => "records.export",
            Self::Endpoints { .. } => "records.endpoints",
            Self::Hls(_) => "records.hls",
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        let args = match self {
            Self::Status | Self::Stop | Self::Clear => json!({}),
            Self::Start(options) => json!({
                "port": options.port,
                "https_inspection": options.https_inspection,
            }),
            Self::SetHttpsInspection(enabled) => json!({ "https_inspection": enabled }),
            Self::List {
                filter,
                since,
                limit,
                full,
            } => json!({
                "filter": filter_json(filter),
                "since": since.map(RecordRef::value),
                "limit": limit,
                "full": full,
            }),
            Self::Show { id, part } => json!({ "id": id.value(), "part": part_name(*part) }),
            Self::Body(body) => body_json(body),
            Self::Watch {
                filter,
                since,
                full,
            } => json!({
                "filter": filter_json(filter),
                "since": since.map(RecordRef::value),
                "full": full,
            }),
            Self::Wait {
                filter,
                since,
                timeout,
            } => json!({
                "filter": filter_json(filter),
                "since": since.map(RecordRef::value),
                "timeout_ms": u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            }),
            Self::SaveLog(path) => json!({ "path": path.to_string_lossy() }),
            Self::Replay(replay) => replay_json(replay),
            Self::Curl { id, placeholders } => {
                json!({ "id": id.value(), "placeholders": placeholders })
            }
            Self::Export(export) => json!({
                "filter": filter_json(&export.filter),
                "output": export.output.as_ref().map(|path| path.to_string_lossy()),
                "include_bodies": export.include_bodies,
                "include_redacted_headers": export.include_redacted_headers,
            }),
            Self::Endpoints { filter, openapi } => {
                json!({ "filter": filter_json(filter), "openapi": openapi })
            }
            Self::Hls(hls) => json!({
                "id": hls.id.value(),
                "variant": hls.variant,
                "output": hls.output.as_ref().map(|path| path.to_string_lossy()),
            }),
        };
        json!({ "schema": SCHEMA_VERSION, "command": self.command(), "args": args })
    }

    /// Validates a request line from a client.
    pub(crate) fn from_json(value: &Value) -> Result<Self, CliError> {
        if value.get("schema").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
            return Err(invalid("unsupported request schema version"));
        }
        let command = value
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("the request has no command"))?;
        let empty = Map::new();
        let args = Args(
            value
                .get("args")
                .and_then(Value::as_object)
                .unwrap_or(&empty),
        );
        Ok(match command {
            "proxy.status" => Self::Status,
            "proxy.stop" => Self::Stop,
            "records.clear" => Self::Clear,
            "proxy.start" => Self::Start(StartOptions {
                port: args.optional_u16("port")?,
                https_inspection: args.flag("https_inspection")?,
            }),
            "proxy.set" => Self::SetHttpsInspection(args.flag("https_inspection")?),
            "records.list" => Self::List {
                filter: args.filter()?,
                since: args.optional_id("since")?,
                limit: args.optional_usize("limit")?,
                full: args.flag("full")?,
            },
            "records.show" => Self::Show {
                id: args.id("id")?,
                part: parse_part(args.optional_str("part")?.unwrap_or("all"))?,
            },
            "records.body" => Self::Body(args.body()?),
            "records.watch" => Self::Watch {
                filter: args.filter()?,
                since: args.optional_id("since")?,
                full: args.flag("full")?,
            },
            "records.wait" => Self::Wait {
                filter: args.filter()?,
                since: args.optional_id("since")?,
                timeout: args.timeout()?,
            },
            "log.save" => Self::SaveLog(args.absolute_path("path")?),
            "records.replay" => Self::Replay(args.replay()?),
            "records.curl" => Self::Curl {
                id: args.id("id")?,
                placeholders: args.flag("placeholders")?,
            },
            "records.export" => Self::Export(ExportRequest {
                filter: args.filter()?,
                output: args.optional_absolute_path("output")?,
                include_bodies: args.flag("include_bodies")?,
                include_redacted_headers: args.flag("include_redacted_headers")?,
            }),
            "records.endpoints" => Self::Endpoints {
                filter: args.filter()?,
                openapi: args.flag("openapi")?,
            },
            "records.hls" => Self::Hls(HlsRequest {
                id: args.id("id")?,
                variant: args.optional_usize("variant")?,
                output: args.optional_absolute_path("output")?,
            }),
            other => return Err(invalid(&format!("unknown command {other:?}"))),
        })
    }
}

fn invalid(message: &str) -> CliError {
    CliError::new(ErrorCode::Usage, message)
}

fn filter_json(filter: &FilterSpec) -> Value {
    json!({
        "text": filter.text,
        "scope": scope_name(filter.scope),
        "case_sensitive": filter.options.case_sensitive,
        "regex": filter.options.regex,
        "kind": kind_name(filter.kind.0),
    })
}

fn replay_json(replay: &ReplayRequest) -> Value {
    json!({
        "id": replay.id.value(),
        "method": replay.method,
        "url": replay.url,
        "headers": replay
            .headers
            .iter()
            .map(|(name, value)| json!([name, value]))
            .collect::<Vec<_>>(),
        "remove_headers": replay.remove_headers,
        "body_file": replay.body_file.as_ref().map(|path| path.to_string_lossy()),
        "yes": replay.yes,
        "dry_run": replay.dry_run,
        "timeout_ms": u64::try_from(replay.timeout.as_millis()).unwrap_or(u64::MAX),
    })
}

fn body_json(body: &BodyRequest) -> Value {
    let (json_path, xpath) = match &body.path {
        Some(TreePath::Json(path)) => (Some(path.as_str()), None),
        Some(TreePath::Xml(path)) => (None, Some(path.as_str())),
        None => (None, None),
    };
    json!({
        "id": body.id.value(),
        "side": side_name(body.side),
        "view": body.view.map(view_name),
        "max_bytes": body.max_bytes,
        "json_path": json_path,
        "xpath": xpath,
    })
}

/// Typed access to a request's `args` object.
struct Args<'a>(&'a Map<String, Value>);

impl Args<'_> {
    fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key).filter(|value| !value.is_null())
    }

    fn flag(&self, key: &str) -> Result<bool, CliError> {
        self.get(key).map_or(Ok(false), |value| {
            value
                .as_bool()
                .ok_or_else(|| invalid(&format!("{key} must be true or false")))
        })
    }

    fn optional_u64(&self, key: &str) -> Result<Option<u64>, CliError> {
        self.get(key)
            .map(|value| {
                value
                    .as_u64()
                    .ok_or_else(|| invalid(&format!("{key} must be a non-negative integer")))
            })
            .transpose()
    }

    fn optional_u16(&self, key: &str) -> Result<Option<u16>, CliError> {
        self.optional_u64(key)?
            .map(|value| u16::try_from(value).map_err(|_| invalid(&format!("{key} is too large"))))
            .transpose()
    }

    fn optional_usize(&self, key: &str) -> Result<Option<usize>, CliError> {
        self.optional_u64(key)?
            .map(|value| {
                usize::try_from(value).map_err(|_| invalid(&format!("{key} is too large")))
            })
            .transpose()
    }

    fn optional_id(&self, key: &str) -> Result<Option<RecordRef>, CliError> {
        Ok(self.optional_u64(key)?.map(RecordRef::new))
    }

    fn id(&self, key: &str) -> Result<RecordRef, CliError> {
        self.optional_id(key)?
            .ok_or_else(|| invalid(&format!("{key} is required")))
    }

    fn optional_str(&self, key: &str) -> Result<Option<&str>, CliError> {
        self.get(key)
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| invalid(&format!("{key} must be a string")))
            })
            .transpose()
    }

    fn absolute_path(&self, key: &str) -> Result<PathBuf, CliError> {
        let path = PathBuf::from(
            self.optional_str(key)?
                .ok_or_else(|| invalid(&format!("{key} is required")))?,
        );
        if path.is_absolute() {
            Ok(path)
        } else {
            Err(invalid(&format!("{key} must be an absolute path")))
        }
    }

    fn optional_absolute_path(&self, key: &str) -> Result<Option<PathBuf>, CliError> {
        if self.get(key).is_none() {
            return Ok(None);
        }
        self.absolute_path(key).map(Some)
    }

    fn strings(&self, key: &str) -> Result<Vec<String>, CliError> {
        self.get(key).map_or(Ok(Vec::new()), |value| {
            value
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| invalid(&format!("{key} must be a list of strings")))
        })
    }

    fn header_pairs(&self, key: &str) -> Result<Vec<(String, String)>, CliError> {
        let pair = |item: &Value| match item.as_array()?.as_slice() {
            [name, value] => Some((name.as_str()?.to_owned(), value.as_str()?.to_owned())),
            _ => None,
        };
        self.get(key).map_or(Ok(Vec::new()), |value| {
            value
                .as_array()
                .and_then(|items| items.iter().map(pair).collect::<Option<Vec<_>>>())
                .ok_or_else(|| invalid(&format!("{key} must be a list of [name, value] pairs")))
        })
    }

    fn replay(&self) -> Result<ReplayRequest, CliError> {
        Ok(ReplayRequest {
            id: self.id("id")?,
            method: self.optional_str("method")?.map(str::to_owned),
            url: self.optional_str("url")?.map(str::to_owned),
            headers: self.header_pairs("headers")?,
            remove_headers: self.strings("remove_headers")?,
            body_file: self.optional_absolute_path("body_file")?,
            yes: self.flag("yes")?,
            dry_run: self.flag("dry_run")?,
            timeout: self.timeout()?,
        })
    }

    fn timeout(&self) -> Result<Duration, CliError> {
        let millis = self
            .optional_u64("timeout_ms")?
            .ok_or_else(|| invalid("timeout_ms is required"))?;
        let timeout = Duration::from_millis(millis);
        if timeout > MAX_WAIT {
            return Err(invalid("the timeout must be at most one hour"));
        }
        Ok(timeout)
    }

    fn filter(&self) -> Result<FilterSpec, CliError> {
        let empty = Map::new();
        let filter = Args(
            self.get("filter")
                .and_then(Value::as_object)
                .unwrap_or(&empty),
        );
        Ok(FilterSpec {
            text: filter.optional_str("text")?.unwrap_or_default().to_owned(),
            scope: parse_scope(filter.optional_str("scope")?.unwrap_or("all"))?,
            options: MatchOptions {
                case_sensitive: filter.flag("case_sensitive")?,
                regex: filter.flag("regex")?,
            },
            kind: Kind(parse_kind(filter.optional_str("kind")?.unwrap_or("all"))?),
        })
    }

    fn body(&self) -> Result<BodyRequest, CliError> {
        let path = match (self.optional_str("json_path")?, self.optional_str("xpath")?) {
            (Some(_), Some(_)) => return Err(invalid("use either json_path or xpath")),
            (Some(path), None) => Some(TreePath::Json(path.to_owned())),
            (None, Some(path)) => Some(TreePath::Xml(path.to_owned())),
            (None, None) => None,
        };
        Ok(BodyRequest {
            id: self.id("id")?,
            side: parse_side(self.optional_str("side")?.unwrap_or("response"))?,
            view: self.optional_str("view")?.map(parse_view).transpose()?,
            max_bytes: self.optional_usize("max_bytes")?,
            path,
        })
    }
}

/// Defines the wire names of an enum and their parser.
macro_rules! names {
    ($name_fn:ident, $parse_fn:ident, $ty:ty, $what:literal, [$(($variant:path, $text:literal)),+ $(,)?]) => {
        pub(crate) const fn $name_fn(value: $ty) -> &'static str {
            match value {
                $($variant => $text,)+
            }
        }

        pub(crate) fn $parse_fn(text: &str) -> Result<$ty, CliError> {
            match text {
                $($text => Ok($variant),)+
                other => Err(invalid(&format!(concat!("unknown ", $what, " {:?}"), other))),
            }
        }
    };
}

names!(
    scope_name,
    parse_scope,
    SearchScope,
    "scope",
    [
        (SearchScope::All, "all"),
        (SearchScope::Url, "url"),
        (SearchScope::Headers, "headers"),
        (SearchScope::RequestBody, "request-body"),
        (SearchScope::ResponseBody, "response-body"),
    ]
);

names!(
    kind_name,
    parse_kind,
    RecordFilter,
    "kind",
    [
        (RecordFilter::All, "all"),
        (RecordFilter::Responses, "responses"),
        (RecordFilter::Failures, "failures"),
        (RecordFilter::Tunnels, "tunnels"),
    ]
);

names!(
    side_name,
    parse_side,
    BodySide,
    "side",
    [
        (BodySide::Request, "request"),
        (BodySide::Response, "response"),
    ]
);

names!(
    view_name,
    parse_view,
    BodyView,
    "view",
    [
        (BodyView::Pretty, "pretty"),
        (BodyView::Raw, "raw"),
        (BodyView::Tree, "tree"),
        (BodyView::Hex, "hex"),
    ]
);

names!(
    part_name,
    parse_part,
    Part,
    "part",
    [
        (Part::All, "all"),
        (Part::Request, "request"),
        (Part::Response, "response"),
    ]
);

/// A successful reply line.
pub(crate) fn ok_line(data: &Value) -> Value {
    json!({ "ok": true, "data": data })
}

/// A failed reply line.
pub(crate) fn error_line(error: &CliError) -> Value {
    json!({ "ok": false, "error": error.to_json() })
}

/// Reads a reply line into its data or error.
pub(crate) fn read_reply(value: &Value) -> Result<Value, CliError> {
    match value.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(value.get("data").cloned().unwrap_or(Value::Null)),
        Some(false) => Err(CliError::from_json(
            value.get("error").unwrap_or(&Value::Null),
        )),
        None => Err(CliError::new(
            ErrorCode::DaemonUnavailable,
            "The daemon sent a malformed reply",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_survive_the_wire() -> Result<(), CliError> {
        let filter = FilterSpec {
            text: "method:GET users".to_owned(),
            scope: SearchScope::ResponseBody,
            options: MatchOptions {
                case_sensitive: true,
                regex: false,
            },
            kind: Kind(RecordFilter::Failures),
        };
        for request in [
            Request::Status,
            Request::Start(StartOptions {
                port: Some(0),
                https_inspection: true,
            }),
            Request::SetHttpsInspection(false),
            Request::List {
                filter: filter.clone(),
                since: Some(RecordRef::new(4)),
                limit: Some(10),
                full: true,
            },
            Request::Show {
                id: RecordRef::new(7),
                part: Part::Response,
            },
            Request::Body(BodyRequest {
                id: RecordRef::new(7),
                side: BodySide::Request,
                view: Some(BodyView::Tree),
                max_bytes: Some(4096),
                path: Some(TreePath::Json("$.items[0]".to_owned())),
            }),
            Request::Watch {
                filter: filter.clone(),
                since: None,
                full: false,
            },
            Request::Wait {
                filter,
                since: None,
                timeout: Duration::from_millis(1500),
            },
            Request::Clear,
            Request::SaveLog(PathBuf::from("/tmp/session.log")),
            Request::Replay(ReplayRequest {
                id: RecordRef::new(3),
                method: Some("PUT".to_owned()),
                url: Some("http://example.test/a".to_owned()),
                headers: vec![("X-Debug".to_owned(), "1".to_owned())],
                remove_headers: vec!["Cookie".to_owned()],
                body_file: Some(PathBuf::from("/tmp/body.json")),
                yes: true,
                dry_run: false,
                timeout: Duration::from_secs(5),
            }),
            Request::Curl {
                id: RecordRef::new(3),
                placeholders: true,
            },
            Request::Export(ExportRequest {
                filter: FilterSpec::default(),
                output: Some(PathBuf::from("/tmp/session.har")),
                include_bodies: true,
                include_redacted_headers: false,
            }),
            Request::Endpoints {
                filter: FilterSpec::default(),
                openapi: true,
            },
            Request::Hls(HlsRequest {
                id: RecordRef::new(9),
                variant: Some(1),
                output: Some(PathBuf::from("/tmp/video.ts")),
            }),
            Request::Stop,
        ] {
            assert_eq!(Request::from_json(&request.to_json())?, request);
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_requests_at_the_boundary() {
        let reject = |value: Value| Request::from_json(&value).err().map(|error| error.code);
        assert_eq!(
            reject(json!({"schema": 2, "command": "proxy.status"})),
            Some(ErrorCode::Usage)
        );
        assert_eq!(
            reject(json!({"schema": 1, "command": "log.save", "args": {"path": "relative.log"}})),
            Some(ErrorCode::Usage)
        );
        assert_eq!(
            reject(json!({"schema": 1, "command": "proxy.start", "args": {"port": 70000}})),
            Some(ErrorCode::Usage)
        );
        assert_eq!(
            reject(
                json!({"schema": 1, "command": "records.wait", "args": {"timeout_ms": 7_200_000}})
            ),
            Some(ErrorCode::Usage)
        );
        assert_eq!(
            reject(
                json!({"schema": 1, "command": "records.list", "args": {"filter": {"kind": "other"}}})
            ),
            Some(ErrorCode::Usage)
        );
    }
}
