//! Public Suffix List (eTLD+1) site extraction and third-party classification.

use std::net::IpAddr;

/// Relationship of a host to the referring or originating site.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PartyKind {
    /// Same registrable site (eTLD+1) or direct top-level navigation.
    FirstParty,
    /// Called from a different registrable site (eTLD+1).
    ThirdParty,
}

/// Computes the registrable site (eTLD+1) for `host` using the Mozilla Public
/// Suffix List.
///
/// IP addresses and single-label local hostnames (such as `localhost`) return
/// themselves in lowercase. Unknown private TLDs fall back to their last two
/// dot-separated labels.
#[must_use]
pub fn registrable_domain(host: &str) -> String {
    let cleaned = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if cleaned.is_empty() || cleaned.parse::<IpAddr>().is_ok() || !cleaned.contains('.') {
        return cleaned;
    }
    if let Some(domain) = psl::domain(cleaned.as_bytes())
        && let Ok(text) = std::str::from_utf8(domain.as_bytes())
    {
        return text.to_owned();
    }
    // Fallback for private/unlisted TLDs (e.g., `app.service.internal`).
    let mut parts = cleaned.rsplit('.');
    match (parts.next(), parts.next()) {
        (Some(tld), Some(label)) if !tld.is_empty() && !label.is_empty() => {
            format!("{label}.{tld}")
        }
        _ => cleaned,
    }
}

/// Reports whether `target_host` belongs to a different registrable site
/// (eTLD+1) than `context_host` (from `Referer` or `Origin`).
#[must_use]
pub fn is_third_party(target_host: &str, context_host: &str) -> bool {
    let target_site = registrable_domain(target_host);
    let context_site = registrable_domain(context_host);
    !target_site.is_empty() && !context_site.is_empty() && target_site != context_site
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_etld_plus_one_with_public_suffix_list() {
        assert_eq!(registrable_domain("www.example.com"), "example.com");
        assert_eq!(
            registrable_domain("api.shop.example.co.uk"),
            "example.co.uk"
        );
        assert_eq!(registrable_domain("cdn.sub.example.or.jp"), "example.or.jp");
        assert_eq!(registrable_domain("EXAMPLE.COM."), "example.com");
        assert_eq!(registrable_domain("127.0.0.1"), "127.0.0.1");
        assert_eq!(registrable_domain("[::1]"), "::1");
        assert_eq!(registrable_domain("localhost"), "localhost");
        assert_eq!(registrable_domain("api.app.internal"), "app.internal");
    }

    #[test]
    fn classifies_first_party_and_third_party_hosts() {
        assert!(!is_third_party("api.example.com", "www.example.com"));
        assert!(!is_third_party("api.example.co.uk", "www.example.co.uk"));
        assert!(is_third_party("b.co.uk", "a.co.uk"));
        assert!(is_third_party("cdn.tracker.net", "shop.example.com"));
        assert!(!is_third_party("127.0.0.1", "127.0.0.1"));
        assert!(is_third_party("127.0.0.2", "127.0.0.1"));
        assert!(is_third_party("cdn.thirdparty.test", "app.example.test"));
        assert!(!is_third_party("api.example.test", "app.example.test"));
    }
}
