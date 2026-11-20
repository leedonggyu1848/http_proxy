//! Explicit, validated manual replay using the normal forwarding path.
use super::{
    Arc, BodyExt, BodyPreview, Bytes, CaptureContext, CaptureSource, Destination, EngineState,
    Exchange, Full, HOST, IpAddr, Method, ProxyHandle, Request, SocketAddr, Uri, connect_deadline,
    forward_http, forward_https, infallible_to_hyper, remove_hop_by_hop_headers, resolve_within,
};
use http::HeaderMap;

/// An editable header or cookie; disabled rows are never sent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayField {
    /// Whether to include this row.
    pub enabled: bool,
    /// HTTP field name.
    pub name: String,
    /// Field value, empty for redacted captures.
    pub value: String,
}

/// Owned editor contents. Original sensitive values are never retained.
#[derive(Clone)]
pub struct ReplayDraft {
    /// Original capture.
    pub original: Exchange,
    /// HTTP method.
    pub method: String,
    /// Absolute HTTP or HTTPS URL.
    pub url: String,
    /// Editable end-to-end headers.
    pub headers: Vec<ReplayField>,
    /// Complete replacement body.
    pub body: Vec<u8>,
    /// Requires an explicit complete replacement before sending.
    pub body_missing: bool,
    /// Keep Host aligned with the edited URL.
    pub automatic_host: bool,
}

impl ReplayDraft {
    /// Copies a capture without attempting to recover redacted values.
    pub fn new(original: Exchange) -> Result<Self, String> {
        if original.source() == CaptureSource::Connect {
            return Err("CONNECT tunnels cannot be replayed".to_owned());
        }
        let snapshot = original.request().body().and_then(BodyPreview::snapshot);
        let body_missing = snapshot.as_ref().is_none_or(|body| {
            body.lifecycle() != crate::BodyLifecycle::Complete
                || body.observed_bytes() != body.bytes().len() as u64
        }) || original.request().headers().omitted();
        let entries = original.request().headers().entries();
        let removed =
            framing_header_names(entries.iter().map(|field| (field.name(), field.value())));
        // Keep the captured text as-is; `request` rejects values that cannot be sent exactly.
        let headers = entries
            .iter()
            .filter(|field| !removed.contains(&field.name().to_ascii_lowercase()))
            .map(|field| ReplayField {
                enabled: true,
                name: field.name().to_owned(),
                value: if field.value() == "[REDACTED]" {
                    String::new()
                } else {
                    field.value().to_owned()
                },
            })
            .collect();
        Ok(Self {
            method: original.request().method().to_string(),
            url: original.request().target().to_owned(),
            headers,
            body: snapshot.map_or_else(Vec::new, |body| body.bytes().to_vec()),
            body_missing,
            automatic_host: true,
            original,
        })
    }

    /// Validates framing and fields before any network operation.
    pub fn request(&self, proxy: SocketAddr) -> Result<Request<Full<Bytes>>, String> {
        if self.body_missing {
            return Err(
                "Incomplete body or headers: load a complete replacement before sending".to_owned(),
            );
        }
        let method = self
            .method
            .parse::<Method>()
            .map_err(|_| "Invalid method")?;
        if method == Method::CONNECT || method == Method::TRACE {
            return Err("CONNECT and TRACE cannot be replayed".to_owned());
        }
        let uri = self.url.parse::<Uri>().map_err(|_| "Invalid URL")?;
        if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() {
            return Err("Use an absolute http:// or https:// URL".to_owned());
        }
        let host = uri.host().unwrap_or_default();
        if host.contains('@') || self.url.contains('#') {
            return Err("URL credentials and fragments are not supported".to_owned());
        }
        let port = uri.port_u16().unwrap_or_else(|| {
            if uri.scheme_str() == Some("https") {
                443
            } else {
                80
            }
        });
        if port == proxy.port()
            && (host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback() || ip == proxy.ip()))
        {
            return Err("The URL targets this proxy itself".to_owned());
        }
        let mut request = Request::builder()
            .method(method)
            .uri(uri.clone())
            .body(Full::new(Bytes::copy_from_slice(&self.body)))
            .map_err(|e| e.to_string())?;
        for field in self.headers.iter().filter(|field| field.enabled) {
            let (name, value) = header_pair(field)?;
            request.headers_mut().append(name, value);
        }
        remove_hop_by_hop_headers(request.headers_mut());
        request.headers_mut().remove(http::header::EXPECT);
        if self.automatic_host {
            let authority = uri.authority().ok_or("Missing URL authority")?;
            request.headers_mut().insert(
                HOST,
                authority.as_str().parse().map_err(|_| "Invalid Host")?,
            );
        }
        request.headers_mut().insert(
            http::header::CONTENT_LENGTH,
            self.body
                .len()
                .to_string()
                .parse()
                .map_err(|_| "Invalid body length")?,
        );
        request.extensions_mut().insert(self.original.id());
        Ok(request)
    }

    /// Whether sending requires the user's explicit side-effect confirmation.
    #[must_use]
    pub fn needs_confirmation(&self) -> bool {
        !matches!(self.method.as_str(), "GET" | "HEAD" | "OPTIONS")
    }
}

