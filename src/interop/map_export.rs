//! Exporters for the URL map tree (indented text, JSON) and domain relationship
//! graph (Mermaid flowchart, Graphviz DOT).
//!
//! Privacy guarantee: query parameter values are never present in [`UrlMap`]
//! and are therefore never written to any exported format.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::analysis::url_map::{
    EdgeKind, ForceLayout, NodeStats, SortDirection, TreeFilter, TreeSortField, UrlMap, ViewNode,
    build_view_roots,
};
use crate::exchange::ByteSize;

/// Formats the URL map tree as indented plain text.
#[must_use]
pub fn export_tree_text(map: &UrlMap, fold_ids: bool) -> String {
    let filter = TreeFilter {
        fold_ids,
        ..TreeFilter::default()
    };
    let roots = build_view_roots(map, &filter, TreeSortField::Name, SortDirection::Asc);
    let mut out = String::new();
    for root in &roots {
        write_text_node(root, &mut out);
    }
    out
}

fn write_text_node(node: &ViewNode, out: &mut String) {
    for _ in 0..node.depth {
        out.push_str("  ");
    }
    let stats = &node.subtree;
    let _ = write!(out, "{}", node.label);
    if stats.is_tunnel_only() {
        let _ = write!(out, " [{} req, tunnel (no path info)]", stats.request_count);
    } else {
        let methods = stats.methods.names().join(" ");
        let summary = format_status_summary(stats);
        let _ = write!(
            out,
            " [{} req, {summary}, {methods}, {}]",
            stats.request_count,
            ByteSize::new(stats.total_bytes),
        );
    }
    if stats.is_third_party() {
        out.push_str(" [3rd-party]");
    }
    if !node.direct.query_params.is_empty() {
        let names: Vec<&str> = node
            .direct
            .query_params
            .keys()
            .map(String::as_str)
            .collect();
        let _ = write!(out, " ?{}", names.join(","));
    }
    out.push('\n');
    for child in &node.children {
        write_text_node(child, out);
    }
}

fn format_status_summary(stats: &NodeStats) -> String {
    let mut parts = Vec::new();
    if stats.status.s2xx > 0 {
        parts.push(format!("2xx:{}", stats.status.s2xx));
    }
    if stats.status.s3xx > 0 {
        parts.push(format!("3xx:{}", stats.status.s3xx));
    }
    if stats.status.s4xx > 0 {
        parts.push(format!("4xx:{}", stats.status.s4xx));
    }
    if stats.status.s5xx > 0 {
        parts.push(format!("5xx:{}", stats.status.s5xx));
    }
    if stats.status.failed > 0 {
        parts.push(format!("failed:{}", stats.status.failed));
    }
    if stats.status.tunnel > 0 {
        parts.push(format!("tunnel:{}", stats.status.tunnel));
    }
    if stats.status.pending > 0 {
        parts.push(format!("pending:{}", stats.status.pending));
    }
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(" ")
    }
}

/// Serializes the URL map (nodes, aggregates, query parameter names, and edges) as JSON.
#[must_use]
pub fn export_tree_json(map: &UrlMap, fold_ids: bool) -> Value {
    let filter = TreeFilter {
        fold_ids,
        ..TreeFilter::default()
    };
    let roots = build_view_roots(map, &filter, TreeSortField::Name, SortDirection::Asc);
    let nodes: Vec<Value> = roots.iter().map(view_node_to_json).collect();

    let domain_edges: Vec<Value> = map
        .graph()
        .domain_edges()
        .iter()
        .map(|(key, stats)| {
            json!({
                "from": key.from_host,
                "to": key.to_host,
                "kind": key.kind.label(),
                "count": stats.count,
                "third_party": stats.third_party,
            })
        })
        .collect();

    let url_edges: Vec<Value> = map
        .graph()
        .url_edges()
        .iter()
        .map(|(key, &count)| {
            json!({
                "from": key.from_url,
                "to": key.to_url,
                "kind": key.kind.label(),
                "count": count,
            })
        })
        .collect();

    json!({
        "format": "http-inspector-url-map",
        "version": 1,
        "fold_ids": fold_ids,
        "node_count": map.node_count(),
        "node_limit_reached": map.node_limit_reached(),
        "nodes": nodes,
        "edges": {
            "domain": domain_edges,
            "url": url_edges,
        },
    })
}

