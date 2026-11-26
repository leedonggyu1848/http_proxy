//! Strings that the workbench shows: the shell, the inspector, the URL map,
//! and the status bar.
//!
//! The workbench keeps its labels in one catalog backed by [`Language`] so
//! adding a new language checks every shell label at compile time.

use crate::ui::i18n::Language;

/// A fixed label shown by the shell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShellText {
    /// Accessible name of the Requests and Map switch.
    ViewSwitch,
    Requests,
    Map,
    /// State word of a stopped proxy, beside its dot.
    Stopped,
    Port,
    Start,
    Stop,
    /// Heading of the copy items in the proxy menu.
    Copy,
    HttpsOn,
    HttpsOff,
    DecryptHttps,
    Save,
    Settings,
    CaptureScope,
    /// Hint and accessible name of the request filter input.
    Filter,
    /// Short error at the right end of the filter input.
    InvalidFilter,
    /// Short error for a malformed field condition.
    InvalidCondition,
    /// Accessible name of the outcome tabs above the request list.
    RequestOutcome,
    Searching,
    // Request filter.
    /// Hint of the request filter in regex mode.
    FilterRegex,
    ClearFilter,
    /// Accessible name of the request filter input.
    FilterRequests,
    FilterSyntax,
    UseRegex,
    MatchCase,
    SearchScope,
    /// Row of the filter syntax popup.
    Exclude,
    Phrase,
    FilterByValue,
    ExcludeValue,
    FieldFilterNeedsText,
    /// Tooltip of the host shown in the Path column header.
    SharedHost,
    // Inspector.
    Headers,
    Body,
    Timing,
    NoRequestSelected,
    EditAndResend,
    /// Group of decoded query parameters in the Headers tab.
    Query,
    /// Group of request headers.
    Request,
    /// Group of response headers.
    Response,
    /// Line under headers when the capture kept only some of them.
    HeadersOmitted,
    /// Group of a CONNECT tunnel's details.
    Tunnel,
    /// Tooltip of the Tunnel group.
    TunnelHint,
    TunnelState,
    TunnelSent,
    TunnelReceived,
    Duration,
    Connect,
    Send,
    Wait,
    Receive,
    Total,
    /// A value the proxy did not observe.
    NotObserved,
    RequestBody,
    ResponseBody,
    /// A body preview that is being replaced.
    Updating,
    PreviewOmitted,
    PreviewOmittedHint,
    /// Tooltip of the disabled Tree view mode.
    TreeUnavailable,
    Previous,
    Next,
    NoMatches,
    ExpandAll,
    CollapseAll,
    /// Hint of the body tree search input.
    FilterKeys,
    CloseBrowser,
    /// State beside Close browser once the browser window is up.
    BrowserOpen,
    // Status bar.
    SearchOnUiThread,
    UrlMapLimit,
    FullLogUnavailable,
    LogBodyLimit,
    /// Tooltip of the graph count when the domain limit hides domains.
    DomainLimitHint,
    // URL map.
    Clear,
    /// Tooltip of the Map Clear button.
    ClearHint,
    /// Accessible name of the Tree and Graph tabs.
    MapView,
    Tree,
    Graph,
    /// Hint and accessible name of the Map host filter.
    FilterHosts,
    FoldIds,
    FoldIdsHint,
    Ascending,
    Descending,
    Relayout,
    RelayoutHint,
    /// The graph menu of edge kinds and node outlines.
    Legend,
    Export,
    SaveTreeText,
    SaveMapJson,
    SaveGraphMermaid,
    SaveGraphDot,
    CopyMermaid,
    CopyDot,
    NoRelationships,
    Referer,
    OriginCors,
    Redirect,
    FirstParty,
    ThirdParty,
    NoUrlsYet,
    NoNodesMatch,
    /// Name of the Map detail pane before a node is selected.
    Node,
    NoNodeSelected,
    ShowInRequests,
    ShowInRequestsHint,
    Summary,
    Methods,
    Status,
    ContentType,
    Transferred,
    ResponseTime,
    Observed,
    Party,
    FoldedIds,
    NoPathInfo,
    Merged,
    Failed,
    Pending,
    QueryNames,
    OpenInRequests,
    Endpoints,
    EndpointsHint,
}

