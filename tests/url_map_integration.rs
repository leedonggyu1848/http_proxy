//! End-to-end integration tests verifying live [`ProxyHandle`] traffic feeds
//! [`UrlMap`] accurately with strict query-value privacy and edge tracking.

use std::net::SocketAddr;
use std::time::Duration;

use http_proxy::interop::map_export::{
    export_graph_dot, export_graph_mermaid, export_tree_json, export_tree_text,
};
use http_proxy::url_map::{
    EdgeKind, OriginKey, PathSegmentPattern, SortDirection, TreeFilter, TreeSortField, UrlMap,
    ViewNodeKey, build_view_roots,
};
use http_proxy::{Exchange, ProxyConfig, ProxyHandle};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

pub mod common;

use common::{TestError, read_headers};

async fn run_live_traffic(
    upstream: &TcpListener,
    address: SocketAddr,
    proxy: &mut ProxyHandle,
    map: &mut UrlMap,
) -> Result<Vec<Exchange>, TestError> {
    let mut captured = Vec::new();
    let requests = [
        (
            "GET",
            "/users/101/orders?token=SECRET_ALPHA_1&page=1",
            "Referer: http://portal.example.test/home?sid=SECRET_REF_1\r\nOrigin: http://portal.example.test\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 14\r\nConnection: close\r\n\r\n{\"ok\":true,  }",
        ),
        (
            "POST",
            "/users/202/orders?token=SECRET_ALPHA_2&sort=desc",
            "Referer: http://portal.example.test/home?sid=SECRET_REF_2\r\n",
            "HTTP/1.1 302 Found\r\nLocation: /users/202/receipt?receipt_key=SECRET_LOC_1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ),
        (
            "GET",
            "/users/settings",
            "",
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/plain\r\nContent-Length: 4\r\nConnection: close\r\n\r\nfail",
        ),
    ];

    for (method, path, extra_headers, response) in requests {
        let mut client = TcpStream::connect(proxy.local_addr()).await?;
        let raw = format!(
            "{method} http://{address}{path} HTTP/1.1\r\nHost: {address}\r\n{extra_headers}Connection: close\r\n\r\n"
        );
        client.write_all(raw.as_bytes()).await?;
        let (mut server, _) = upstream.accept().await?;
        let record = proxy.receive().await.ok_or("missing exchange")?;
        let _ = map.observe_exchange(&record);
        let _head = read_headers(&mut server).await?;
        server.write_all(response.as_bytes()).await?;
        drop(server);
        let mut sink = Vec::new();
        client.read_to_end(&mut sink).await?;
        while record.is_live() {
            let _ = map.sync_live();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let _ = map.sync_live();
        captured.push(record);
    }

    run_tunnel_exchange(proxy, upstream, address, map).await?;
    Ok(captured)
}

async fn run_tunnel_exchange(
    proxy: &mut ProxyHandle,
    upstream: &TcpListener,
    address: std::net::SocketAddr,
    map: &mut UrlMap,
) -> Result<(), TestError> {
    let mut tunnel_client = TcpStream::connect(proxy.local_addr()).await?;
    tunnel_client
        .write_all(format!("CONNECT {address} HTTP/1.1\r\nHost: {address}\r\n\r\n").as_bytes())
        .await?;
    let (mut tunnel_server, _) = upstream.accept().await?;
    let _head = read_headers(&mut tunnel_client).await?;
    let tunnel_record = proxy.receive().await.ok_or("missing tunnel exchange")?;
    let _ = map.observe_exchange(&tunnel_record);
    // Exchange one byte through the live tunnel so hyper upgrade has completed
    // before closing both halves with FIN instead of TCP RST.
    tunnel_client.write_all(b"x").await?;
    let mut ping = [0; 1];
    tunnel_server.read_exact(&mut ping).await?;
    tunnel_client.shutdown().await?;
    tunnel_server.shutdown().await?;
    drop(tunnel_server);
    drop(tunnel_client);
    while tunnel_record.is_live() {
        let _ = map.sync_live();
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let _ = map.sync_live();
    Ok(())
}

fn assert_exports_omit_secrets(map: &UrlMap) {
    let text = export_tree_text(map, true);
    let json = export_tree_json(map, true).to_string();
    let mermaid = export_graph_mermaid(map, 40);
    let dot = export_graph_dot(map, 40);
    for secret in [
        "SECRET_ALPHA_1",
        "SECRET_ALPHA_2",
        "SECRET_REF_1",
        "SECRET_REF_2",
        "SECRET_LOC_1",
    ] {
        assert!(!text.contains(secret), "text leaked {secret}");
        assert!(!json.contains(secret), "json leaked {secret}");
        assert!(!mermaid.contains(secret), "mermaid leaked {secret}");
        assert!(!dot.contains(secret), "dot leaked {secret}");
    }
}

#[tokio::test]
async fn live_proxy_traffic_populates_tree_edges_tunnels_without_query_values()
-> Result<(), TestError> {
    timeout(Duration::from_secs(10), async {
        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let address = upstream.local_addr()?;
        let mut proxy = ProxyHandle::start(ProxyConfig::loopback(0))?;
        let mut map = UrlMap::new();
        let captured = run_live_traffic(&upstream, address, &mut proxy, &mut map).await?;

        let roots = build_view_roots(
            &map,
            &TreeFilter {
                fold_ids: true,
                ..TreeFilter::default()
            },
            TreeSortField::Name,
            SortDirection::Asc,
        );
        assert_eq!(roots.len(), 2);

        let plain_origin = OriginKey::new("http", "127.0.0.1", Some(address.port()));
        let tunnel_origin = OriginKey::new("https", "127.0.0.1", Some(address.port()));
        let plain_root = roots
            .iter()
            .find(|r| r.key.origin == plain_origin)
            .ok_or("missing http origin")?;
        let tunnel_root = roots
            .iter()
            .find(|r| r.key.origin == tunnel_origin)
            .ok_or("missing tunnel origin")?;

        assert!(tunnel_root.subtree.is_tunnel_only());
        assert!(tunnel_root.children.is_empty());
        assert_eq!(plain_root.subtree.request_count, 3);
        assert_eq!(plain_root.subtree.status.s2xx, 1);
        assert_eq!(plain_root.subtree.status.s3xx, 1);
        assert_eq!(plain_root.subtree.status.s5xx, 1);
        assert!(plain_root.subtree.total_bytes >= 18);
        assert!(plain_root.subtree.query_params.contains_key("token"));
        assert!(plain_root.subtree.query_params.contains_key("page"));
        assert!(plain_root.subtree.query_params.contains_key("sort"));

        let folded_orders_key = ViewNodeKey {
            origin: plain_origin,
            path: vec![
                PathSegmentPattern::Literal("users".to_owned()),
                PathSegmentPattern::FoldedId,
                PathSegmentPattern::Literal("orders".to_owned()),
            ],
        };
        assert_eq!(map.matching_request_count(&folded_orders_key), 2);
        assert_eq!(
            captured
                .iter()
                .filter(|rec| folded_orders_key.matches_exchange(rec))
                .count(),
            2
        );

        for kind in [EdgeKind::Referer, EdgeKind::Origin, EdgeKind::Redirect] {
            assert!(
                map.graph().domain_edges().keys().any(|k| k.kind == kind),
                "missing edge kind {kind:?}"
            );
        }
        assert_exports_omit_secrets(&map);

        proxy.stop()?;
        Ok::<_, TestError>(())
    })
    .await??;
    Ok(())
}
