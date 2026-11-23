//! End-to-end check that live proxy traffic produces per-endpoint request
//! and response structure without keeping any value.

use std::time::Duration;

use http::Method;
use http_proxy::endpoint_schema::{
    CapturedBodies, EndpointKey, EndpointSchema, EndpointSchemas, ExclusionReason, FieldShape,
    ParamStats, ResponseKey, SchemaAnalyzer, ValueKind,
};
use http_proxy::url_map::{
    RequestScope, SortDirection, TreeFilter, TreeSortField, UrlMap, ViewNode, build_view_roots,
};
use http_proxy::{AuthScheme, BodyStore, Exchange, ProxyConfig, ProxyHandle};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

struct Case {
    method: &'static str,
    path: &'static str,
    headers: &'static str,
    body: String,
    response: String,
}

fn json_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn cases() -> Vec<Case> {
    // Larger than the 8 KiB preview, so it is read back from the body store.
    let padding = "p".repeat(12 * 1024);
    let large = format!(r#"{{"id":303,"name":"SECRET_NAME_3","note":"{padding}"}}"#);
    vec![
        Case {
            method: "GET",
            path: "/users/101?page=1&token=SECRET_QUERY_1",
            headers: "Authorization: Bearer SECRET_BEARER_1\r\nX-Client-Version: 7\r\n",
            body: String::new(),
            response: json_response(
                "200 OK",
                r#"{"id":101,"name":"SECRET_NAME_1","email":null,"tags":["a"]}"#,
            ),
        },
        Case {
            method: "GET",
            path: "/users/202?page=two",
            headers: "Cookie: session=SECRET_COOKIE_1\r\n",
            body: String::new(),
            response: json_response("200 OK", r#"{"id":"u-202","name":"SECRET_NAME_2"}"#),
        },
        Case {
            method: "GET",
            path: "/users/303",
            headers: "",
            body: String::new(),
            response: json_response("200 OK", &large),
        },
        Case {
            method: "GET",
            path: "/users/404",
            headers: "",
            body: String::new(),
            // Declares more bytes than it sends: the body ends interrupted.
            response: "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"id\":4".to_owned(),
        },
        Case {
            method: "POST",
            path: "/users/505",
            headers: "Content-Type: application/json\r\n",
            body: r#"{"name":"SECRET_BODY_1","age":30}"#.to_owned(),
            response: json_response("422 Unprocessable Entity", r#"{"error":"invalid"}"#),
        },
        Case {
            method: "POST",
            path: "/users/606",
            headers: "Content-Type: application/x-www-form-urlencoded\r\n",
            body: "name=SECRET_FORM_1&age=31".to_owned(),
            response: json_response("201 Created", r#"{"id":606}"#),
        },
    ]
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut byte = [0; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await?;
        head.extend_from_slice(&byte);
    }
    let text = String::from_utf8_lossy(&head).to_ascii_lowercase();
    let length = text
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(())
}

async fn run_case(
    proxy: &mut ProxyHandle,
    upstream: &TcpListener,
    address: std::net::SocketAddr,
    case: &Case,
) -> Result<Exchange, Box<dyn std::error::Error>> {
    let mut client = TcpStream::connect(proxy.local_addr()).await?;
    let length = if case.body.is_empty() {
        String::new()
    } else {
        format!("Content-Length: {}\r\n", case.body.len())
    };
    let raw = format!(
        "{} http://{address}{} HTTP/1.1\r\nHost: {address}\r\n{}{length}Connection: close\r\n\r\n{}",
        case.method, case.path, case.headers, case.body
    );
    client.write_all(raw.as_bytes()).await?;
    let (mut server, _) = upstream.accept().await?;
    let record = proxy.receive().await.ok_or("missing exchange")?;
    read_request(&mut server).await?;
    server.write_all(case.response.as_bytes()).await?;
    drop(server);
    let mut sink = Vec::new();
    let _ = client.read_to_end(&mut sink).await;
    Ok(record)
}

async fn wait_for(
    analyzer: &mut SchemaAnalyzer,
    schemas: &mut EndpointSchemas,
    expected: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    while schemas.version() < expected {
        let _ = analyzer.drain_into(schemas);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    Ok(())
}

fn folded_node(map: &UrlMap) -> Result<ViewNode, Box<dyn std::error::Error>> {
    let roots = build_view_roots(
        map,
        &TreeFilter::default(),
        TreeSortField::Name,
        SortDirection::Asc,
    );
    let users = roots
        .first()
        .and_then(|root| root.children.first())
        .ok_or("missing /users")?;
    Ok(users.children.first().ok_or("missing /users/{id}")?.clone())
}

fn field<'a>(shape: &'a FieldShape, name: &str) -> Result<&'a FieldShape, String> {
    shape
        .fields()
        .get(name)
        .ok_or_else(|| format!("missing field {name}"))
}

fn assert_get(get: &EndpointSchema) -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(get.requests(), 4);
    assert_eq!(get.query().get("page").map(ParamStats::seen), Some(2));
    assert!(
        get.query()
            .get("page")
            .is_some_and(|param| param.types().is_mixed())
    );
    assert_eq!(get.headers().get("x-client-version"), Some(&1));
    assert_eq!(get.auth().get(&AuthScheme::Bearer), Some(&1));
    assert_eq!(get.auth().get(&AuthScheme::Cookie), Some(&1));
    let ok = get
        .responses()
        .get(&ResponseKey {
            status: 200,
            content_type: "application/json".to_owned(),
        })
        .ok_or("missing 200 group")?;
    assert_eq!(ok.structured_samples(), 3);
    assert_eq!(ok.excluded().get(&ExclusionReason::Truncated), Some(&1));
    let root = ok.fields();
    assert!(field(root, "id")?.types().is_mixed());
    assert_eq!(field(root, "email")?.types().count(ValueKind::Null), 1);
    assert_eq!(field(root, "email")?.seen(), 1);
    assert_eq!(field(root, "name")?.seen(), 3);
    assert_eq!(field(root, "note")?.seen(), 1);
    Ok(())
}

fn assert_post(post: &EndpointSchema) {
    assert_eq!(post.requests(), 2);
    assert_eq!(
        post.request_bodies()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["application/json", "application/x-www-form-urlencoded"]
    );
    assert_eq!(
        post.responses()
            .keys()
            .map(|key| key.status)
            .collect::<Vec<_>>(),
        [201, 422]
    );
}

#[tokio::test]
async fn live_traffic_builds_endpoint_structure_without_values()
-> Result<(), Box<dyn std::error::Error>> {
    timeout(Duration::from_secs(20), async {
        let root = tempfile::tempdir()?;
        let store = BodyStore::with_limits(root.path(), 64 * 1024 * 1024, 0)?;
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let address = upstream.local_addr()?;
        let mut proxy =
            ProxyHandle::start(ProxyConfig::loopback(0).with_body_store(store.clone()))?;
        let mut analyzer = SchemaAnalyzer::spawn(CapturedBodies::new(Some(store)), || {})?;
        let mut schemas = EndpointSchemas::new();
        let mut map = UrlMap::new();
        let mut retained = Vec::new();
        let cases = cases();
        for case in &cases {
            let record = run_case(&mut proxy, &upstream, address, case).await?;
            let node = map.observe_exchange(&record).ok_or("record not mapped")?;
            let method = record.request().method().clone();
            analyzer.submit(EndpointKey { node, method }, record.clone());
            retained.push(record);
        }
        wait_for(&mut analyzer, &mut schemas, 6).await?;

        // `/users/101` … `/users/606` fold into `/users/{id}`, and so do their aggregates.
        let node = folded_node(&map)?;
        assert_eq!(node.label, "/{id}");
        let merged = schemas.merged(&node.raw_node_ids);
        let methods: Vec<&str> = merged.iter().map(|(method, _)| method.as_str()).collect();
        assert_eq!(methods, ["GET", "POST"]);
        let [(_, get), (_, post)] = merged.as_slice() else {
            return Err("expected GET and POST".into());
        };
        assert_get(get)?;
        assert_post(post);

        let debug = format!("{merged:?}");
        for secret in [
            "SECRET_QUERY_1",
            "SECRET_BEARER_1",
            "SECRET_COOKIE_1",
            "SECRET_NAME_1",
            "SECRET_NAME_3",
            "SECRET_BODY_1",
            "SECRET_FORM_1",
            "two",
        ] {
            assert!(!debug.contains(secret), "schema kept {secret}");
        }

        // Show in Requests for one endpoint lists only that method on that path.
        let scope = RequestScope::endpoint(node.key.clone(), Method::POST);
        let shown = retained
            .iter()
            .filter(|record| scope.matches_exchange(record));
        assert_eq!(shown.count(), 2);

        // Dropping the retained requests keeps the aggregate; Clear resets it.
        drop(retained);
        assert_eq!(schemas.merged(&node.raw_node_ids).len(), 2);
        schemas.clear();
        analyzer.reset();
        assert_eq!(schemas.merged(&node.raw_node_ids).len(), 0);
        proxy.stop()?;
        Ok(())
    })
    .await?
}
