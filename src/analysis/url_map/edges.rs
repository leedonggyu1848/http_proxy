//! Domain-level and URL-level relationship edge tracking (`Referer`, `Origin`,
//! and `3xx` `Location` redirects).

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::url::{OriginKey, is_third_party, registrable_domain};

/// Maximum distinct domain-level edges retained to bound memory.
const MAX_DOMAIN_EDGES: usize = 10_000;
/// Maximum distinct URL-level edges retained to bound memory.
const MAX_URL_EDGES: usize = 20_000;

/// Source of a directed relationship edge between two pages or hosts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum EdgeKind {
    /// Referring page loaded a resource or API (`Referer`).
    Referer,
    /// Cross-origin request declared an `Origin` header (CORS).
    Origin,
    /// `3xx` response redirected to a `Location` target.
    Redirect,
}

impl EdgeKind {
    /// Short display label used in graphs and exports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Referer => "Referer",
            Self::Origin => "Origin",
            Self::Redirect => "Redirect",
        }
    }
}

/// Directed edge between two normalized host names.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DomainEdgeKey {
    /// Source host (referring page, origin, or redirecting host).
    pub from_host: String,
    /// Destination host (requested resource or redirect destination).
    pub to_host: String,
    /// Relationship type.
    pub kind: EdgeKind,
}

/// Aggregated metadata on one domain edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DomainEdgeStats {
    /// Number of observed exchanges that produced this edge.
    pub count: u64,
    /// Whether `from_host` and `to_host` have different registrable sites (eTLD+1).
    pub third_party: bool,
}

/// Directed edge between two path-only URLs (query strings always stripped).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct UrlEdgeKey {
    /// Normalized path-only source URL (no query values).
    pub from_url: String,
    /// Normalized path-only destination URL (no query values).
    pub to_url: String,
    /// Relationship type.
    pub kind: EdgeKind,
}

/// Per-host summary shown in the domain relationship graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainSummary {
    /// Normalized lowercase host name.
    pub host: String,
    /// Registrable site (eTLD+1) derived via the Public Suffix List.
    pub site: String,
    /// Origins observed on this host (e.g. `http://...`, `https://...`).
    pub origins: BTreeSet<OriginKey>,
    /// Total requests targeting this host.
    pub request_count: u64,
    /// Total error requests (`4xx`, `5xx`, failures) on this host.
    pub error_count: u64,
    /// Total transferred bytes on this host.
    pub total_bytes: u64,
    /// Requests to this host coming from a different registrable site.
    pub third_party_hits: u64,
    /// Requests to this host that were first-party or top-level navigations.
    pub first_party_hits: u64,
}

impl DomainSummary {
    fn new(host: &str) -> Self {
        Self {
            host: host.to_owned(),
            site: registrable_domain(host),
            origins: BTreeSet::new(),
            request_count: 0,
            error_count: 0,
            total_bytes: 0,
            third_party_hits: 0,
            first_party_hits: 0,
        }
    }

    /// Reports whether this host was observed as a third-party dependency.
    #[must_use]
    pub const fn is_third_party(&self) -> bool {
        self.third_party_hits > 0
    }
}

/// Directed relationship graph collected alongside the URL tree.
#[derive(Clone, Debug, Default)]
pub struct RelationshipGraph {
    domains: BTreeMap<String, DomainSummary>,
    domain_edges: BTreeMap<DomainEdgeKey, DomainEdgeStats>,
    url_edges: BTreeMap<UrlEdgeKey, u64>,
    topology_version: u64,
    version: u64,
}

impl RelationshipGraph {
    /// Clears all domains and edges.
    pub fn clear(&mut self) {
        self.domains.clear();
        self.domain_edges.clear();
        self.url_edges.clear();
        self.topology_version = self.topology_version.saturating_add(1);
        self.version = self.version.saturating_add(1);
    }

    /// Monotonic counter bumped whenever the set of domains or domain edges changes.
    #[must_use]
    pub const fn topology_version(&self) -> u64 {
        self.topology_version
    }

    /// Monotonic counter bumped on any change to domains, counts, or edges.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Returns all known domains keyed by host name.
    #[must_use]
    pub const fn domains(&self) -> &BTreeMap<String, DomainSummary> {
        &self.domains
    }

    /// Returns all domain-level edges.
    #[must_use]
    pub const fn domain_edges(&self) -> &BTreeMap<DomainEdgeKey, DomainEdgeStats> {
        &self.domain_edges
    }