impl ShellText {
    #[expect(
        clippy::too_many_lines,
        reason = "the catalog is one match arm per string"
    )]
    pub(crate) const fn text(self, lang: Language) -> &'static str {
        match self {
            Self::ViewSwitch => lang.pick("View", "보기"),
            Self::Requests => lang.pick("Requests", "요청"),
            Self::Map => lang.pick("Map", "맵"),
            Self::Stopped => lang.pick("Stopped", "중지됨"),
            Self::Port => lang.pick("Port", "포트"),
            Self::Start => lang.pick("Start", "시작"),
            Self::Stop => lang.pick("Stop", "중지"),
            Self::Copy => lang.pick("Copy", "복사"),
            Self::HttpsOn => lang.pick("HTTPS on", "HTTPS 켬"),
            Self::HttpsOff => lang.pick("HTTPS off", "HTTPS 끔"),
            Self::DecryptHttps => lang.pick("Decrypt HTTPS", "HTTPS 복호화"),
            Self::Save => lang.pick("Save", "저장"),
            Self::Settings => lang.pick("Settings", "설정"),
            Self::CaptureScope => lang.pick("Capture scope", "캡처 범위"),
            Self::Filter => lang.pick(
                "method:POST url:/api status:4xx",
                "method:POST url:/api status:4xx",
            ),
            Self::InvalidFilter => lang.pick("Invalid pattern", "잘못된 패턴"),
            Self::InvalidCondition => lang.pick("Invalid condition", "잘못된 조건"),
            Self::RequestOutcome => lang.pick("Request outcome", "요청 결과"),
            Self::Searching => lang.pick("Searching…", "검색 중…"),
            Self::FilterRegex => lang.pick("Filter with regex", "정규식으로 필터링"),
            Self::ClearFilter => lang.pick("Clear filter", "필터 비우기"),
            Self::FilterRequests => lang.pick("Filter requests", "요청 필터"),
            Self::FilterSyntax => lang.pick("Filter syntax", "필터 문법"),
            Self::UseRegex => lang.pick("Use a regular expression", "정규식 사용"),
            Self::MatchCase => lang.pick("Match free-text case", "자유 텍스트 대소문자 구분"),
            Self::SearchScope => lang.pick("Search scope for free text", "자유 텍스트 검색 범위"),
            Self::Exclude => lang.pick("Exclude", "제외"),
            Self::Phrase => lang.pick("Phrase", "구문"),
            Self::FilterByValue => lang.pick("Filter by value", "이 값으로 필터링"),
            Self::ExcludeValue => lang.pick("Exclude value", "이 값 제외"),
            Self::FieldFilterNeedsText => lang.pick(
                "Turn off regex mode to use field filters",
                "필드 필터를 쓰려면 정규식 모드를 끄세요",
            ),
            Self::SharedHost => lang.pick(
                "Every request in the list goes to this host",
                "목록의 모든 요청이 이 호스트로 전송되었습니다",
            ),
            Self::Headers => lang.pick("Headers", "헤더"),
            Self::Body => lang.pick("Body", "본문"),
            Self::Timing => lang.pick("Timing", "타이밍"),
            Self::NoRequestSelected => lang.pick("No request selected", "선택된 요청 없음"),
            Self::EditAndResend => lang.pick("Edit and Resend", "편집 후 재전송"),
            Self::Query => lang.pick("Query", "쿼리"),
            Self::Request => lang.pick("Request", "요청"),
            Self::Response => lang.pick("Response", "응답"),
            Self::HeadersOmitted => lang.pick("More headers omitted", "추가 헤더 생략됨"),
            Self::Tunnel => lang.pick("Tunnel", "터널"),
            Self::TunnelHint => lang.pick(
                "Forwarded without decryption, so only the host, port, bytes, and duration are recorded.",
                "복호화 없이 전달되어 호스트·포트·전송량·소요 시간만 기록됩니다.",
            ),
            Self::TunnelState => lang.pick("State", "상태"),
            Self::TunnelSent => lang.pick("Sent", "보냄"),
            Self::TunnelReceived => lang.pick("Received", "받음"),
            Self::Duration => lang.pick("Duration", "소요 시간"),
            Self::Connect => lang.pick("Connect", "연결"),
            Self::Send => lang.pick("Send", "전송"),
            Self::Wait => lang.pick("Wait (TTFB)", "대기 (TTFB)"),
            Self::Receive => lang.pick("Receive", "수신"),
            Self::Total => lang.pick("Total", "전체"),
            Self::NotObserved => "—",
            Self::RequestBody => lang.pick("Request body", "요청 본문"),
            Self::ResponseBody => lang.pick("Response body", "응답 본문"),
            Self::Updating => lang.pick("Updating…", "업데이트 중…"),
            Self::PreviewOmitted => lang.pick("Preview omitted", "미리보기 생략됨"),
            Self::PreviewOmittedHint => lang.pick(
                "The body was forwarded without keeping a preview, so forwarding never waited.",
                "전달이 지연되지 않도록 미리보기를 보관하지 않고 본문을 전달했습니다.",
            ),
            Self::TreeUnavailable => {
                lang.pick("Needs a JSON or XML body", "JSON 또는 XML 본문이 필요합니다")
            }
            Self::Previous => lang.pick("Previous", "이전"),
            Self::Next => lang.pick("Next", "다음"),
            Self::NoMatches => lang.pick("No matches", "일치 없음"),
            Self::ExpandAll => lang.pick("Expand all", "모두 펼치기"),
            Self::CollapseAll => lang.pick("Collapse all", "모두 접기"),
            Self::FilterKeys => lang.pick("Filter keys", "키 필터"),
            Self::CloseBrowser => lang.pick("Close browser", "브라우저 닫기"),
            Self::BrowserOpen => lang.pick("Browser open", "브라우저 열림"),
            Self::SearchOnUiThread => lang.pick("Search on UI thread", "UI 스레드 검색"),
            Self::UrlMapLimit => lang.pick("URL map limit", "URL 맵 한도"),
            Self::FullLogUnavailable => lang.pick("Full log unavailable", "전체 로그 사용 불가"),
            Self::LogBodyLimit => lang.pick("Log body limit", "로그 본문 한도"),
            Self::DomainLimitHint => lang.pick(
                "Raise the domain limit in the graph header to show more.",
                "더 많은 도메인을 보려면 그래프 머리줄에서 도메인 한도를 높이세요.",
            ),
            Self::Clear => lang.pick("Clear", "비우기"),
            Self::ClearHint => lang.pick(
                "Clear captured requests and reset the URL map.",
                "캡처된 요청을 비우고 URL 맵을 초기화합니다.",
            ),
            Self::MapView => lang.pick("Map view", "맵 보기"),
            Self::Tree => lang.pick("Tree", "트리"),
            Self::Graph => lang.pick("Graph", "그래프"),
            Self::FilterHosts => lang.pick("Filter hosts", "호스트 필터"),
            Self::FoldIds => lang.pick("Fold IDs", "ID 접기"),
            Self::FoldIdsHint => lang.pick(
                "Group sibling ID segments (integers, UUIDs, hex tokens) into /{id}.",
                "정수·UUID·16진수 토큰 같은 ID 경로 조각을 /{id}로 묶습니다.",
            ),
            Self::Ascending => lang.pick("Ascending", "오름차순"),
            Self::Descending => lang.pick("Descending", "내림차순"),
            Self::Relayout => lang.pick("Relayout", "다시 배치"),
            Self::RelayoutHint => lang.pick(
                "Recompute the force-directed domain layout.",
                "도메인 관계 그래프 배치를 다시 계산합니다.",
            ),
            Self::Legend => lang.pick("Edges", "연결선"),
            Self::Export => lang.pick("Export", "내보내기"),
            Self::SaveTreeText => lang.pick("Save tree as Text (.txt)", "트리를 텍스트(.txt)로 저장"),
            Self::SaveMapJson => lang.pick("Save map as JSON (.json)", "맵을 JSON(.json)으로 저장"),
            Self::SaveGraphMermaid => {
                lang.pick("Save graph as Mermaid (.mmd)", "그래프를 Mermaid(.mmd)로 저장")
            }
            Self::SaveGraphDot => lang.pick(
                "Save graph as Graphviz DOT (.dot)",
                "그래프를 Graphviz DOT(.dot)로 저장",
            ),
            Self::CopyMermaid => lang.pick("Copy Mermaid to clipboard", "Mermaid를 클립보드에 복사"),
            Self::CopyDot => lang.pick("Copy DOT to clipboard", "DOT를 클립보드에 복사"),
            Self::NoRelationships => {
                lang.pick("No domain relationships yet", "아직 도메인 관계가 없습니다")
            }
            Self::Referer => "Referer",
            Self::OriginCors => "Origin (CORS)",
            Self::Redirect => lang.pick("Redirect (3xx)", "리다이렉트 (3xx)"),
            Self::FirstParty => lang.pick("1st-party", "퍼스트파티"),
            Self::ThirdParty => lang.pick("3rd-party", "서드파티"),
            Self::NoUrlsYet => lang.pick("No URLs captured yet", "캡처된 URL이 없습니다"),
            Self::NoNodesMatch => {
                lang.pick("No nodes match the filters", "필터와 일치하는 노드가 없습니다")
            }
            Self::Node => lang.pick("Node", "노드"),
            Self::NoNodeSelected => lang.pick("No node selected", "선택된 노드 없음"),
            Self::ShowInRequests => lang.pick("Show in Requests", "요청 목록에서 보기"),
            Self::ShowInRequestsHint => lang.pick(
                "Switch to Requests view filtered to this origin and path prefix.",
                "이 출처와 경로 접두사로 필터링된 요청 보기로 전환합니다.",
            ),
            Self::Summary => lang.pick("Summary", "요약"),
            Self::Methods => lang.pick("Methods", "메서드"),
            Self::Status => lang.pick("Status", "상태"),
            Self::ContentType => "Content-Type",
            Self::Transferred => lang.pick("Transferred", "전송량"),
            Self::ResponseTime => lang.pick("Response time", "응답 시간"),
            Self::Observed => lang.pick("Observed", "관측 기간"),
            Self::Party => lang.pick("Party", "구분"),
            Self::FoldedIds => lang.pick("Folded IDs", "접힌 ID"),
            Self::NoPathInfo => lang.pick("No path info", "경로 정보 없음"),
            Self::Merged => lang.pick("Merged", "병합됨"),
            Self::Failed => lang.pick("Failed", "실패"),
            Self::Pending => lang.pick("Pending", "대기 중"),
            Self::QueryNames => lang.pick("Query names", "쿼리 이름"),
            Self::OpenInRequests => lang.pick(
                "Open this request in Requests view.",
                "이 요청을 요청 보기에서 엽니다.",
            ),
            Self::Endpoints => lang.pick("Endpoints", "엔드포인트"),
            Self::EndpointsHint => lang.pick(
                "Names, types, and frequencies from requests on this exact path; values are never shown.",
                "이 경로의 요청에서 추출한 이름·형식·빈도만 표시하며 실제 값은 보이지 않습니다.",
            ),
        }
    }
}

