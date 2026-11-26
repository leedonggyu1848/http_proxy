//! Strings that the workbench shell shows.
//!
//! The shell keeps its labels in one catalog so a translation layer can
//! replace [`ShellText::text`] and the count formatters without touching the
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
    /// Accessible name of the outcome tabs above the request list.
    RequestOutcome,
    Searching,
}

impl ShellText {
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
            Self::Filter => "Filter",
            Self::InvalidFilter => "Invalid pattern",
            Self::RequestOutcome => "Request outcome",
            Self::Searching => "Searching…",
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
    }
}
