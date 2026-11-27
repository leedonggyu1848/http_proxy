//! Language-neutral numeric, unit, and symbol formatting helpers shared by the workbench.
//!
//! All localized UI strings live under `crate::ui::i18n`.

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

/// Status bar entries left out for lack of room.
pub(crate) fn more_entries(count: usize) -> String {
    format!("+{count}")
}

/// A menu button label: the current choice and the menu arrow.
pub(crate) fn menu_label(label: &str) -> String {
    format!("{label} ▼")
}

/// A folded ID node of the URL map tree.
pub(crate) fn folded_ids(label: &str, count: usize) -> String {
    format!("{label} ({count} IDs)")
}

/// IDs folded into one node and their kinds.
pub(crate) fn folded_kinds(count: usize, kinds: &[&str]) -> String {
    if kinds.is_empty() {
        count.to_string()
    } else {
        format!("{count} · {}", kinds.join(", "))
    }
}

/// How often a name was seen.
pub(crate) fn times(count: u64) -> String {
    format!("{count}×")
}

/// Position of the current search match in a body.
pub(crate) fn match_position(current: usize, total: usize) -> String {
    format!("{current}/{total}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::i18n::{Language, toolbar};

    #[test]
    fn counts_name_only_what_differs() {
        assert_eq!(
            toolbar::request_count(Language::English, 12, 12),
            "12 requests"
        );
        assert_eq!(
            toolbar::request_count(Language::English, 3, 12),
            "3 of 12 requests"
        );
        assert_eq!(
            toolbar::request_count(Language::Korean, 12, 12),
            "요청 12건"
        );
        assert_eq!(
            toolbar::request_count(Language::Korean, 3, 12),
            "요청 3/12건"
        );
        assert_eq!(toolbar::failure_count(Language::English, 2), "2 failed");
        assert_eq!(toolbar::failure_count(Language::Korean, 2), "실패 2건");
        assert_eq!(
            toolbar::map_count(Language::English, 3, 17),
            "3 hosts · 17 nodes"
        );
        assert_eq!(
            toolbar::map_count(Language::Korean, 3, 17),
            "호스트 3 · 노드 17"
        );
        assert_eq!(
            toolbar::search_progress(Language::English, 4, 9),
            "Searching… 4/9"
        );
        assert_eq!(
            toolbar::search_progress(Language::Korean, 4, 9),
            "검색 중… 4/9"
        );
        assert_eq!(match_position(1, 3), "1/3");
        assert_eq!(milliseconds(52.64), "52.6 ms");
        assert_eq!(milliseconds_running(1204.4), "1204 ms…");
    }
}
