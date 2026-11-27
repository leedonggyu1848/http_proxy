//! URL map, relationship graph, legend, node detail, and endpoint schema UI text catalog.

use http_inspector_exchange::ByteSize;

use super::Language;

define_catalog! {
    pub(crate) struct MapText {
        map_view,
        graph,
        filter_hosts,
        fold_ids,
        relayout,
        edges,
        legend,
        legend_node_desc,
        legend_node_size,
        legend_node_size_desc,
        legend_first_party_desc,
        legend_third_party_desc,
        legend_referer_desc,
        legend_origin_desc,
        legend_redirect_desc,
        legend_active,
        legend_active_desc,
        legend_self_loop,
        legend_self_loop_desc,
        legend_drag,
        legend_drag_desc,
        legend_click,
        legend_click_desc,
        export,
        save_tree_text,
        save_map_json,
        save_graph_mermaid,
        save_graph_dot,
        copy_mermaid,
        copy_dot,
        no_relationships,
        referer,
        origin_cors,
        redirect,
        no_urls_yet,
        no_nodes_match,
        node,
        no_node_selected,
        show_in_requests,
        summary,
        methods,
        content_type,
        transferred,
        response_time,
        observed,
        party,
        folded_ids,
        no_path_info,
        merged,
        query_names,
        endpoints,
        invalid_target,
        status_err_short,
        status_tun_short,
        status_pend_short,
        endpoint_query_params,
        endpoint_none_observed,
        endpoint_custom_headers,
        endpoint_auth,
        endpoint_no_credentials,
        endpoint_request_body,
        endpoint_no_request_bodies,
        endpoint_responses,
        endpoint_no_responses,
        endpoint_empty,
        endpoint_unstructured,
        endpoint_invalid,
        endpoint_file,
        endpoint_field,
        endpoint_part,
        endpoint_header_item,
        endpoint_query_item,
        endpoint_body_group_item,
        endpoint_depth_limited,
        endpoint_optional_suffix,
        endpoint_nullable_suffix,
        endpoint_mixed_suffix,
    }
    tooltips: pub(crate) struct MapTooltips {
        clear_hint,
        fold_ids_hint,
        ascending,
        descending,
        relayout_hint,
        legend_hint,
        show_in_requests_hint,
        open_in_requests,
        endpoints_hint,
        show_method_in_requests_hint,
    }
    en = {
        map_view: "Map view",
        graph: "Graph",
        filter_hosts: "Filter hosts",
        fold_ids: "Fold IDs",
        relayout: "Relayout",
        edges: "Edges",
        legend: "Legend",
        legend_node_desc: "One domain (host)",
        legend_node_size: "Node size",
        legend_node_size_desc: "Request count (log scale)",
        legend_first_party_desc: "Solid border · same eTLD+1",
        legend_third_party_desc: "Dashed border · other eTLD+1",
        legend_referer_desc: "Page loads resource (caller → target)",
        legend_origin_desc: "Cross-origin request (Origin header)",
        legend_redirect_desc: "3xx response Location target",
        legend_active: "Active",
        legend_active_desc: "Hovered or Tree selection",
        legend_self_loop: "Self-loop",
        legend_self_loop_desc: "Same-domain relation",
        legend_drag: "Drag",
        legend_drag_desc: "Move node",
        legend_click: "Click",
        legend_click_desc: "Open host in Tree",
        export: "Export",
        save_tree_text: "Save tree as Text (.txt)",
        save_map_json: "Save map as JSON (.json)",
        save_graph_mermaid: "Save graph as Mermaid (.mmd)",
        save_graph_dot: "Save graph as Graphviz DOT (.dot)",
        copy_mermaid: "Copy Mermaid to clipboard",
        copy_dot: "Copy DOT to clipboard",
        no_relationships: "No domain relationships yet",
        referer: "Referer",
        origin_cors: "Origin (CORS)",
        redirect: "Redirect (3xx)",
        no_urls_yet: "No URLs captured yet",
        no_nodes_match: "No nodes match the filters",
        node: "Node",
        no_node_selected: "No node selected",
        show_in_requests: "Show in Requests",
        summary: "Summary",
        methods: "Methods",
        content_type: "Content-Type",
        transferred: "Transferred",
        response_time: "Response time",
        observed: "Observed",
        party: "Party",
        folded_ids: "Folded IDs",
        no_path_info: "No path info",
        merged: "Merged",
        query_names: "Query names",
        endpoints: "Endpoints",
        invalid_target: "(invalid target)",
        status_err_short: "err",
        status_tun_short: "tun",
        status_pend_short: "pend",
        endpoint_query_params: "Query parameters",
        endpoint_none_observed: "None observed.",
        endpoint_custom_headers: "Custom request headers (names only)",
        endpoint_auth: "Authentication (scheme names only)",
        endpoint_no_credentials: "No credentials observed.",
        endpoint_request_body: "Request body",
        endpoint_no_request_bodies: "No request bodies observed.",
        endpoint_responses: "Responses",
        endpoint_no_responses: "No responses observed.",
        endpoint_empty: "empty",
        endpoint_unstructured: "without field structure",
        endpoint_invalid: "unparseable",
        endpoint_file: "file",
        endpoint_field: "field",
        endpoint_part: "part",
        endpoint_header_item: "header",
        endpoint_query_item: "query parameter",
        endpoint_body_group_item: "body group",
        endpoint_depth_limited: "Deeper nesting is not described.",
        endpoint_optional_suffix: " · optional",
        endpoint_nullable_suffix: " · nullable",
        endpoint_mixed_suffix: " · mixed types",
    }
    en_tooltips = {
        clear_hint: "Clear captured requests and reset the URL map.",
        fold_ids_hint: "Group sibling ID segments (integers, UUIDs, hex tokens) into /{id}.",
        ascending: "Ascending",
        descending: "Descending",
        relayout_hint: "Recompute the force-directed domain layout.",
        legend_hint: "Show or hide the graph legend.",
        show_in_requests_hint: "Switch to Requests view filtered to this origin and path prefix.",
        open_in_requests: "Open this request in Requests view.",
        endpoints_hint: "Names, types, and frequencies from requests on this exact path; values are never shown.",
        show_method_in_requests_hint: "Switch to Requests filtered to this method and exact path.",
    }
    ko = {
        map_view: "맵 보기",
        graph: "그래프",
        filter_hosts: "호스트 필터",
        fold_ids: "ID 접기",
        relayout: "다시 배치",
        edges: "연결선",
        legend: "범례",
        legend_node_desc: "도메인(호스트) 하나",
        legend_node_size: "노드 크기",
        legend_node_size_desc: "요청 수 비례 (로그 비율)",
        legend_first_party_desc: "실선 테두리 · 같은 eTLD+1",
        legend_third_party_desc: "점선 테두리 · 다른 eTLD+1",
        legend_referer_desc: "페이지 리소스 호출 (참조 → 대상)",
        legend_origin_desc: "Origin 헤더 교차 출처 요청",
        legend_redirect_desc: "3xx 응답 Location 대상",
        legend_active: "강조",
        legend_active_desc: "호버·트리 선택 도메인과 선",
        legend_self_loop: "고리 선",
        legend_self_loop_desc: "같은 도메인 내 관계",
        legend_drag: "드래그",
        legend_drag_desc: "노드 이동",
        legend_click: "클릭",
        legend_click_desc: "트리에서 해당 호스트로 이동",
        export: "내보내기",
        save_tree_text: "트리를 텍스트(.txt)로 저장",
        save_map_json: "맵을 JSON(.json)으로 저장",
        save_graph_mermaid: "그래프를 Mermaid(.mmd)로 저장",
        save_graph_dot: "그래프를 Graphviz DOT(.dot)로 저장",
        copy_mermaid: "Mermaid를 클립보드에 복사",
        copy_dot: "DOT를 클립보드에 복사",
        no_relationships: "아직 도메인 관계가 없습니다",
        referer: "Referer",
        origin_cors: "Origin (CORS)",
        redirect: "리다이렉트 (3xx)",
        no_urls_yet: "캡처된 URL이 없습니다",
        no_nodes_match: "필터와 일치하는 노드가 없습니다",
        node: "노드",
        no_node_selected: "선택된 노드 없음",
        show_in_requests: "요청 목록에서 보기",
        summary: "요약",
        methods: "메서드",
        content_type: "Content-Type",
        transferred: "전송량",
        response_time: "응답 시간",
        observed: "관측 기간",
        party: "구분",
        folded_ids: "접힌 ID",
        no_path_info: "경로 정보 없음",
        merged: "병합됨",
        query_names: "쿼리 이름",
        endpoints: "엔드포인트",
        invalid_target: "(잘못된 대상)",
        status_err_short: "오류",
        status_tun_short: "터널",
        status_pend_short: "대기",
        endpoint_query_params: "쿼리 매개변수",
        endpoint_none_observed: "관측되지 않음.",
        endpoint_custom_headers: "사용자 지정 요청 헤더 (이름만)",
        endpoint_auth: "인증 (방식 이름만)",
        endpoint_no_credentials: "관측된 인증 정보 없음.",
        endpoint_request_body: "요청 본문",
        endpoint_no_request_bodies: "관측된 요청 본문이 없습니다.",
        endpoint_responses: "응답",
        endpoint_no_responses: "관측된 응답이 없습니다.",
        endpoint_empty: "비어 있음",
        endpoint_unstructured: "필드 구조 없음",
        endpoint_invalid: "파싱 불가",
        endpoint_file: "파일",
        endpoint_field: "필드",
        endpoint_part: "파트",
        endpoint_header_item: "헤더",
        endpoint_query_item: "쿼리 매개변수",
        endpoint_body_group_item: "본문 그룹",
        endpoint_depth_limited: "더 깊은 중첩 구조는 표시하지 않습니다.",
        endpoint_optional_suffix: " · 선택",
        endpoint_nullable_suffix: " · null 허용",
        endpoint_mixed_suffix: " · 혼합 형식",
    }
    ko_tooltips = {
        clear_hint: "캡처된 요청을 비우고 URL 맵을 초기화합니다.",
        fold_ids_hint: "정수·UUID·16진수 토큰 같은 ID 경로 조각을 /{id}로 묶습니다.",
        ascending: "오름차순",
        descending: "내림차순",
        relayout_hint: "도메인 관계 그래프 배치를 다시 계산합니다.",
        legend_hint: "그래프 범례 패널을 열거나 닫습니다.",
        show_in_requests_hint: "이 출처와 경로 접두사로 필터링된 요청 보기로 전환합니다.",
        open_in_requests: "이 요청을 요청 보기에서 엽니다.",
        endpoints_hint: "이 경로의 요청에서 추출한 이름·형식·빈도만 표시하며 실제 값은 보이지 않습니다.",
        show_method_in_requests_hint: "이 메서드와 정확한 경로로 필터링된 요청 목록으로 전환합니다.",
    }
}

