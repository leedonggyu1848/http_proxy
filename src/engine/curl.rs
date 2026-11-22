//! Conversion of a captured request into a POSIX-shell `curl` command.
//!
//! The command reproduces the method, URL, end-to-end headers, and body the
//! client sent. Anything the capture cannot reproduce exactly (a truncated
//! URL or body, omitted headers, header bytes lost in capture) makes the
//! conversion fail instead of producing a command that sends something else.

use std::fmt::Write as _;

use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};

use super::domain::{BodyLifecycle, BodyRetention, CaptureSource, CapturedRequest, Exchange};
use super::proxy::headers::remove_hop_by_hop_headers;

/// Headers whose original values are never retained by the capture.
const REDACTED_HEADERS: [&str; 2] = ["authorization", "cookie"];
/// Value written for a redacted header when placeholders are requested.
pub const CURL_REDACTED_PLACEHOLDER: &str = "<REDACTED>";
/// Separator between command arguments, continuing the command on a new line.
const CONTINUATION: &str = " \\\n  ";

/// How headers whose values were redacted at capture appear in the command.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RedactedHeaders {
    /// Leave the headers out of the command.
    #[default]
    Omit,
    /// Include them with [`CURL_REDACTED_PLACEHOLDER`] for the user to fill in.
    Placeholder,
}

/// A `curl` command for one captured request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CurlCommand {
    text: String,
    redacted: Vec<String>,
    placeholders: bool,
}

impl CurlCommand {
    /// Returns the command for POSIX shells such as bash and zsh.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the lowercase names of redacted headers the client sent, which
    /// the command omits or carries only as placeholders.
    #[must_use]
    pub fn redacted_headers(&self) -> &[String] {
        &self.redacted
    }

    /// Reports whether redacted headers appear as placeholders rather than
    /// being omitted.
    #[must_use]
    pub const fn has_placeholders(&self) -> bool {
        self.placeholders && !self.redacted.is_empty()
    }
}

/// Why a captured request cannot be copied as an exact `curl` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CurlUnavailable {
    /// A `CONNECT` tunnel record carries no HTTP request to repeat.
    Tunnel,
    /// The request URL was cut short at capture.
    TruncatedUrl,
    /// The request URL is not an absolute `http` or `https` URL.
    UnsupportedUrl,
    /// Some request headers were not retained.
    OmittedHeaders,
    /// A header value contained bytes that were not valid UTF-8.
    UnreproducibleHeader(String),
    /// The request body is still being received.
    BodyInProgress,
    /// The request body ended before it was complete.
    BodyIncomplete,
    /// Only a prefix of the request body was retained.
    BodyTruncated {
        /// Bytes retained in memory.
        retained: usize,
        /// Bytes the client sent.
        observed: u64,
    },
    /// The request body was declared but not captured.
    BodyNotCaptured,
}

impl std::fmt::Display for CurlUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tunnel => f.write_str("CONNECT tunnels have no HTTP request to copy"),
            Self::TruncatedUrl => {
                f.write_str("The captured URL was truncated, so the command would use another URL")
            }
            Self::UnsupportedUrl => f.write_str("The request URL is not an absolute http(s) URL"),
            Self::OmittedHeaders => {
                f.write_str("Some request headers were not captured, so they cannot be copied")
            }
            Self::UnreproducibleHeader(name) => write!(
                f,
                "Header {name} was captured with non-UTF-8 bytes and cannot be reproduced"
            ),
            Self::BodyInProgress => {
                f.write_str("The request body is still being sent; try again when it ends")
            }
            Self::BodyIncomplete => {
                f.write_str("The request body ended early, so the original body is unknown")
            }
            Self::BodyTruncated { retained, observed } => write!(
                f,
                "The request body was truncated: only {retained} of {observed} bytes are kept in \
                 memory, so the command was not copied"
            ),
            Self::BodyNotCaptured => {
                f.write_str("The request body was not captured, so it cannot be copied")
            }
        }
    }
}

impl std::error::Error for CurlUnavailable {}

/// Reports why `exchange` can never be copied, regardless of its body, so a
/// menu can be disabled up front.
#[must_use]
pub fn curl_unavailable_reason(exchange: &Exchange) -> Option<CurlUnavailable> {
    (exchange.source() == CaptureSource::Connect).then_some(CurlUnavailable::Tunnel)
}

