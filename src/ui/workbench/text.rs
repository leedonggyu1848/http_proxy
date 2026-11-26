//! Strings that the workbench shows: the shell, the inspector, the URL map,
//! and the status bar.
//!
//! The workbench keeps its labels in one catalog so a translation layer can
//! replace [`ShellText::text`] and the formatters without touching the
//! layout code.

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
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::ViewSwitch => "View",
            Self::Requests => "Requests",
            Self::Map => "Map",
            Self::Stopped => "Stopped",
            Self::Port => "Port",
            Self::Start => "Start",
            Self::Stop => "Stop",
            Self::Copy => "Copy",
            Self::HttpsOn => "HTTPS on",
            Self::HttpsOff => "HTTPS off",
            Self::DecryptHttps => "Decrypt HTTPS",
            Self::Save => "Save",
            Self::Settings => "Settings",
            Self::CaptureScope => "Capture scope",
            Self::Filter => "method:POST url:/api status:4xx",
            Self::InvalidFilter => "Invalid pattern",
            Self::InvalidCondition => "Invalid condition",
            Self::RequestOutcome => "Request outcome",
            Self::Searching => "Searching…",
            Self::FilterRegex => "Filter with regex",
            Self::ClearFilter => "Clear filter",
            Self::FilterRequests => "Filter requests",
            Self::FilterSyntax => "Filter syntax",
            Self::UseRegex => "Use a regular expression",
            Self::MatchCase => "Match free-text case",
            Self::SearchScope => "Search scope for free text",
            Self::Exclude => "Exclude",
            Self::Phrase => "Phrase",
            Self::FilterByValue => "Filter by value",
            Self::ExcludeValue => "Exclude value",
            Self::FieldFilterNeedsText => "Turn off regex mode to use field filters",
            Self::SharedHost => "Every request in the list goes to this host",
            Self::Headers => "Headers",
            Self::Body => "Body",
            Self::Timing => "Timing",
            Self::NoRequestSelected => "No request selected",
            Self::EditAndResend => "Edit and Resend",
            Self::Query => "Query",
            Self::Request => "Request",
            Self::Response => "Response",
            Self::HeadersOmitted => "More headers omitted",
            Self::Tunnel => "Tunnel",
            Self::TunnelHint => {
                "Forwarded without decryption, so only the host, port, bytes, and duration are recorded."
            }
            Self::TunnelState => "State",
            Self::TunnelSent => "Sent",
            Self::TunnelReceived => "Received",
            Self::Duration => "Duration",
            Self::Connect => "Connect",
            Self::Send => "Send",
            Self::Wait => "Wait (TTFB)",
            Self::Receive => "Receive",
            Self::Total => "Total",
            Self::NotObserved => "—",
            Self::RequestBody => "Request body",
            Self::ResponseBody => "Response body",
            Self::Updating => "Updating…",
            Self::PreviewOmitted => "Preview omitted",
            Self::PreviewOmittedHint => {
                "The body was forwarded without keeping a preview, so forwarding never waited."
            }
            Self::TreeUnavailable => "Needs a JSON or XML body",
            Self::Previous => "Previous",
            Self::Next => "Next",
            Self::NoMatches => "No matches",
            Self::ExpandAll => "Expand all",
            Self::CollapseAll => "Collapse all",
            Self::FilterKeys => "Filter keys",
            Self::CloseBrowser => "Close browser",
            Self::BrowserOpen => "Browser open",
            Self::SearchOnUiThread => "Search on UI thread",
            Self::UrlMapLimit => "URL map limit",
            Self::FullLogUnavailable => "Full log unavailable",
            Self::LogBodyLimit => "Log body limit",
            Self::DomainLimitHint => "Raise the domain limit in the graph header to show more.",
            Self::Clear => "Clear",
            Self::ClearHint => "Clear captured requests and reset the URL map.",
            Self::MapView => "Map view",
            Self::Tree => "Tree",
            Self::Graph => "Graph",
            Self::FilterHosts => "Filter hosts",
            Self::FoldIds => "Fold IDs",
            Self::FoldIdsHint => {
                "Group sibling ID segments (integers, UUIDs, hex tokens) into /{id}."
            }
            Self::Ascending => "Ascending",
            Self::Descending => "Descending",
            Self::Relayout => "Relayout",
            Self::RelayoutHint => "Recompute the force-directed domain layout.",
            Self::Legend => "Edges",
            Self::Export => "Export",
            Self::SaveTreeText => "Save tree as Text (.txt)",
            Self::SaveMapJson => "Save map as JSON (.json)",
            Self::SaveGraphMermaid => "Save graph as Mermaid (.mmd)",
            Self::SaveGraphDot => "Save graph as Graphviz DOT (.dot)",
            Self::CopyMermaid => "Copy Mermaid to clipboard",
            Self::CopyDot => "Copy DOT to clipboard",
            Self::NoRelationships => "No domain relationships yet",
            Self::Referer => "Referer",
            Self::OriginCors => "Origin (CORS)",
            Self::Redirect => "Redirect (3xx)",
            Self::FirstParty => "1st-party",
            Self::ThirdParty => "3rd-party",
            Self::NoUrlsYet => "No URLs captured yet",
            Self::NoNodesMatch => "No nodes match the filters",
            Self::Node => "Node",
            Self::NoNodeSelected => "No node selected",
            Self::ShowInRequests => "Show in Requests",
            Self::ShowInRequestsHint => {
                "Switch to Requests view filtered to this origin and path prefix."
            }
            Self::Summary => "Summary",
            Self::Methods => "Methods",
            Self::Status => "Status",
            Self::ContentType => "Content-Type",
            Self::Transferred => "Transferred",
            Self::ResponseTime => "Response time",
            Self::Observed => "Observed",
            Self::Party => "Party",
            Self::FoldedIds => "Folded IDs",
            Self::NoPathInfo => "No path info",
            Self::Merged => "Merged",
            Self::Failed => "Failed",
            Self::Pending => "Pending",
            Self::QueryNames => "Query names",
            Self::OpenInRequests => "Open this request in Requests view.",
            Self::Endpoints => "Endpoints",
            Self::EndpointsHint => {
                "Names, types, and frequencies from requests on this exact path; values are never shown."
            }
        }
    }
}