/// Label of the URL map sort menu.
#[must_use]
pub(crate) fn sort_label(lang: Language, field: &str) -> String {
    match lang {
        Language::English => format!("Sort: {field}"),
        Language::Korean => format!("정렬: {field}"),
    }
}

/// A choice of the graph's domain limit.
#[must_use]
pub(crate) fn top_domains(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("Top {count} domains"),
        Language::Korean => format!("상위 도메인 {count}개"),
    }
}

/// Requests counted under a URL map node.
#[must_use]
pub(crate) fn request_total(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} req"),
        Language::Korean => format!("{count}건"),
    }
}

/// Error responses and failures counted under a URL map node.
#[must_use]
pub(crate) fn error_total(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} err"),
        Language::Korean => format!("오류 {count}"),
    }
}

/// Requests of a URL map node: the total, and the direct, retained, and
/// removed requests when they differ from it.
#[must_use]
pub(crate) fn node_requests(
    lang: Language,
    total: u64,
    direct: u64,
    retained: usize,
    evicted: u64,
) -> String {
    let mut parts = Vec::new();
    if direct > 0 && direct != total {
        parts.push(match lang {
            Language::English => format!("{direct} direct"),
            Language::Korean => format!("직접 {direct}건"),
        });
    }
    if evicted > 0 {
        parts.push(match lang {
            Language::English => format!("{retained} retained"),
            Language::Korean => format!("{retained}건 유지"),
        });
        parts.push(match lang {
            Language::English => format!("{evicted} removed"),
            Language::Korean => format!("오래된 기록 {evicted}건 정리됨"),
        });
    }
    if parts.is_empty() {
        total.to_string()
    } else {
        format!("{total} ({})", parts.join(", "))
    }
}