/// Builds a `curl` command that sends the captured request again.
///
/// # Errors
///
/// Returns why the capture cannot be reproduced exactly.
pub fn curl_command(
    exchange: &Exchange,
    redacted: RedactedHeaders,
) -> Result<CurlCommand, CurlUnavailable> {
    if let Some(reason) = curl_unavailable_reason(exchange) {
        return Err(reason);
    }
    let request = exchange.request();
    let uri = request_uri(request.target())?;
    if request.headers().omitted() {
        return Err(CurlUnavailable::OmittedHeaders);
    }
    let headers = end_to_end_headers(request, &uri)?;
    let body = request_body(request)?;
    Ok(render(
        request.method(),
        &uri,
        &headers,
        body.as_deref(),
        redacted,
    ))
}

/// Validates the captured target as the absolute URL curl will request.
fn request_uri(target: &str) -> Result<Uri, CurlUnavailable> {
    // Captured targets are ASCII URIs, so the ellipsis only marks truncation.
    if target.ends_with('…') {
        return Err(CurlUnavailable::TruncatedUrl);
    }
    let uri = target
        .parse::<Uri>()
        .map_err(|_| CurlUnavailable::UnsupportedUrl)?;
    if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.authority().is_none() {
        return Err(CurlUnavailable::UnsupportedUrl);
    }
    Ok(uri)
}

/// One header of the command; `None` marks a value redacted at capture.
struct Field<'a> {
    name: &'a str,
    value: Option<&'a str>,
}

/// Returns the headers the server received, in captured order, without
/// hop-by-hop fields, `Content-Length`, and a `Host` that matches the URL.
fn end_to_end_headers<'a>(
    request: &'a CapturedRequest,
    uri: &Uri,
) -> Result<Vec<Field<'a>>, CurlUnavailable> {
    let entries = request.headers().entries();
    let mut map = HeaderMap::new();
    for entry in entries {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(entry.name().as_bytes()),
            HeaderValue::from_str(entry.value()),
        ) {
            map.append(name, value);
        }
    }
    remove_hop_by_hop_headers(&mut map);
    let authority = uri.authority().map(http::uri::Authority::as_str);
    let mut fields = Vec::new();
    for entry in entries {
        let name = entry.name();
        if !map.contains_key(name)
            || name.eq_ignore_ascii_case(http::header::CONTENT_LENGTH.as_str())
            || (name.eq_ignore_ascii_case(http::header::HOST.as_str())
                && authority.is_some_and(|authority| entry.value().eq_ignore_ascii_case(authority)))
        {
            continue;
        }
        if REDACTED_HEADERS
            .iter()
            .any(|redacted| name.eq_ignore_ascii_case(redacted))
        {
            fields.push(Field { name, value: None });
            continue;
        }
        if entry.value().contains(char::REPLACEMENT_CHARACTER) {
            return Err(CurlUnavailable::UnreproducibleHeader(name.to_owned()));
        }
        fields.push(Field {
            name,
            value: Some(entry.value()),
        });
    }
    Ok(fields)
}

/// Returns the complete request body, or `None` when the request had none.
fn request_body(request: &CapturedRequest) -> Result<Option<Vec<u8>>, CurlUnavailable> {
    let headers = request.headers();
    let length = headers.get(http::header::CONTENT_LENGTH.as_str()).next();
    let chunked = headers
        .get(http::header::TRANSFER_ENCODING.as_str())
        .next()
        .is_some();
    // A declared body, even an empty one, is sent with `--data-binary`.
    let declared = length.is_some() || chunked;
    let Some(preview) = request.body() else {
        let empty = !chunked && length.is_none_or(|length| length.trim() == "0");
        return if empty {
            Ok(declared.then(Vec::new))
        } else {
            Err(CurlUnavailable::BodyNotCaptured)
        };
    };
    let snapshot = preview.snapshot().ok_or(CurlUnavailable::BodyInProgress)?;
    match snapshot.lifecycle() {
        BodyLifecycle::Streaming => return Err(CurlUnavailable::BodyInProgress),
        BodyLifecycle::Interrupted | BodyLifecycle::Cancelled => {
            return Err(CurlUnavailable::BodyIncomplete);
        }
        BodyLifecycle::Complete => {}
    }
    match snapshot.retention() {
        BodyRetention::Omitted => return Err(CurlUnavailable::BodyNotCaptured),
        BodyRetention::Truncated => {
            return Err(CurlUnavailable::BodyTruncated {
                retained: snapshot.bytes().len(),
                observed: snapshot.observed_bytes(),
            });
        }
        BodyRetention::Prefix => {}
    }
    if snapshot.observed_bytes() != snapshot.bytes().len() as u64 {
        return Err(CurlUnavailable::BodyTruncated {
            retained: snapshot.bytes().len(),
            observed: snapshot.observed_bytes(),
        });
    }
    let body = snapshot.bytes();
    Ok((declared || !body.is_empty()).then(|| body.to_vec()))
}

