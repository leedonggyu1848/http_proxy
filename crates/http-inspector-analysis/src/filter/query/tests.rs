use http::header::{AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use http_inspector_exchange::{
    BodyLifecycle, ExchangeBuilder, FailureStage, ResponseFixture, TunnelState,
};

use super::{FilterKey, FilterQuery, TextTerm};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn preserves_plain_words_urls_addresses_and_quoted_phrases() {
    let query = FilterQuery::parse(
        r#"localhost:8080 127.0.0.1:8080 [::1]:8080 http://example.test/a HTTPS://example.test/b "connection refused" -token "tag:value""#,
    );
    assert!(!query.has_fields());
    assert!(query.diagnostics.is_empty());
    assert_eq!(
        query.text,
        [
            ("localhost:8080", false),
            ("127.0.0.1:8080", false),
            ("[::1]:8080", false),
            ("http://example.test/a", false),
            ("HTTPS://example.test/b", false),
            ("connection refused", false),
            ("token", true),
            ("tag:value", false),
        ]
        .map(|(text, excluded)| TextTerm {
            text: text.into(),
            excluded
        })
    );
}

#[test]
fn skips_invalid_terms_and_reports_their_spans() -> TestResult {
    let input = "한글 methd:POST status:abc method:GET size:>abc duration:12m status:400, body:token url:\"oops";
    let query = FilterQuery::parse(input);
    let terms: Vec<_> = query
        .diagnostics
        .iter()
        .filter_map(|error| input.get(error.span.clone()))
        .collect();
    assert_eq!(
        terms,
        [
            "methd:POST",
            "status:abc",
            "size:>abc",
            "duration:12m",
            "status:400,",
            "body:token",
            "url:\"oops"
        ]
    );
    for (error, message) in query.diagnostics.iter().zip([
        "Unknown filter key",
        "Invalid status",
        "Invalid size",
        "Invalid duration",
        "Missing value",
        "Reserved filter key",
        "Unclosed quote",
    ]) {
        assert!(error.message.starts_with(message), "{}", error.message);
    }
    let get = ExchangeBuilder::http(Method::GET, "http://localhost:8080/")?.in_flight()?;
    let post = ExchangeBuilder::http(Method::POST, "http://localhost:8080/")?.in_flight()?;
    assert!(query.matches(&get));
    assert!(!query.matches(&post));
    Ok(())
}

#[test]
fn matches_fields_and_combinations_against_captured_values() -> TestResult {
    let record = ExchangeBuilder::http(
        Method::POST,
        "http://api.example.test:8080/api/users?page=1",
    )?
    .header(
        AUTHORIZATION,
        HeaderValue::from_static("Bearer PRIVATE_SENTINEL"),
    )
    .header(COOKIE, HeaderValue::from_static("session=PRIVATE_SENTINEL"))
    .header(
        HeaderName::from_static("x-tag"),
        HeaderValue::from_static("first, second"),
    )
    .header(
        HeaderName::from_static("x-empty"),
        HeaderValue::from_static(""),
    )
    .respond(
        ResponseFixture::new(StatusCode::NOT_FOUND)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/problem+json; charset=utf-8"),
            )
            .header(
                SET_COOKIE,
                HeaderValue::from_static("session=PRIVATE_SENTINEL"),
            )
            .body(vec![b'x'; 11 * 1024], BodyLifecycle::Complete),
    )?;
    for (input, expected) in [
        ("method:POST url:/api/users", true),
        ("method:POST url:/api/orders", false),
        ("MeThOd:GET,pOsT -url:/static/ status:4XX", true),
        ("method:POS", false),
        ("-method:POST,GET", false),
        ("host:API.EXAMPLE.TEST:8080", true),
        ("domain:*.example.test", true),
        ("host:*.example.test:8080", true),
        ("host:*.example.test:80", false),
        ("host:*.api.example.test", false),
        ("url:localhost", false),
        ("path:?PAGE=1", true),
        ("path:api.example.test", false),
        (
            "status:404 status:>=400 status:<405 status:<=404 status:=404",
            true,
        ),
        ("status:200,404", true),
        ("status:>404", false),
        ("status:400", false),
        ("type:JSON", true),
        ("type:problem+json", false),
        (
            "header:Authorization req-header:cookie res-header:set-cookie",
            true,
        ),
        ("req-header:set-cookie", false),
        ("res-header:authorization", false),
        ("header:authorization=PRIVATE_SENTINEL", false),
        ("header:cookie=PRIVATE_SENTINEL", false),
        ("res-header:set-cookie=session", false),
        ("header:authorization=[redacted]", true),
        ("res-header:content-type=problem+json", true),
        (r#"header:"x-tag=first, second",x-none"#, true),
        ("header:x-empty=", true),
        ("header:x-tag=", false),
        ("size:>10kb size:<1MB size:=11264B", true),
        ("size:<=10kb", false),
        ("duration:<1s time:<=1000ms", true),
        ("is:failed,pending,tunnel,replay,https", false),
    ] {
        let query = FilterQuery::parse(input);
        assert!(
            query.diagnostics.is_empty(),
            "{input}: {:?}",
            query.diagnostics
        );
        assert_eq!(query.matches(&record), expected, "{input}");
    }
    Ok(())
}

#[test]
fn matches_outcome_types_connection_targets_and_replays() -> TestResult {
    let pending = ExchangeBuilder::http(Method::GET, "http://localhost:8080/")?.in_flight()?;
    let failed = ExchangeBuilder::http(Method::GET, "http://localhost:8080/")?
        .fail(FailureStage::Connect, "connection refused")?;
    let tunnel =
        ExchangeBuilder::connect("api.example.test:443").tunnel(0, 100, TunnelState::Closed)?;
    let inspection = ExchangeBuilder::connect("api.example.test:443").inspection_established()?;
    let replay = ExchangeBuilder::http(Method::GET, "http://localhost:8080/replay")?
        .replay_of(pending.id())
        .respond(ResponseFixture::new(StatusCode::OK))?;
    for (record, yes, no) in [
        (&pending, "is:pending", "status:>=100"),
        (&failed, "is:failed", "size:>0"),
        (
            &tunnel,
            "is:tunnel is:https host:api.example.test:443",
            "path:/",
        ),
        (&inspection, "is:https", "is:tunnel"),
        (&replay, "is:replay", "is:https"),
    ] {
        assert!(FilterQuery::parse(yes).matches(record), "{yes}");
        assert!(!FilterQuery::parse(no).matches(record), "{no}");
    }
    Ok(())
}

#[test]
fn completion_requires_a_whole_unquoted_key() -> TestResult {
    let text = r#"header:"한글 \"quoted status:" -DOMAIN:"#;
    let inside = text.find("status:").ok_or("missing quoted status")?;
    assert_eq!(
        FilterKey::for_completion(text, inside..inside + "status:".len()),
        None
    );
    let hostname = text.find("-DOMAIN:").ok_or("missing host key")?;
    assert_eq!(
        FilterKey::for_completion(text, hostname..text.len()),
        Some((FilterKey::Host, true))
    );
    assert_eq!(FilterKey::for_completion("status:404", 0..7), None);
    assert_eq!(
        FilterKey::for_completion(r#""unclosed status:"#, 10..17),
        None
    );
    Ok(())
}