/// Average and maximum response time.
#[must_use]
pub(crate) fn avg_max(lang: Language, avg: f64, max: f64) -> String {
    match lang {
        Language::English => format!("avg {avg:.1} ms · max {max:.1} ms"),
        Language::Korean => format!("평균 {avg:.1} ms · 최대 {max:.1} ms"),
    }
}

/// Third-party requests under a node.
#[must_use]
pub(crate) fn third_party_hits(lang: Language, hits: u64) -> String {
    match lang {
        Language::English => format!("3rd-party · {hits} hits"),
        Language::Korean => format!("서드파티 · {hits}건"),
    }
}

/// Deeper requests merged into a node at the node limit.
#[must_use]
pub(crate) fn merged_at_cap(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} deeper requests at the node limit"),
        Language::Korean => format!("노드 한도로 병합된 하위 요청 {count}건"),
    }
}

/// The newest requests listed out of all retained ones.
#[must_use]
pub(crate) fn newest_of(lang: Language, shown: usize, total: usize) -> String {
    match lang {
        Language::English => format!("Newest {shown} of {total}"),
        Language::Korean => format!("최근 {shown}/{total}건"),
    }
}

/// Requests the endpoint analysis did not take this session.
#[must_use]
pub(crate) fn analysis_skipped(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} requests not analyzed"),
        Language::Korean => format!("분석 생략 요청 {count}건"),
    }
}