fn render(
    method: &Method,
    uri: &Uri,
    headers: &[Field<'_>],
    body: Option<&[u8]>,
    redacted: RedactedHeaders,
) -> CurlCommand {
    // The method and URL share the first line, like a request line.
    let mut request_line = Vec::new();
    // `--data-binary` alone makes curl send POST; otherwise it sends GET.
    let default_method = if body.is_some() {
        Method::POST
    } else {
        Method::GET
    };
    if *method == Method::HEAD && body.is_none() {
        request_line.push("--head".to_owned());
    } else if *method != default_method {
        request_line.push(format!("-X {}", word(method.as_str())));
    }
    let url = uri.to_string();
    request_line.push(quote(&url));
    let mut arguments = Vec::new();
    if url.contains(['[', ']', '{', '}']) {
        arguments.push("--globoff".to_owned());
    }

    let mut redacted_names: Vec<String> = Vec::new();
    for field in headers {
        let value = if let Some(value) = field.value {
            value
        } else {
            let name = field.name.to_ascii_lowercase();
            if !redacted_names.contains(&name) {
                redacted_names.push(name);
            }
            if redacted == RedactedHeaders::Omit {
                continue;
            }
            CURL_REDACTED_PLACEHOLDER
        };
        arguments.push(format!("-H {}", quote(&header_argument(field.name, value))));
    }
    let has = |name: &str| {
        headers
            .iter()
            .any(|field| field.name.eq_ignore_ascii_case(name))
    };
    // curl adds these on its own; an empty value tells it not to.
    for name in ["User-Agent", "Accept"] {
        if !has(name) {
            arguments.push(format!("-H {}", quote(&format!("{name}:"))));
        }
    }
    if body.is_some() && !has("content-type") {
        arguments.push(format!("-H {}", quote("Content-Type:")));
    }

    let mut input = None;
    if let Some(body) = body {
        match std::str::from_utf8(body) {
            Ok(text) if is_printable(text) && !text.starts_with('@') => {
                arguments.push(format!("--data-binary {}", quote(text)));
            }
            _ => {
                input = Some(format!("printf {} | ", quote(&printf_format(body))));
                arguments.push("--data-binary @-".to_owned());
            }
        }
    }
    if has("accept-encoding") {
        arguments.push("--compressed".to_owned());
    }

    let mut text = input.unwrap_or_default();
    text.push_str("curl ");
    text.push_str(&request_line.join(" "));
    for argument in arguments {
        text.push_str(CONTINUATION);
        text.push_str(&argument);
    }
    CurlCommand {
        text,
        redacted: redacted_names,
        placeholders: redacted == RedactedHeaders::Placeholder,
    }
}

/// Formats one `-H` value. curl drops a header given as `name:`, so an
/// empty value uses its `name;` form, which sends the header empty.
fn header_argument(name: &str, value: &str) -> String {
    if value.is_empty() {
        format!("{name};")
    } else {
        format!("{name}: {value}")
    }
}

/// Reports whether `text` can be pasted inside single quotes as is: no
/// control characters other than tab and line feed.
fn is_printable(text: &str) -> bool {
    text.chars()
        .all(|character| matches!(character, '\t' | '\n') || !character.is_control())
}

/// Writes `value` bare when it has no shell-special characters, as for
/// standard methods, and quotes it otherwise.
fn word(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        value.to_owned()
    } else {
        quote(value)
    }
}

/// Quotes `value` as one POSIX-shell word. Inside single quotes every byte
/// is literal; a single quote itself ends the quote, is escaped, and reopens it.
fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for character in value.chars() {
        if character == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(character);
        }
    }
    quoted.push('\'');
    quoted
}