/// Converts an enabled editor row, refusing values that would not be sent as entered.
fn header_pair(field: &ReplayField) -> Result<(http::HeaderName, http::HeaderValue), String> {
    let name = http::HeaderName::from_bytes(field.name.as_bytes())
        .map_err(|_| format!("Invalid header name: {}", field.name))?;
    if field.value == "[REDACTED]"
        || (field.value.is_empty()
            && matches!(
                name.as_str(),
                "authorization" | "cookie" | "set-cookie" | "proxy-authorization"
            ))
    {
        return Err("Enter redacted header values or disable their rows".to_owned());
    }
    if field.value.contains(char::REPLACEMENT_CHARACTER) {
        return Err(format!(
            "Header {name} was captured with non-UTF-8 bytes and cannot be reproduced; \
             re-enter its value or disable the row"
        ));
    }
    let value = http::HeaderValue::from_str(&field.value)
        .map_err(|_| format!("Invalid value for header {name}"))?;
    Ok((name, value))
}

/// Lowercase names that the proxy regenerates or must not forward.
fn framing_header_names<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut headers = HeaderMap::new();
    for (name, value) in fields {
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name.as_bytes()),
            http::HeaderValue::from_str(value),
        ) {
            headers.append(name, value);
        }
    }
    let before: Vec<_> = headers.keys().map(ToString::to_string).collect();
    remove_hop_by_hop_headers(&mut headers);
    let mut removed: Vec<_> = before
        .into_iter()
        .filter(|name| !headers.contains_key(name.as_str()))
        .collect();
    removed.push(http::header::CONTENT_LENGTH.as_str().to_owned());
    removed
}

/// Running replay; cancelling aborts the engine task.
pub struct ReplayTask(tokio::task::JoinHandle<()>);

impl ReplayTask {
    /// Cancels pending forwarding and response consumption.
    pub fn cancel(&self) {
        self.0.abort();
    }
    /// Whether forwarding and response consumption ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
}

impl ProxyHandle {
    /// Sends a user-approved draft on the running engine, recording a new exchange.
    pub fn replay(&self, draft: &ReplayDraft) -> Result<ReplayTask, String> {
        let request = draft.request(self.local_addr)?;
        if !matches!(self.state(), EngineState::Running) {
            return Err("Start the proxy before sending".to_owned());
        }
        if request.uri().scheme_str() == Some("https") && self.engine.inspector.is_none() {
            return Err("Enable HTTPS inspection before replaying HTTPS".to_owned());
        }
        let engine = Arc::clone(&self.engine);
        Ok(ReplayTask(self.runtime.spawn(async move {
            let Ok(_permit) = Arc::clone(&engine.connections).acquire_owned().await else {
                return;
            };
            let target = request.uri().to_string();
            let source = if request.uri().scheme_str() == Some("https") {
                CaptureSource::InspectedHttps
            } else {
                CaptureSource::HttpProxy
            };
            let capture = CaptureContext::start(&request, source, &target, &engine);
            let request = request.map(|body| body.map_err(infallible_to_hyper).boxed());
            // Resolve before HTTPS as well, preserving the loop guard.
            let response = if source == CaptureSource::InspectedHttps {
                let Some(authority) = request.uri().authority() else {
                    return;
                };
                if let Err(failure) =
                    resolve_within(connect_deadline(&engine), authority, 443, &engine).await
                {
                    failure.respond(&capture);
                    return;
                }
                let Ok(destination) = Destination::from_authority(authority) else {
                    return;
                };
                let Some(inspector) = engine.inspector.as_ref() else {
                    return;
                };
                forward_https(request, &engine, &capture, inspector, &destination).await
            } else {
                forward_http(request, &engine, &capture).await
            };
            let mut body = response.into_body();
            while let Some(frame) = body.frame().await {
                if frame.is_err() {
                    break;
                }
            }
        })))
    }
}