/// Request count in the status bar, or how many of them the filters show.
pub(crate) fn request_count(lang: Language, shown: usize, total: usize) -> String {
    match lang {
        Language::English => {
            if shown == total {
                format!("{total} requests")
            } else {
                format!("{shown} of {total} requests")
            }
        }
        Language::Korean => {
            if shown == total {
                format!("요청 {total}건")
            } else {
                format!("요청 {shown}/{total}건")
            }
        }
    }
}

/// Failed request count in the status bar.
pub(crate) fn failure_count(lang: Language, failed: usize) -> String {
    match lang {
        Language::English => format!("{failed} failed"),
        Language::Korean => format!("실패 {failed}건"),
    }
}

/// Host and node counts of the URL map in the status bar.
pub(crate) fn map_count(lang: Language, hosts: usize, nodes: usize) -> String {
    match lang {
        Language::English => format!("{hosts} hosts · {nodes} nodes"),
        Language::Korean => format!("호스트 {hosts} · 노드 {nodes}"),
    }
}

/// Search progress in the status bar.
pub(crate) fn search_progress(lang: Language, checked: usize, total: usize) -> String {
    format!("{} {checked}/{total}", ShellText::Searching.text(lang))
}

/// A time in milliseconds with one decimal.
pub(crate) fn milliseconds(millis: f64) -> String {
    format!("{millis:.1} ms")
}