/// Request count in the status bar, or how many of them the filters show.
pub(crate) fn request_count(shown: usize, total: usize) -> String {
    if shown == total {
        format!("{total} requests")
    } else {
        format!("{shown} of {total} requests")
    }
}

/// Failed request count in the status bar.
pub(crate) fn failure_count(failed: usize) -> String {
    format!("{failed} failed")
}

/// Host and node counts of the URL map in the status bar.
pub(crate) fn map_count(hosts: usize, nodes: usize) -> String {
    format!("{hosts} hosts · {nodes} nodes")
}

/// Search progress in the status bar.
pub(crate) fn search_progress(checked: usize, total: usize) -> String {
    format!("{} {checked}/{total}", ShellText::Searching.text())
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
pub(crate) fn replay_of(original: u64) -> String {
    format!("Replay of #{original} — show original")
}

/// Tooltip of the request filter: its keyboard shortcut.
pub(crate) fn focus_filter(shortcut: &str) -> String {
    format!("Focus filter ({shortcut})")
}

/// Bodies the search could read only in part.
pub(crate) fn partly_searched(count: usize) -> String {
    format!("{count} bodies partly searched")
}

/// Exchanges the capture queue could not keep.
pub(crate) fn dropped(count: u64) -> String {
    format!("{count} dropped")
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
pub(crate) fn sort_label(field: &str) -> String {
    format!("Sort: {field}")
}

/// A choice of the graph's domain limit.
pub(crate) fn top_domains(count: usize) -> String {
    format!("Top {count} domains")
}

/// Domains and edges the graph shows, in the status bar.
pub(crate) fn graph_count(
    shown: usize,
    total: usize,
    visible_edges: usize,
    total_edges: usize,
) -> String {
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

/// A folded ID node of the URL map tree.
pub(crate) fn folded_ids(label: &str, count: usize) -> String {
    format!("{label} ({count} IDs)")
}

/// Requests counted under a URL map node.
pub(crate) fn request_total(count: u64) -> String {
    format!("{count} req")
}

/// Error responses and failures counted under a URL map node.
pub(crate) fn error_total(count: u64) -> String {
    format!("{count} err")
}

/// Requests of a URL map node: the total, and the direct, retained, and
/// removed requests when they differ from it.
pub(crate) fn node_requests(total: u64, direct: u64, retained: usize, evicted: u64) -> String {
    let mut parts = Vec::new();
    if direct > 0 && direct != total {
        parts.push(format!("{direct} direct"));
    }
    if evicted > 0 {
        parts.push(format!("{retained} retained"));
        parts.push(format!("{evicted} removed"));
    }
    if parts.is_empty() {
        total.to_string()
    } else {
        format!("{total} ({})", parts.join(", "))
    }
}

/// Average and maximum response time.
pub(crate) fn avg_max(avg: f64, max: f64) -> String {
    format!("avg {avg:.1} ms · max {max:.1} ms")
}

/// Third-party requests under a node.
pub(crate) fn third_party_hits(hits: u64) -> String {
    format!("3rd-party · {hits} hits")
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
pub(crate) fn merged_at_cap(count: u64) -> String {
    format!("{count} deeper requests at the node limit")
}

/// How often a name was seen.
pub(crate) fn times(count: u64) -> String {
    format!("{count}×")
}

/// Requests removed from the list but still in the counts.
pub(crate) fn older_removed(count: u64) -> String {
    format!("{count} older removed")
}

/// The newest requests listed out of all retained ones.
pub(crate) fn newest_of(shown: usize, total: usize) -> String {
    format!("Newest {shown} of {total}")
}

/// Requests the endpoint analysis did not take this session.
pub(crate) fn analysis_skipped(count: u64) -> String {
    format!("{count} requests not analyzed")
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
        assert_eq!(request_count(12, 12), "12 requests");
        assert_eq!(request_count(3, 12), "3 of 12 requests");
        assert_eq!(failure_count(2), "2 failed");
        assert_eq!(map_count(3, 17), "3 hosts · 17 nodes");
        assert_eq!(search_progress(4, 9), "Searching… 4/9");
        assert_eq!(match_position(1, 3), "1/3");
        assert_eq!(milliseconds(52.64), "52.6 ms");
        assert_eq!(milliseconds_running(1204.4), "1204 ms…");
    }
}