    /// Returns all path-only URL-level edges (query values are never stored).
    #[must_use]
    pub const fn url_edges(&self) -> &BTreeMap<UrlEdgeKey, u64> {
        &self.url_edges
    }

    /// Records a request targeting `origin`.
    pub fn record_request(&mut self, origin: &OriginKey, third_party: bool) {
        let host = origin.host();
        let is_new = !self.domains.contains_key(host);
        let summary = self
            .domains
            .entry(host.to_owned())
            .or_insert_with(|| DomainSummary::new(host));
        let _ = summary.origins.insert(origin.clone());
        summary.request_count = summary.request_count.saturating_add(1);
        if third_party {
            summary.third_party_hits = summary.third_party_hits.saturating_add(1);
        } else {
            summary.first_party_hits = summary.first_party_hits.saturating_add(1);
        }
        if is_new {
            self.topology_version = self.topology_version.saturating_add(1);
        }
        self.version = self.version.saturating_add(1);
    }

    /// Increments the error count for `host`.
    pub fn record_error(&mut self, host: &str) {
        if let Some(summary) = self.domains.get_mut(host) {
            summary.error_count = summary.error_count.saturating_add(1);
            self.version = self.version.saturating_add(1);
        }
    }

    /// Adds observed transferred bytes for `host`.
    pub fn add_bytes(&mut self, host: &str, delta: u64) {
        if let Some(summary) = self.domains.get_mut(host) {
            summary.total_bytes = summary.total_bytes.saturating_add(delta);
            self.version = self.version.saturating_add(1);
        }
    }

    /// Records both a domain-level edge and a path-only URL-level edge.
    ///
    /// Callers must pass query-stripped path-only URLs for `from_url` and `to_url`.
    pub fn record_edge(
        &mut self,
        from_host: &str,
        to_host: &str,
        from_url: String,
        to_url: String,
        kind: EdgeKind,
    ) {
        if from_host.is_empty() || to_host.is_empty() {
            return;
        }
        let third_party = is_third_party(to_host, from_host);
        self.ensure_domain(from_host);
        self.ensure_domain(to_host);

        let domain_key = DomainEdgeKey {
            from_host: from_host.to_owned(),
            to_host: to_host.to_owned(),
            kind,
        };
        if let Some(stats) = self.domain_edges.get_mut(&domain_key) {
            stats.count = stats.count.saturating_add(1);
        } else if self.domain_edges.len() < MAX_DOMAIN_EDGES {
            let _ = self.domain_edges.insert(
                domain_key,
                DomainEdgeStats {
                    count: 1,
                    third_party,
                },
            );
            self.topology_version = self.topology_version.saturating_add(1);
        }

        let url_key = UrlEdgeKey {
            from_url,
            to_url,
            kind,
        };
        if let Some(count) = self.url_edges.get_mut(&url_key) {
            *count = count.saturating_add(1);
        } else if self.url_edges.len() < MAX_URL_EDGES {
            let _ = self.url_edges.insert(url_key, 1);
        }
        self.version = self.version.saturating_add(1);
    }

    fn ensure_domain(&mut self, host: &str) {
        if !self.domains.contains_key(host) {
            let _ = self
                .domains
                .insert(host.to_owned(), DomainSummary::new(host));
            self.topology_version = self.topology_version.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_domain_and_url_edges_and_marks_third_party() {
        let mut graph = RelationshipGraph::default();
        let app = OriginKey::new("https", "app.example.com", None);
        let cdn = OriginKey::new("https", "cdn.tracker.net", None);

        graph.record_request(&app, false);
        graph.record_request(&cdn, true);
        graph.record_edge(
            app.host(),
            cdn.host(),
            "https://app.example.com/".to_owned(),
            "https://cdn.tracker.net/sdk.js".to_owned(),
            EdgeKind::Referer,
        );

        let key = DomainEdgeKey {
            from_host: "app.example.com".to_owned(),
            to_host: "cdn.tracker.net".to_owned(),
            kind: EdgeKind::Referer,
        };
        let stats = graph.domain_edges().get(&key).copied();
        assert_eq!(
            stats,
            Some(DomainEdgeStats {
                count: 1,
                third_party: true,
            })
        );
        assert!(
            graph
                .domains()
                .get("cdn.tracker.net")
                .is_some_and(DomainSummary::is_third_party)
        );
        assert!(
            graph
                .domains()
                .get("app.example.com")
                .is_some_and(|d| !d.is_third_party())
        );
    }
}