/// A time that is still growing, in whole milliseconds.
pub(crate) fn milliseconds_running(millis: f64) -> String {
    format!("{millis:.0} ms…")
}

/// A byte count in the units of the request list.
pub(crate) fn byte_count(bytes: u64) -> String {
    http_inspector_exchange::ByteSize::new(bytes).to_string()
}

/// Link from a resent request to the one it was sent from.
pub(crate) fn replay_of(lang: Language, original: u64) -> String {
    match lang {
        Language::English => format!("Replay of #{original} — show original"),
        Language::Korean => format!("#{original}의 재전송 — 원본 보기"),
    }
}

/// Tooltip of the request filter: its keyboard shortcut.
pub(crate) fn focus_filter(lang: Language, shortcut: &str) -> String {
    match lang {
        Language::English => format!("Focus filter ({shortcut})"),
        Language::Korean => format!("필터에 포커스 ({shortcut})"),
    }
}

/// A response as the server sent it: status code, reason phrase as sent,
/// and HTTP version, such as `502 Bad Gateway · HTTP/1.1`. A response
/// without a reason phrase shows the code and version only.
pub(crate) fn response_line(code: u16, reason: &str, version: &str) -> String {
    if reason.is_empty() {
        format!("{code} · {version}")
    } else {
        format!("{code} {reason} · {version}")
    }
}

