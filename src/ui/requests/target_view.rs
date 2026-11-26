//! How a request target is split and shortened in the request list.

use std::borrow::Cow;

const ELLIPSIS: char = '…';

use http_inspector_analysis::filter::TargetParts;

/// The host every target shares, or `None` when they differ or are empty.
pub(super) fn shared_host<'a>(mut targets: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let host = TargetParts::of(targets.next()?).host;
    (!host.is_empty() && targets.all(|target| TargetParts::of(target).host == host)).then_some(host)
}

/// Whether the whole `host` fits after `primary` and a gap of `gap_chars`
/// within `max_chars`. A cut host such as `127.0.0.1:1…` reads like another
/// port, so the list shows the host whole or not at all.
pub(super) fn host_fits(max_chars: usize, primary: &str, gap_chars: usize, host: &str) -> bool {
    primary.chars().count() + gap_chars + host.chars().count() <= max_chars
}

/// Shortens `path` to at most `max_chars` characters by eliding its middle,
/// so that the start of the path and its last segment stay readable, as in
/// `/api/v1/…/users.json`. A last segment too long to keep, such as a long
/// query, falls back to cutting the end.
pub(super) fn middle_ellipsis(path: &str, max_chars: usize) -> Cow<'_, str> {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_chars {
        return Cow::Borrowed(path);
    }
    let Some(budget) = max_chars.checked_sub(1).filter(|&budget| budget > 0) else {
        return Cow::Owned(if max_chars == 0 {
            String::new()
        } else {
            ELLIPSIS.to_string()
        });
    };
    // Keep at least a few leading characters so the path's start stays visible.
    let min_head = budget.min(3);
    // The last segment starts at the last `/` before the query.
    let query_start = chars.iter().position(|&c| c == '?').unwrap_or(chars.len());
    let tail_start = chars
        .iter()
        .take(query_start)
        .rposition(|&c| c == '/')
        .unwrap_or(0);
    let tail = chars.get(tail_start..).unwrap_or_default();
    if tail_start == 0 || tail.len() + min_head > budget {
        let head: String = chars.iter().take(budget).collect();
        return Cow::Owned(format!("{head}{ELLIPSIS}"));
    }
    let head = chars.get(..budget - tail.len()).unwrap_or_default();
    // Cut the head after a `/` when one exists, so that whole segments remain.
    let head = head
        .iter()
        .rposition(|&c| c == '/')
        .filter(|&slash| slash > 0)
        .and_then(|slash| head.get(..=slash))
        .unwrap_or(head);
    let mut shortened: String = head.iter().collect();
    shortened.push(ELLIPSIS);
    shortened.extend(tail);
    Cow::Owned(shortened)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_absolute_origin_and_authority_targets() {
        let absolute = TargetParts::of("http://127.0.0.1:8080/users.json?page=7");
        assert_eq!(absolute.host, "127.0.0.1:8080");
        assert_eq!(absolute.primary(), "/users.json?page=7");
        assert_eq!(absolute.secondary(), Some("127.0.0.1:8080"));

        let query_only = TargetParts::of("https://example.test?q=1");
        assert_eq!((query_only.host, query_only.path), ("example.test", "?q=1"));

        let authority = TargetParts::of("example.test:443");
        assert_eq!(authority.primary(), "example.test:443");
        assert_eq!(authority.secondary(), None);

        let origin = TargetParts::of("/health");
        assert_eq!((origin.host, origin.primary()), ("", "/health"));
        assert_eq!(TargetParts::of("*").primary(), "*");
    }

    #[test]
    fn finds_a_host_only_when_all_targets_share_it() {
        let same = ["http://a.test/x", "https://a.test/y?z", "a.test"];
        assert_eq!(shared_host(same.into_iter()), Some("a.test"));
        let mixed = ["http://a.test/x", "http://b.test/x"];
        assert_eq!(shared_host(mixed.into_iter()), None);
        assert_eq!(shared_host(["/origin-form"].into_iter()), None);
        assert_eq!(shared_host(std::iter::empty()), None);
    }

    #[test]
    fn shows_a_host_only_when_it_fits_whole() {
        let host = "127.0.0.1:18931";
        // 11 + 1 + 15 = 27 characters.
        assert!(host_fits(27, "/users.json", 1, host));
        assert!(!host_fits(26, "/users.json", 1, host));
        // A wider gap leaves less room for the host.
        assert!(!host_fits(27, "/users.json", 2, host));
        assert!(host_fits(4, "", 0, "a.b:"));
    }

    #[test]
    fn elides_the_middle_and_keeps_the_last_segment() {
        let path = "/api/v1/projects/42/members/users.json";
        assert_eq!(middle_ellipsis(path, 80), path);
        let short = middle_ellipsis(path, 22);
        assert_eq!(short, "/api/v1/…/users.json");
        assert!(short.chars().count() <= 22);
        assert_eq!(middle_ellipsis(path, 15), "/ap…/users.json");
        let nested_query = "/api/v1/projects/x?next=/a/b";
        assert_eq!(middle_ellipsis(nested_query, 20), "/api/…/x?next=/a/b");
    }

    #[test]
    fn cuts_the_end_when_the_last_segment_is_too_long() {
        let query = "/search?q=a-very-long-query-string";
        assert_eq!(middle_ellipsis(query, 12), "/search?q=a…");
        let long_tail = "/a/an-extremely-long-file-name.json";
        assert_eq!(middle_ellipsis(long_tail, 10), "/a/an-ext…");
        assert_eq!(middle_ellipsis("/abc", 1), "…");
        assert_eq!(middle_ellipsis("/abc", 0), "");
        assert_eq!(
            middle_ellipsis("/한글/경로/파일.json", 12),
            "/한글…/파일.json"
        );
    }
}
