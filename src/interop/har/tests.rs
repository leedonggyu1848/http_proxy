//! Unit tests for HAR 1.2 export, built with [`crate::ExchangeBuilder`] without
//! starting a proxy or opening network sockets.
#![expect(
    clippy::indexing_slicing,
    reason = "Reading a serde_json::Value by key yields Null instead of panicking."
)]

use std::error::Error;
use std::io::Write as _;
use std::time::SystemTime;

use http::header::{
    AUTHORIZATION, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, COOKIE, HOST,
    PROXY_AUTHORIZATION, SET_COOKIE,
};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use serde_json::Value;

use super::{HarOptions, document, write};
use crate::exchange::{BodyLifecycle, Exchange, FailureStage, TunnelState};
use crate::{BODY_PREVIEW_BYTES, ExchangeBuilder, ResponseFixture};

type TestResult = Result<(), Box<dyn Error>>;

const TEST_HOST: &str = "127.0.0.1:8080";
const SENSITIVE_HEADERS: [&str; 4] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];
const SECRETS: [&str; 4] = [
    "secret-authorization-token",
    "secret-proxy-credentials",
    "secret-session-cookie",
    "secret-set-cookie",
];
const REQUEST_BODY: &str = "name=request-body-text";
const RESPONSE_BODY: &str = "{\"answer\":\"response-body-text\"}";

fn with_headers<T>(
    target: T,
    headers: Vec<(HeaderName, String)>,
    add: impl Fn(T, HeaderName, HeaderValue) -> T,
) -> Result<T, Box<dyn Error>> {
    headers
        .into_iter()
        .try_fold(target, |target, (name, value)| {
            Ok(add(target, name, HeaderValue::try_from(value)?))
        })
}

fn sensitive_exchange() -> Result<Exchange, Box<dyn Error>> {
    let request_headers = vec![
        (HOST, TEST_HOST.to_owned()),
        (AUTHORIZATION, format!("Bearer {}", SECRETS[0])),
        (PROXY_AUTHORIZATION, format!("Basic {}", SECRETS[1])),
        (COOKIE, format!("session={}", SECRETS[2])),
        (CONTENT_TYPE, "application/x-www-form-urlencoded".to_owned()),
        (CONTENT_LENGTH, REQUEST_BODY.len().to_string()),
        (CONNECTION, "close".to_owned()),
    ];
    let response_headers = vec![
        (CONTENT_TYPE, "application/json".to_owned()),
        (SET_COOKIE, format!("id={}; HttpOnly", SECRETS[3])),
        (CONTENT_LENGTH, RESPONSE_BODY.len().to_string()),
        (CONNECTION, "close".to_owned()),
    ];
    let response = with_headers(
        ResponseFixture::new(StatusCode::OK),
        response_headers,
        ResponseFixture::header,
    )?
    .body(RESPONSE_BODY, BodyLifecycle::Complete);
    Ok(with_headers(
        ExchangeBuilder::http(
            Method::POST,
            format!("http://{TEST_HOST}/login?user=alice&empty"),
        )?,
        request_headers,
        ExchangeBuilder::header,
    )?
    .request_body(REQUEST_BODY, BodyLifecycle::Complete)
    .respond(response)?)
}

fn entries(document: &Value) -> &[Value] {
    document["log"]["entries"]
        .as_array()
        .map_or(&[], Vec::as_slice)
}

fn header_names(headers: &Value) -> Vec<String> {
    headers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|header| header["name"].as_str())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn header_value<'a>(headers: &'a Value, name: &str) -> Option<&'a str> {
    headers.as_array()?.iter().find_map(|header| {
        header["name"]
            .as_str()?
            .eq_ignore_ascii_case(name)
            .then(|| header["value"].as_str())?
    })
}

#[test]
fn default_export_leaves_out_sensitive_headers_and_bodies() -> TestResult {
    let record = sensitive_exchange()?;

    let har = document(&[record], HarOptions::default(), SystemTime::now());
    let text = serde_json::to_string(&har)?;
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} leaked");
    }
    assert!(!text.contains("[REDACTED]"));
    assert!(!text.contains("request-body-text"));
    assert!(!text.contains("response-body-text"));
    let entry = entries(&har).first().ok_or("missing entry")?;
    for headers in [&entry["request"]["headers"], &entry["response"]["headers"]] {
        let names = header_names(headers);
        assert_ne!(names.len(), 0);
        for name in SENSITIVE_HEADERS {
            assert!(
                !names.iter().any(|header| header == name),
                "{name} exported"
            );
        }
    }
    assert!(entry["request"].get("postData").is_none());
    assert!(entry["response"]["content"].get("text").is_none());
    assert_eq!(entry["response"]["content"]["size"], RESPONSE_BODY.len());
    assert_eq!(entry["request"]["cookies"], serde_json::json!([]));
    assert_eq!(entry["response"]["cookies"], serde_json::json!([]));
    assert_har_1_2(&har);
    Ok(())
}