/// Body notice: the preview kept only the first `kib` KiB.
pub(crate) fn body_truncated(lang: Language, kib: usize) -> String {
    match lang {
        Language::English => format!("Showing the first {kib} KiB captured"),
        Language::Korean => format!("캡처된 처음 {kib} KiB 표시"),
    }
}

/// Body notice: the body is still arriving.
pub(crate) const fn body_streaming(lang: Language) -> &'static str {
    lang.pick(
        "Still receiving; showing what arrived",
        "수신 중이며 지금까지 도착한 내용 표시",
    )
}

/// Body notice: the body stopped before its end.
pub(crate) const fn body_ended(lang: Language) -> &'static str {
    lang.pick(
        "Ended early; showing what arrived",
        "중간에 종료되어 지금까지 도착한 내용 표시",
    )
}

/// Tooltip of the dropped-records entry of the status bar.
pub(crate) const fn dropped_hint(lang: Language) -> &'static str {
    lang.pick(
        "The capture queue was full, so these exchanges were forwarded but not shown.",
        "캡처 대기열이 가득 차 해당 요청은 전달되었지만 화면에는 표시되지 않았습니다.",
    )
}

/// Tooltip of the removed-records entry of the status bar.
pub(crate) fn older_removed_hint(lang: Language, kept: usize) -> String {
    match lang {
        Language::English => format!(
            "Only the newest {kept} requests stay on screen; the full-session log keeps every record."
        ),
        Language::Korean => {
            format!("화면에는 최근 {kept}건만 유지되며 전체 세션 로그에는 모든 기록이 남습니다.")
        }
    }
}

/// Tooltip of the URL map limit entry of the status bar.
pub(crate) fn url_map_limit_hint(lang: Language, nodes: usize) -> String {
    match lang {
        Language::English => {
            format!("The URL map reached {nodes} nodes; deeper paths are folded into parent nodes.")
        }
        Language::Korean => {
            format!("URL 맵이 {nodes}개 노드 한도에 도달해 하위 경로는 상위 노드에 병합됩니다.")
        }
    }
}

/// Tooltip of the log body limit entry of the status bar.
pub(crate) fn log_body_limit_hint(lang: Language, limit: &str) -> String {
    match lang {
        Language::English => format!(
            "Body bytes after {limit} are left out of the full-session log; headers and results are still logged."
        ),
        Language::Korean => format!(
            "{limit}를 넘는 본문 바이트는 전체 세션 로그에서 생략되며 헤더와 결과는 계속 기록됩니다."
        ),
    }
}

/// Bodies the search could read only in part.
pub(crate) fn partly_searched(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("{count} bodies partly searched"),
        Language::Korean => format!("본문 {count}건 일부만 검색됨"),
    }
}

/// Exchanges the capture queue could not keep.
pub(crate) fn dropped(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} dropped"),
        Language::Korean => format!("누락 {count}건"),
    }
}

/// Status bar entries left out for lack of room.
pub(crate) fn more_entries(count: usize) -> String {
    format!("+{count}")
}

/// A menu button label: the current choice and the menu arrow.
pub(crate) fn menu_label(label: &str) -> String {
    format!("{label} ▼")
}

/// Label of the URL map sort menu.
pub(crate) fn sort_label(lang: Language, field: &str) -> String {
    match lang {
        Language::English => format!("Sort: {field}"),
        Language::Korean => format!("정렬: {field}"),
    }
}

/// A choice of the graph's domain limit.
pub(crate) fn top_domains(lang: Language, count: usize) -> String {
    match lang {
        Language::English => format!("Top {count} domains"),
        Language::Korean => format!("상위 도메인 {count}개"),
    }
}