/// Tunnel target preview in the node detail pane.
#[must_use]
pub(crate) fn tunnel_target(lang: Language, origin: &str) -> String {
    match lang {
        Language::English => format!("{origin} (tunnel)"),
        Language::Korean => format!("{origin} (터널)"),
    }
}

/// Domain node summary passed into [`graph_node_tooltip`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct GraphNodeSummary<'a> {
    pub(crate) host: &'a str,
    pub(crate) site: &'a str,
    pub(crate) third_party: bool,
    pub(crate) request_count: u64,
    pub(crate) error_count: u64,
    pub(crate) total_bytes: u64,
    pub(crate) outgoing: usize,
    pub(crate) incoming: usize,
}

/// Multi-line summary tooltip for a domain node in the relationship graph.
#[must_use]
pub(crate) fn graph_node_tooltip(lang: Language, node: GraphNodeSummary<'_>) -> String {
    let party = if node.third_party {
        lang.shared().third_party
    } else {
        lang.shared().first_party
    };
    let bytes = ByteSize::new(node.total_bytes);
    match lang {
        Language::English => format!(
            "{}\nSite (eTLD+1): {}\nParty: {party}\nRequests: {} · Errors: {} · Size: {bytes}\nEdges: {} outgoing, {} incoming",
            node.host,
            node.site,
            node.request_count,
            node.error_count,
            node.outgoing,
            node.incoming
        ),
        Language::Korean => format!(
            "{}\n사이트 (eTLD+1): {}\n구분: {party}\n요청: {} · 오류: {} · 크기: {bytes}\n연결선: 발신 {}, 수신 {}",
            node.host,
            node.site,
            node.request_count,
            node.error_count,
            node.outgoing,
            node.incoming
        ),
    }
}

/// Collapsible header for one method under a URL map node's Endpoints section.
#[must_use]
pub(crate) fn endpoint_header(lang: Language, method: &str, requests: u64) -> String {
    match lang {
        Language::English => format!("{method} · {requests} request(s)"),
        Language::Korean => format!("{method} · 요청 {requests}건"),
    }
}

/// Button label to jump to Requests filtered by `method` on this exact path.
#[must_use]
pub(crate) fn show_method_in_requests(lang: Language, method: &str) -> String {
    match lang {
        Language::English => format!("Show {method} in Requests"),
        Language::Korean => format!("요청 목록에서 {method} 보기"),
    }
}

