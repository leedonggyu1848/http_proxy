//! Shared projections for the request list and field filters.

use http_inspector_exchange::{Exchange, Outcome};

/// The host and path of a request target, without the scheme.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TargetParts<'a> {
    /// `host[:port]`, empty for origin-form targets such as `/path` or `*`.
    pub host: &'a str,
    /// Path and query starting at `/`, empty for authority-form `CONNECT`
    /// targets such as `example.com:443`.
    pub path: &'a str,
}

impl<'a> TargetParts<'a> {
    /// Splits an absolute, origin-form, or authority-form target.
    #[must_use]
    pub fn of(target: &'a str) -> Self {
        let rest = target
            .split_once("://")
            .filter(|(scheme, _)| {
                scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
            })
            .map_or(target, |(_, authority_and_path)| authority_and_path);
        if rest.starts_with('/') || rest == "*" {
            return Self {
                host: "",
                path: rest,
            };
        }
        let split = rest.find(['/', '?']).unwrap_or(rest.len());
        let (host, path) = rest.split_at(split);
        Self { host, path }
    }

    /// The text that identifies the request: the path, or the host when
    /// the target has no path.
    #[must_use]
    pub const fn primary(self) -> &'a str {
        if self.path.is_empty() {
            self.host
        } else {
            self.path
        }
    }

    /// The host shown beside the path, if it is not already the primary text.
    #[must_use]
    pub const fn secondary(self) -> Option<&'a str> {
        if self.path.is_empty() || self.host.is_empty() {
            None
        } else {
            Some(self.host)
        }
    }
}

/// Short media type shown in the Type column.
#[must_use]
pub fn content_type(record: &Exchange) -> Option<&str> {
    let Outcome::Response(response) = record.outcome() else {
        return None;
    };
    let subtype = response.media_type()?.subtype();
    let subtype = subtype.rsplit('+').next().unwrap_or(subtype);
    Some(subtype.strip_prefix("x-").unwrap_or(subtype))
}