/// Domains and edges the graph shows, in the status bar.
pub(crate) fn graph_count(
    lang: Language,
    shown: usize,
    total: usize,
    visible_edges: usize,
    total_edges: usize,
) -> String {
    match lang {
        Language::English => {
            let domains = if total > shown {
                format!("top {shown} of {total} domains")
            } else {
                format!("{shown} domains")
            };
            let edges = if visible_edges == total_edges {
                format!("{total_edges} edges")
            } else {
                format!("{visible_edges}/{total_edges} edges")
            };
            format!("{domains} · {edges}")
        }
        Language::Korean => {
            let domains = if total > shown {
                format!("상위 도메인 {shown}/{total}")
            } else {
                format!("도메인 {shown}")
            };
            let edges = if visible_edges == total_edges {
                format!("연결선 {total_edges}")
            } else {
                format!("연결선 {visible_edges}/{total_edges}")
            };
            format!("{domains} · {edges}")
        }
    }
}

/// A folded ID node of the URL map tree.
pub(crate) fn folded_ids(label: &str, count: usize) -> String {
    format!("{label} ({count} IDs)")
}

/// Requests counted under a URL map node.
pub(crate) fn request_total(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} req"),
        Language::Korean => format!("{count}건"),
    }
}

/// Error responses and failures counted under a URL map node.
pub(crate) fn error_total(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} err"),
        Language::Korean => format!("오류 {count}"),
    }
}

/// Requests of a URL map node: the total, and the direct, retained, and
/// removed requests when they differ from it.
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
pub(crate) fn avg_max(lang: Language, avg: f64, max: f64) -> String {
    match lang {
        Language::English => format!("avg {avg:.1} ms · max {max:.1} ms"),
        Language::Korean => format!("평균 {avg:.1} ms · 최대 {max:.1} ms"),
    }
}

/// Third-party requests under a node.
pub(crate) fn third_party_hits(lang: Language, hits: u64) -> String {
    match lang {
        Language::English => format!("3rd-party · {hits} hits"),
        Language::Korean => format!("서드파티 · {hits}건"),
    }
}

/// IDs folded into one node and their kinds.
pub(crate) fn folded_kinds(count: usize, kinds: &[&str]) -> String {
    if kinds.is_empty() {
        count.to_string()
    } else {
        format!("{count} · {}", kinds.join(", "))
    }
}

/// Deeper requests merged into a node at the node limit.
pub(crate) fn merged_at_cap(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} deeper requests at the node limit"),
        Language::Korean => format!("노드 한도로 병합된 하위 요청 {count}건"),
    }
}

/// How often a name was seen.
pub(crate) fn times(count: u64) -> String {
    format!("{count}×")
}

/// Requests removed from the list but still in the counts.
pub(crate) fn older_removed(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} older removed"),
        Language::Korean => format!("오래된 기록 {count}건 정리됨"),
    }
}

/// The newest requests listed out of all retained ones.
pub(crate) fn newest_of(lang: Language, shown: usize, total: usize) -> String {
    match lang {
        Language::English => format!("Newest {shown} of {total}"),
        Language::Korean => format!("최근 {shown}/{total}건"),
    }
}

/// Requests the endpoint analysis did not take this session.
pub(crate) fn analysis_skipped(lang: Language, count: u64) -> String {
    match lang {
        Language::English => format!("{count} requests not analyzed"),
        Language::Korean => format!("분석 생략 요청 {count}건"),
    }
}

/// Position of the current search match in a body.
pub(crate) fn match_position(current: usize, total: usize) -> String {
    format!("{current}/{total}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_name_only_what_differs() {
        assert_eq!(request_count(Language::English, 12, 12), "12 requests");
        assert_eq!(request_count(Language::English, 3, 12), "3 of 12 requests");
        assert_eq!(request_count(Language::Korean, 12, 12), "요청 12건");
        assert_eq!(request_count(Language::Korean, 3, 12), "요청 3/12건");
        assert_eq!(failure_count(Language::English, 2), "2 failed");
        assert_eq!(failure_count(Language::Korean, 2), "실패 2건");
        assert_eq!(map_count(Language::English, 3, 17), "3 hosts · 17 nodes");
        assert_eq!(map_count(Language::Korean, 3, 17), "호스트 3 · 노드 17");
        assert_eq!(search_progress(Language::English, 4, 9), "Searching… 4/9");
        assert_eq!(search_progress(Language::Korean, 4, 9), "검색 중… 4/9");
        assert_eq!(match_position(1, 3), "1/3");
        assert_eq!(
            response_line(502, "Bad Gateway", "HTTP/1.1"),
            "502 Bad Gateway · HTTP/1.1"
        );
        assert_eq!(response_line(200, "", "HTTP/2.0"), "200 · HTTP/2.0");
        assert_eq!(milliseconds(52.64), "52.6 ms");
        assert_eq!(milliseconds_running(1204.4), "1204 ms…");
    }
}