#[test]
fn opt_in_adds_bodies_and_redacted_header_names_only() -> TestResult {
    let record = sensitive_exchange()?;

    let options = HarOptions {
        include_bodies: true,
        include_sensitive_headers: true,
    };
    let har = document(&[record], options, SystemTime::now());
    let text = serde_json::to_string(&har)?;
    for secret in SECRETS {
        assert!(!text.contains(secret), "{secret} leaked");
    }
    let entry = entries(&har).first().ok_or("missing entry")?;
    let request = &entry["request"];
    for name in ["authorization", "cookie"] {
        assert_eq!(header_value(&request["headers"], name), Some("[REDACTED]"));
    }
    assert_eq!(
        header_value(&entry["response"]["headers"], "set-cookie"),
        Some("[REDACTED]")
    );
    assert_eq!(request["postData"]["text"], REQUEST_BODY);
    assert_eq!(
        request["postData"]["mimeType"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(request["bodySize"], REQUEST_BODY.len());
    assert_eq!(
        request["queryString"],
        serde_json::json!([
            { "name": "user", "value": "alice" },
            { "name": "empty", "value": "" },
        ])
    );
    let content = &entry["response"]["content"];
    assert_eq!(content["text"], RESPONSE_BODY);
    assert_eq!(content["mimeType"], "application/json");
    assert!(content.get("comment").is_none());
    assert_har_1_2(&har);
    Ok(())
}

#[test]
fn truncated_binary_and_compressed_bodies_are_marked() -> TestResult {
    let get = || -> Result<ExchangeBuilder, Box<dyn Error>> {
        Ok(
            ExchangeBuilder::http(Method::GET, format!("http://{TEST_HOST}/"))?
                .header(HOST, HeaderValue::from_static(TEST_HOST))
                .header(CONNECTION, HeaderValue::from_static("close")),
        )
    };
    let large = "a".repeat(BODY_PREVIEW_BYTES + 100);
    let truncated = get()?.respond(
        ResponseFixture::new(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
            .header(
                CONTENT_LENGTH,
                HeaderValue::try_from(large.len().to_string())?,
            )
            .body(large.as_bytes(), BodyLifecycle::Complete),
    )?;
    let binary = get()?.respond(
        ResponseFixture::new(StatusCode::OK)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            )
            .header(CONTENT_LENGTH, HeaderValue::from_static("3"))
            .body(vec![0xff, 0x00, 0x80], BodyLifecycle::Complete),
    )?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"decoded text")?;
    let gzip = encoder.finish()?;
    let compressed = get()?.respond(
        ResponseFixture::new(StatusCode::OK)
            .header(CONTENT_TYPE, HeaderValue::from_static("text/plain"))
            .header(CONTENT_ENCODING, HeaderValue::from_static("gzip"))
            .header(
                CONTENT_LENGTH,
                HeaderValue::try_from(gzip.len().to_string())?,
            )
            .body(gzip.clone(), BodyLifecycle::Complete),
    )?;

    let options = HarOptions {
        include_bodies: true,
        include_sensitive_headers: false,
    };
    let har = document(&[truncated, binary, compressed], options, SystemTime::now());
    let [truncated, binary, compressed] = entries(&har) else {
        return Err("expected three entries".into());
    };
    let content = &truncated["response"]["content"];
    assert_eq!(
        content["text"].as_str().map(str::len),
        Some(BODY_PREVIEW_BYTES)
    );
    assert_eq!(content["size"], large.len());
    assert!(
        content["comment"]
            .as_str()
            .is_some_and(|comment| comment.starts_with("Truncated: only the first 8192 of"))
    );
    let content = &binary["response"]["content"];
    assert_eq!(content["text"], "/wCA");
    assert_eq!(content["encoding"], "base64");
    let content = &compressed["response"]["content"];
    assert_eq!(content["text"], "decoded text");
    assert_eq!(content["size"], 12);
    assert_eq!(compressed["response"]["bodySize"], gzip.len());
    assert_eq!(content["compression"], gzip.len() - 12);
    assert_har_1_2(&har);
    Ok(())
}

#[test]
fn failures_tunnels_and_pending_requests_follow_the_documented_mapping() -> TestResult {
    let host = HeaderValue::from_static(TEST_HOST);
    let tunnel = ExchangeBuilder::connect(TEST_HOST)
        .header(HOST, host.clone())
        .tunnel(4, 5, TunnelState::Closed)?;
    let pending = ExchangeBuilder::http(Method::GET, format!("http://{TEST_HOST}/slow"))?
        .header(HOST, host.clone())
        .request_body(Vec::new(), BodyLifecycle::Complete)
        .in_flight()?;
    let failed = ExchangeBuilder::http(Method::GET, format!("http://{TEST_HOST}/"))?
        .header(HOST, host)
        .header(CONNECTION, HeaderValue::from_static("close"))
        .fail(FailureStage::Connect, "connection refused")?;

    let har = document(
        &[tunnel, pending, failed],
        HarOptions::default(),
        SystemTime::now(),
    );
    assert_har_1_2(&har);
    let [tunnel, pending, failed] = entries(&har) else {
        return Err("expected three entries".into());
    };
    assert_eq!(tunnel["request"]["method"], "CONNECT");
    assert_eq!(tunnel["request"]["url"], format!("https://{TEST_HOST}"));
    assert_eq!(tunnel["request"]["bodySize"], 4);
    assert_eq!(tunnel["response"]["status"], 200);
    assert_eq!(tunnel["response"]["bodySize"], 5);
    assert!(
        tunnel["comment"]
            .as_str()
            .is_some_and(|comment| comment.starts_with("CONNECT tunnel to"))
    );
    assert_eq!(pending["response"]["status"], 0);
    assert!(pending["response"].get("_error").is_none());
    assert!(
        pending["comment"]
            .as_str()
            .is_some_and(|comment| comment.contains("Still waiting"))
    );
    assert_eq!(failed["response"]["status"], 0);
    assert!(
        failed["response"]["_error"]
            .as_str()
            .is_some_and(|error| error.starts_with("Connection"))
    );
    Ok(())
}

#[test]
fn write_creates_a_private_file_and_never_replaces_one() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("export.har");
    let har = document(&[], HarOptions::default(), SystemTime::now());
    assert_eq!(write(&path, &har)?, path);
    let written: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(written, har);
    assert_har_1_2(&written);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o077, 0);
    }
    let error = write(&path, &serde_json::json!({}))
        .err()
        .ok_or("replaced")?;
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&path)?)?,
        har
    );
    let missing = directory.path().join("missing").join("export.har");
    assert!(write(&missing, &har).is_err());
    assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
    Ok(())
}