fn view_node_to_json(node: &ViewNode) -> Value {
    let kind = if node.is_origin && node.subtree.is_tunnel_only() {
        "tunnel"
    } else if node.is_origin {
        "origin"
    } else if node.is_folded_id {
        "folded_id"
    } else {
        "segment"
    };
    let children: Vec<Value> = node.children.iter().map(view_node_to_json).collect();
    let mut query_params = Map::new();
    for (name, &count) in &node.direct.query_params {
        let _ = query_params.insert(name.clone(), json!(count));
    }
    json!({
        "url": node.key.display_url(),
        "label": node.label,
        "kind": kind,
        "third_party": node.subtree.is_third_party(),
        "folded_count": node.folded_count,
        "stats": stats_to_json(&node.subtree),
        "query_params": Value::Object(query_params),
        "children": children,
    })
}

fn stats_to_json(stats: &NodeStats) -> Value {
    let mut content_types = Map::new();
    for (ct, &count) in &stats.content_types {
        let _ = content_types.insert(ct.clone(), json!(count));
    }
    json!({
        "requests": stats.request_count,
        "methods": stats.methods.names(),
        "status": {
            "2xx": stats.status.s2xx,
            "3xx": stats.status.s3xx,
            "4xx": stats.status.s4xx,
            "5xx": stats.status.s5xx,
            "failed": stats.status.failed,
            "tunnel": stats.status.tunnel,
            "pending": stats.status.pending,
        },
        "primary_content_type": stats.primary_content_type(),
        "content_types": Value::Object(content_types),
        "total_bytes": stats.total_bytes,
        "avg_duration_ms": stats.avg_duration_ms(),
        "max_duration_ms": stats.max_duration_ms(),
        "first_seen_unix_ms": stats.first_seen.and_then(unix_millis),
        "last_seen_unix_ms": stats.last_seen.and_then(unix_millis),
        "overflow_merged": stats.overflow_merged,
    })
}

fn unix_millis(time: SystemTime) -> Option<u64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(duration.as_millis()).ok()
}

/// Exports the top `max_domains` domain relationship graph as a Mermaid flowchart.
#[must_use]
pub fn export_graph_mermaid(map: &UrlMap, max_domains: usize) -> String {
    let mut layout = ForceLayout::new(max_domains);
    layout.sync(map.graph());

    let mut out = String::from("flowchart LR\n");
    for (index, node) in layout.nodes().iter().enumerate() {
        let party = if node.third_party { " (3rd-party)" } else { "" };
        let label = escape_mermaid(&format!(
            "{} ({} req){party}",
            node.host, node.request_count
        ));
        if node.third_party {
            let _ = writeln!(out, "    d{index}([\"{label}\"]):::thirdParty");
        } else {
            let _ = writeln!(out, "    d{index}[\"{label}\"]:::firstParty");
        }
    }
    for edge in layout.edges() {
        let arrow = match edge.kind {
            EdgeKind::Referer => "-->",
            EdgeKind::Origin => "-.->",
            EdgeKind::Redirect => "==>",
        };
        let label = format!("{} ({})", edge.kind.label(), edge.count);
        let _ = writeln!(out, "    d{} {arrow}|\"{label}\"| d{}", edge.from, edge.to);
    }
    out.push_str("    classDef firstParty stroke:#0065c8,stroke-width:2px;\n");
    out.push_str(
        "    classDef thirdParty stroke:#c87800,stroke-width:3px,stroke-dasharray: 5 3;\n",
    );
    out
}