/// Encodes arbitrary bytes as a POSIX `printf` format that prints exactly
/// those bytes, including NUL, which no shell argument can carry.
fn printf_format(bytes: &[u8]) -> String {
    let mut format = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        match byte {
            b'\\' => format.push_str("\\\\"),
            b'%' => format.push_str("%%"),
            b' '..=b'~' => format.push(char::from(byte)),
            _ => {
                let _infallible = write!(format, "\\{byte:03o}");
            }
        }
    }
    format
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::domain::{BodyPreview, CapturedHeader, HeaderPreview, RecordId, SessionId};

    fn exchange(
        method: Method,
        target: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Exchange {
        let headers = HeaderPreview::new(
            headers
                .iter()
                .map(|(name, value)| CapturedHeader::new((*name).into(), (*value).into()))
                .collect(),
            false,
        );
        let exchange = Exchange::new(
            RecordId::new(1),
            SessionId::next(),
            CaptureSource::HttpProxy,
            CapturedRequest::new(method, target.to_owned(), headers),
        );
        if let Some(body) = body {
            let preview = BodyPreview::new(8 * 1024);
            preview.observe(body);
            preview.complete();
            exchange.attach_request_body(preview);
        }
        exchange
    }

    fn command(exchange: &Exchange) -> String {
        curl_command(exchange, RedactedHeaders::Omit).map_or_else(
            |error| error.to_string(),
            |command| command.text().to_owned(),
        )
    }

    #[test]
    fn quotes_special_characters_as_one_literal_word() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("$HOME \\n `x`"), "'$HOME \\n `x`'");
        assert_eq!(quote("줄\n바꿈"), "'줄\n바꿈'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn encodes_unprintable_bytes_for_printf() {
        assert_eq!(
            printf_format(b"a\0b\\c%d'\n\xff"),
            "a\\000b\\\\c%%d'\\012\\377"
        );
    }

    #[test]
    fn get_omits_method_framing_and_host_matching_the_url() {
        let record = exchange(
            Method::GET,
            "http://example.test/a?b=1",
            &[
                ("host", "example.test"),
                ("user-agent", "agent"),
                ("accept", "*/*"),
                ("proxy-connection", "keep-alive"),
                ("connection", "keep-alive, x-hop"),
                ("x-hop", "1"),
                ("te", "trailers"),
            ],
            None,
        );
        assert_eq!(
            command(&record),
            "curl 'http://example.test/a?b=1' \\\n  -H 'user-agent: agent' \\\n  -H 'accept: */*'"
        );
    }

    #[test]
    fn post_body_with_special_characters_stays_literal() {
        let body = "it's $HOME\n\\ 한글\t".as_bytes();
        let record = exchange(
            Method::POST,
            "http://example.test/",
            &[
                ("host", "other.test"),
                ("content-type", "text/plain"),
                ("x-quote", "a'b $c \\d 한"),
                ("x-empty", ""),
                ("accept-encoding", "gzip"),
                ("content-length", "20"),
            ],
            Some(body),
        );
        assert_eq!(
            command(&record),
            "curl 'http://example.test/' \\\n  -H 'host: other.test' \\\n  \
             -H 'content-type: text/plain' \\\n  -H 'x-quote: a'\\''b $c \\d 한' \\\n  \
             -H 'x-empty;' \\\n  -H 'accept-encoding: gzip' \\\n  -H 'User-Agent:' \\\n  \
             -H 'Accept:' \\\n  --data-binary 'it'\\''s $HOME\n\\ 한글\t' \\\n  --compressed"
        );
    }

    #[test]
    fn binary_or_at_prefixed_bodies_are_piped_from_printf() {
        for body in [&b"\x1f\x8b\0\xff"[..], b"@/etc/passwd", b"a\r\nb"] {
            let record = exchange(
                Method::PUT,
                "http://example.test/",
                &[
                    ("content-type", "application/octet-stream"),
                    ("user-agent", "u"),
                    ("accept", "a"),
                ],
                Some(body),
            );
            let text = command(&record);
            assert!(
                text.starts_with(&format!(
                    "printf {} | curl -X PUT ",
                    quote(&printf_format(body))
                )),
                "{text}"
            );
            assert!(text.ends_with("--data-binary @-"), "{text}");
        }
    }

    #[test]
    fn methods_differing_from_the_curl_default_are_explicit() {
        let headers = [("user-agent", "u"), ("accept", "a")];
        let with = |method, body: Option<&[u8]>| {
            command(&exchange(method, "http://h.test/", &headers, body))
        };
        assert!(with(Method::HEAD, None).starts_with("curl --head 'http://h.test/'"));
        assert!(with(Method::DELETE, None).starts_with("curl -X DELETE 'http://h.test/'"));
        assert!(with(Method::GET, Some(b"q")).starts_with("curl -X GET 'http://h.test/'"));
        assert!(with(Method::POST, None).starts_with("curl -X POST 'http://h.test/'"));
        assert!(with(Method::POST, Some(b"q")).starts_with("curl 'http://h.test/'"));
        let empty_post = exchange(
            Method::POST,
            "http://h.test/",
            &[
                ("user-agent", "u"),
                ("accept", "a"),
                ("content-length", "0"),
            ],
            Some(b""),
        );
        assert!(command(&empty_post).contains("--data-binary ''"));
        assert!(
            command(&exchange(Method::GET, "http://h.test/{a}", &headers, None))
                .contains("--globoff")
        );
    }

    #[test]
    fn redacted_headers_are_omitted_or_replaced_by_placeholders() {
        let record = exchange(
            Method::GET,
            "https://example.test/",
            &[
                ("authorization", "[REDACTED]"),
                ("cookie", "[REDACTED]"),
                ("cookie", "[REDACTED]"),
                ("proxy-authorization", "[REDACTED]"),
                ("user-agent", "u"),
                ("accept", "a"),
            ],
            None,
        );
        let omitted = curl_command(&record, RedactedHeaders::Omit);
        assert_eq!(
            omitted.as_ref().map(CurlCommand::text),
            Ok("curl 'https://example.test/' \\\n  -H 'user-agent: u' \\\n  -H 'accept: a'")
        );
        assert_eq!(
            omitted.as_ref().map(CurlCommand::redacted_headers),
            Ok(&["authorization".to_owned(), "cookie".to_owned()][..])
        );
        assert_eq!(omitted.map(|command| command.has_placeholders()), Ok(false));
        let placeholders = curl_command(&record, RedactedHeaders::Placeholder);
        assert!(
            placeholders
                .as_ref()
                .is_ok_and(|command| command.has_placeholders()
                    && command.text().contains("-H 'authorization: <REDACTED>'")
                    && command.text().matches("-H 'cookie: <REDACTED>'").count() == 2
                    && !command.text().contains("proxy-authorization"))
        );
    }

    #[test]
    fn refuses_requests_that_cannot_be_reproduced() {
        let headers = [("user-agent", "u")];
        let truncated = exchange(Method::GET, "http://h.test/aaaa…", &headers, None);
        assert_eq!(
            curl_command(&truncated, RedactedHeaders::Omit),
            Err(CurlUnavailable::TruncatedUrl)
        );
        let lossy = exchange(
            Method::GET,
            "http://h.test/",
            &[("x-raw", "a\u{fffd}")],
            None,
        );
        assert_eq!(
            curl_command(&lossy, RedactedHeaders::Omit),
            Err(CurlUnavailable::UnreproducibleHeader("x-raw".into()))
        );
        let undeclared = exchange(
            Method::POST,
            "http://h.test/",
            &[("content-length", "3")],
            None,
        );
        assert_eq!(
            curl_command(&undeclared, RedactedHeaders::Omit),
            Err(CurlUnavailable::BodyNotCaptured)
        );
    }

    #[test]
    fn refuses_bodies_not_complete_in_memory() {
        let headers = [("user-agent", "u")];
        let long = exchange(Method::POST, "http://h.test/", &headers, None);
        let preview = BodyPreview::new(4);
        preview.observe(b"abcdef");
        long.attach_request_body(preview.clone());
        assert_eq!(
            curl_command(&long, RedactedHeaders::Omit),
            Err(CurlUnavailable::BodyInProgress)
        );
        preview.complete();
        assert_eq!(
            curl_command(&long, RedactedHeaders::Omit),
            Err(CurlUnavailable::BodyTruncated {
                retained: 4,
                observed: 6
            })
        );

        let cut = exchange(Method::POST, "http://h.test/", &headers, None);
        let preview = BodyPreview::new(8);
        preview.cancel();
        cut.attach_request_body(preview);
        assert_eq!(
            curl_command(&cut, RedactedHeaders::Omit),
            Err(CurlUnavailable::BodyIncomplete)
        );
    }

    #[test]
    fn refuses_headerless_and_tunnel_records() {
        let omitted = Exchange::new(
            RecordId::new(2),
            SessionId::next(),
            CaptureSource::HttpProxy,
            CapturedRequest::new(
                Method::GET,
                "http://h.test/".into(),
                HeaderPreview::new(Vec::new(), true),
            ),
        );
        assert_eq!(
            curl_command(&omitted, RedactedHeaders::Omit),
            Err(CurlUnavailable::OmittedHeaders)
        );

        let tunnel = Exchange::new(
            RecordId::new(3),
            SessionId::next(),
            CaptureSource::Connect,
            CapturedRequest::new(
                Method::CONNECT,
                "h.test:443".into(),
                HeaderPreview::default(),
            ),
        );
        assert_eq!(
            curl_unavailable_reason(&tunnel),
            Some(CurlUnavailable::Tunnel)
        );
        assert_eq!(
            curl_command(&tunnel, RedactedHeaders::Omit),
            Err(CurlUnavailable::Tunnel)
        );
    }
}