/// Checks the fields and types HAR 1.2 requires, and that `time` is the sum
/// of the observed timings.
fn assert_har_1_2(har: &Value) {
    let log = &har["log"];
    assert_eq!(log["version"], "1.2");
    assert!(log["creator"]["name"].is_string() && log["creator"]["version"].is_string());
    assert!(log["entries"].is_array());
    for entry in entries(har) {
        assert_entry(entry);
    }
}

fn assert_entry(entry: &Value) {
    let started = entry["startedDateTime"].as_str().unwrap_or_default();
    assert_eq!(started.len(), 24, "{started}");
    assert!(started.ends_with('Z') && started.as_bytes().get(10) == Some(&b'T'));
    assert!(entry["cache"].is_object());
    let request = &entry["request"];
    for field in ["method", "url", "httpVersion"] {
        assert!(request[field].is_string(), "request.{field}");
    }
    assert!(
        request["url"]
            .as_str()
            .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"))
    );
    let response = &entry["response"];
    assert!(response["status"].is_u64());
    for field in ["statusText", "httpVersion", "redirectURL"] {
        assert!(response[field].is_string(), "response.{field}");
    }
    for message in [request, response] {
        assert_message(message);
    }
    assert!(request["queryString"].is_array());
    if let Some(post_data) = request.get("postData") {
        assert!(post_data["mimeType"].is_string() && post_data["text"].is_string());
    }
    let content = &response["content"];
    assert!(content["size"].is_i64() && content["mimeType"].is_string());
    assert_timings(entry);
}

fn assert_message(message: &Value) {
    for field in ["cookies", "headers"] {
        assert!(message[field].is_array(), "{field}");
    }
    for field in ["headersSize", "bodySize"] {
        assert!(message[field].is_i64(), "{field}");
    }
    for header in message["headers"].as_array().into_iter().flatten() {
        assert!(header["name"].is_string() && header["value"].is_string());
    }
}

/// Checks that required phases are observed, optional ones are `-1` when
/// not, and `time` is their sum (`ssl` is already part of `connect`).
fn assert_timings(entry: &Value) {
    let timings = &entry["timings"];
    let mut sum = 0.0;
    for phase in [
        "blocked", "dns", "connect", "send", "wait", "receive", "ssl",
    ] {
        let value = timings[phase].as_f64().unwrap_or(f64::NAN);
        let required = matches!(phase, "send" | "wait" | "receive");
        assert!(
            value >= 0.0 || (!required && (value + 1.0).abs() < f64::EPSILON),
            "{phase}"
        );
        if value >= 0.0 && phase != "ssl" {
            sum += value;
        }
    }
    let time = entry["time"].as_f64().unwrap_or(f64::NAN);
    assert!((time - sum).abs() < 0.01, "time {time} != timings {sum}");
}