/// Exports the top `max_domains` domain relationship graph as Graphviz DOT.
#[must_use]
pub fn export_graph_dot(map: &UrlMap, max_domains: usize) -> String {
    let mut layout = ForceLayout::new(max_domains);
    layout.sync(map.graph());

    let mut out = String::from("digraph url_map {\n    rankdir=LR;\n");
    for (index, node) in layout.nodes().iter().enumerate() {
        let party = if node.third_party {
            "\\n[3rd-party]"
        } else {
            ""
        };
        let host = escape_dot(&node.host);
        let label = format!("{host}\\n{} req{party}", node.request_count);
        let style = if node.third_party {
            "dashed,bold"
        } else {
            "solid"
        };
        let _ = writeln!(
            out,
            "    d{index} [label=\"{label}\", shape=box, style=\"{style}\"];"
        );
    }
    for edge in layout.edges() {
        let style = match edge.kind {
            EdgeKind::Referer => "solid",
            EdgeKind::Origin => "dashed",
            EdgeKind::Redirect => "bold",
        };
        let label = format!("{} ({})", edge.kind.label(), edge.count);
        let _ = writeln!(
            out,
            "    d{} -> d{} [label=\"{label}\", style=\"{style}\"];",
            edge.from, edge.to
        );
    }
    out.push_str("}\n");
    out
}

fn escape_mermaid(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "'")
}

fn escape_dot(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use http::Method;

    use super::*;
    use crate::analysis::url::{parse_origin_header, parse_url};
    use crate::analysis::url_map::{ObservedInput, StatusBucket};

    #[test]
    fn exports_contain_structure_and_never_leak_query_values() {
        let mut map = UrlMap::new();
        let Some(page) = parse_url("https://app.example.com/dashboard?session=TOP_SECRET_1") else {
            unreachable!("valid url");
        };
        let Some(api) = parse_url("https://api.example.com/v1/items?token=TOP_SECRET_2") else {
            unreachable!("valid url");
        };
        let Some(script) = parse_url("https://cdn.tracker.net/tag.js?key=TOP_SECRET_3") else {
            unreachable!("valid url");
        };
        let Some(redir) = parse_url("https://auth.example.com/login?next=TOP_SECRET_4") else {
            unreachable!("valid url");
        };

        let _ = map.observe(ObservedInput {
            target: page.clone(),
            method: Method::GET,
            status: StatusBucket::S2xx,
            content_type: Some("text/html".to_owned()),
            bytes: 1_500,
            duration: Some(Duration::from_millis(12)),
            started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            referer: None,
            origin_header: None,
            redirect_location: None,
        });
        let _ = map.observe(ObservedInput {
            target: api,
            method: Method::POST,
            status: StatusBucket::S3xx,
            content_type: Some("application/json".to_owned()),
            bytes: 400,
            duration: Some(Duration::from_millis(20)),
            started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_001),
            referer: Some(page.clone()),
            origin_header: parse_origin_header("https://app.example.com"),
            redirect_location: Some(redir),
        });
        let _ = map.observe(ObservedInput {
            target: script,
            method: Method::GET,
            status: StatusBucket::S2xx,
            content_type: Some("application/javascript".to_owned()),
            bytes: 800,
            duration: Some(Duration::from_millis(5)),
            started_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_002),
            referer: Some(page),
            origin_header: None,
            redirect_location: None,
        });

        let text = export_tree_text(&map, true);
        let json_str = export_tree_json(&map, true).to_string();
        let mermaid = export_graph_mermaid(&map, 40);
        let dot = export_graph_dot(&map, 40);

        for secret in [
            "TOP_SECRET_1",
            "TOP_SECRET_2",
            "TOP_SECRET_3",
            "TOP_SECRET_4",
        ] {
            assert!(!text.contains(secret), "text leaked {secret}");
            assert!(!json_str.contains(secret), "json leaked {secret}");
            assert!(!mermaid.contains(secret), "mermaid leaked {secret}");
            assert!(!dot.contains(secret), "dot leaked {secret}");
        }

        assert!(text.contains("?session"));
        assert!(text.contains("?token"));
        assert!(text.contains("[3rd-party]"));
        assert!(text.contains("1.4 KiB]"), "text was:\n{text}");
        assert!(text.contains("400 B]"), "text was:\n{text}");
        assert!(text.contains("800 B]"), "text was:\n{text}");
        assert!(json_str.contains("\"format\":\"http-inspector-url-map\""));
        assert!(mermaid.starts_with("flowchart LR\n"));
        assert!(mermaid.contains("Referer"));
        assert!(mermaid.contains("Origin"));
        assert!(mermaid.contains("Redirect"));
        assert!(dot.starts_with("digraph url_map {\n"));
        assert!(dot.contains("->"));
    }
}