/// Evidence line summarizing sample counts for an endpoint schema.
#[must_use]
pub(crate) fn endpoint_evidence(
    lang: Language,
    requests: u64,
    structured_bodies: u64,
    excluded_bodies: u64,
    missing_responses: u64,
) -> String {
    let mut parts = match lang {
        Language::English => vec![
            format!("{requests} request(s)"),
            format!("{structured_bodies} body sample(s) described"),
        ],
        Language::Korean => vec![
            format!("요청 {requests}건"),
            format!("본문 샘플 {structured_bodies}건 분석"),
        ],
    };
    if excluded_bodies > 0 {
        parts.push(match lang {
            Language::English => format!("{excluded_bodies} excluded as truncated or unreadable"),
            Language::Korean => format!("{excluded_bodies}건 제외(잘림 또는 읽기 불가)"),
        });
    }
    if missing_responses > 0 {
        parts.push(match lang {
            Language::English => format!("{missing_responses} without response"),
            Language::Korean => format!("응답 없음 {missing_responses}건"),
        });
    }
    match lang {
        Language::English => format!("Evidence: {}", parts.join(" · ")),
        Language::Korean => format!("근거: {}", parts.join(" · ")),
    }
}

/// Path parameter summary line for a folded `{id}` segment.
#[must_use]
pub(crate) fn endpoint_path_param(lang: Language, segment_one_based: usize, kinds: &str) -> String {
    match lang {
        Language::English => {
            format!("Path parameter {{id}} (segment {segment_one_based}): {kinds}")
        }
        Language::Korean => format!("경로 매개변수 {{id}} (세그먼트 {segment_one_based}): {kinds}"),
    }
}

/// Collapsible request body group title.
#[must_use]
pub(crate) fn endpoint_request_body_title(
    lang: Language,
    content_type: &str,
    bodies: u64,
) -> String {
    match lang {
        Language::English => format!("{content_type} · {bodies} request(s)"),
        Language::Korean => format!("{content_type} · 요청 {bodies}건"),
    }
}

/// Note for requests on this endpoint that carried no request body.
#[must_use]
pub(crate) fn endpoint_requests_without_body(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} request(s) without a body"),
        Language::Korean => format!("본문 없는 요청 {count}건"),
    }
}

/// Collapsible response group title.
#[must_use]
pub(crate) fn endpoint_response_title(
    lang: Language,
    status: u16,
    content_type: &str,
    bodies: u64,
) -> String {
    match lang {
        Language::English => format!("{status} · {content_type} · {bodies} response(s)"),
        Language::Korean => format!("{status} · {content_type} · 응답 {bodies}건"),
    }
}

/// Root body type label when the root value is not purely an object.
#[must_use]
pub(crate) fn endpoint_body_type(lang: Language, summary: &str) -> String {
    match lang {
        Language::English => format!("Body type: {summary}"),
        Language::Korean => format!("본문 형식: {summary}"),
    }
}

/// Count of structured body samples described in a body group.
#[must_use]
pub(crate) fn endpoint_described_count(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} described"),
        Language::Korean => format!("분석 {count}건"),
    }
}

/// Count with a localized status label inside an endpoint body group summary.
#[must_use]
pub(crate) fn endpoint_labeled_count(lang: Language, count: u64, label: &str) -> String {
    match lang {
        Language::English => format!("{count} {label}"),
        Language::Korean => format!("{label} {count}건"),
    }
}

/// Count and reason breakdown of excluded body samples in a body group.
#[must_use]
pub(crate) fn endpoint_excluded_reasons(
    lang: Language,
    excluded: u64,
    reasons: &[String],
) -> String {
    match lang {
        Language::English => format!("{excluded} excluded ({})", reasons.join(", ")),
        Language::Korean => format!("제외 {excluded}건 ({})", reasons.join(", ")),
    }
}

/// Array item count suffix in an endpoint field tree row.
#[must_use]
pub(crate) fn endpoint_item_count(lang: Language, seen: u64) -> String {
    match lang {
        Language::English => format!("  {seen} item(s)"),
        Language::Korean => format!("  항목 {seen}개"),
    }
}

/// Omission note when an endpoint schema hits its per-section item limit.
#[must_use]
pub(crate) fn endpoint_omitted_note(lang: Language, omitted: u64, what: &str) -> String {
    match lang {
        Language::English => format!("{omitted} more {what}(s) not kept (limit reached)."),
        Language::Korean => format!("한도에 도달해 {what} {omitted}개가 더 보관되지 않았습니다."),
    }
}
