use std::time::Duration;

use http::header::HeaderName;
use http_inspector_exchange::{CaptureSource, Exchange, HeaderPreview, Outcome};

use crate::filter::{TargetParts, content_type};
use crate::search::{MatchOptions, Matcher};

use super::FilterKey;

#[derive(Debug)]
pub(super) enum Predicate {
    Method(String),
    Url(Box<Matcher>),
    Host(Host),
    Path(Box<Matcher>),
    Status(Status),
    Type(String),
    Header(Header),
    Size(Comparison<u64>),
    Duration(Comparison<Duration>),
    Is(Kind),
}

impl Predicate {
    pub(super) fn parse(key: FilterKey, value: &str) -> Result<Self, String> {
        if value.is_empty() {
            return Err(format!("Missing value for `{}`", key.name()));
        }
        let invalid = || format!("Invalid {} value `{value}`", key.name());
        let literal = || {
            Matcher::new(value, MatchOptions::default())
                .map(Box::new)
                .map_err(|_| invalid())
        };
        match key {
            FilterKey::Method => {
                http::Method::from_bytes(value.as_bytes()).map_err(|_| invalid())?;
                Ok(Self::Method(value.to_ascii_lowercase()))
            }
            FilterKey::Url => literal().map(Self::Url),
            FilterKey::Path => literal().map(Self::Path),
            FilterKey::Host => Host::parse(value).ok_or_else(invalid).map(Self::Host),
            FilterKey::Status => Status::parse(value).ok_or_else(invalid).map(Self::Status),
            FilterKey::Type => Ok(Self::Type(value.to_ascii_lowercase())),
            FilterKey::Header | FilterKey::RequestHeader | FilterKey::ResponseHeader => {
                Header::parse(key, value)
                    .ok_or_else(invalid)
                    .map(Self::Header)
            }
            FilterKey::Size => Comparison::parse(value, bytes)
                .ok_or_else(invalid)
                .map(Self::Size),
            FilterKey::Duration => Comparison::parse(value, duration)
                .ok_or_else(invalid)
                .map(Self::Duration),
            FilterKey::Is => Kind::parse(value).ok_or_else(invalid).map(Self::Is),
        }
    }

    pub(super) fn matches(&self, record: &Exchange) -> bool {
        let request = record.request();
        match self {
            Self::Method(method) => request.method().as_str().eq_ignore_ascii_case(method),
            Self::Url(matcher) => matcher.is_match(request.target().as_bytes()),
            Self::Host(host) => host.matches(TargetParts::of(request.target()).host),
            Self::Path(matcher) => {
                matcher.is_match(TargetParts::of(request.target()).path.as_bytes())
            }
            Self::Status(status) => match record.outcome() {
                Outcome::Response(response) => status.matches(response.status().as_u16()),
                _ => false,
            },
            Self::Type(kind) => {
                content_type(record).is_some_and(|ty| ty.eq_ignore_ascii_case(kind))
            }
            Self::Header(header) => header.matches(record),
            Self::Size(comparison) => match record.outcome() {
                Outcome::Response(response) => comparison.matches(response.body().observed_bytes()),
                _ => false,
            },
            Self::Duration(comparison) => record
                .timing()
                .complete()
                .is_some_and(|duration| comparison.matches(duration)),
            Self::Is(kind) => kind.matches(record),
        }
    }
}

#[derive(Debug)]
pub(super) enum Host {
    Contains(Box<Matcher>),
    Subdomain(String),
}

impl Host {
    fn parse(value: &str) -> Option<Self> {
        if let Some(rest) = value.strip_prefix("*.") {
            if rest.is_empty() || rest.contains('*') {
                return None;
            }
            return Some(Self::Subdomain(format!(".{}", rest.to_ascii_lowercase())));
        }
        if value.contains('*') {
            return None;
        }
        Matcher::new(value, MatchOptions::default())
            .ok()
            .map(Box::new)
            .map(Self::Contains)
    }

    fn matches(&self, authority: &str) -> bool {
        match self {
            Self::Contains(matcher) => matcher.is_match(authority.as_bytes()),
            Self::Subdomain(suffix) => {
                let authority = if suffix.contains(':') {
                    authority
                } else {
                    authority.split(':').next().unwrap_or(authority)
                };
                authority.len() > suffix.len()
                    && authority
                        .get(authority.len() - suffix.len()..)
                        .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
            }
        }
    }
}

#[derive(Debug)]
pub(super) struct Header {
    side: FilterKey,
    name: HeaderName,
    value: HeaderValue,
}

#[derive(Debug)]
enum HeaderValue {
    Any,
    Empty,
    Contains(Box<Matcher>),
}

impl Header {
    fn parse(side: FilterKey, text: &str) -> Option<Self> {
        let (name, value) = text
            .split_once('=')
            .map_or((text, None), |(name, value)| (name, Some(value)));
        Some(Self {
            side,
            name: HeaderName::from_bytes(name.as_bytes()).ok()?,
            value: match value {
                None => HeaderValue::Any,
                Some("") => HeaderValue::Empty,
                Some(text) => HeaderValue::Contains(Box::new(
                    Matcher::new(text, MatchOptions::default()).ok()?,
                )),
            },
        })
    }

    fn matches(&self, record: &Exchange) -> bool {
        (self.side != FilterKey::ResponseHeader && self.in_headers(record.request().headers()))
            || (self.side != FilterKey::RequestHeader
                && matches!(record.outcome(), Outcome::Response(response) if self.in_headers(response.headers())))
    }

    fn in_headers(&self, headers: &HeaderPreview) -> bool {
        headers
            .get(self.name.as_str())
            .any(|value| match &self.value {
                HeaderValue::Any => true,
                HeaderValue::Empty => value.is_empty(),
                HeaderValue::Contains(matcher) => matcher.is_match(value.as_bytes()),
            })
    }
}

#[derive(Debug)]
pub(super) enum Status {
    Class(u16),
    Code(Comparison<u16>),
}

impl Status {
    fn parse(value: &str) -> Option<Self> {
        if let Some(class) = value.to_ascii_lowercase().strip_suffix("xx") {
            let class = class
                .parse()
                .ok()
                .filter(|number| (1..=5).contains(number))?;
            return Some(Self::Class(class));
        }
        Comparison::parse(value, |code| {
            http::StatusCode::from_bytes(code.as_bytes())
                .ok()
                .map(|status| status.as_u16())
        })
        .map(Self::Code)
    }

    fn matches(&self, code: u16) -> bool {
        match self {
            Self::Class(class) => code / 100 == *class,
            Self::Code(comparison) => comparison.matches(code),
        }
    }
}

#[derive(Debug)]
enum Operator {
    Greater,
    AtLeast,
    Less,
    AtMost,
    Equal,
}

#[derive(Debug)]
pub(super) struct Comparison<T> {
    operator: Operator,
    value: T,
}

impl<T: Copy + Ord> Comparison<T> {
    fn parse(text: &str, parse_value: impl FnOnce(&str) -> Option<T>) -> Option<Self> {
        let (operator, value) = [
            (">=", Operator::AtLeast),
            ("<=", Operator::AtMost),
            (">", Operator::Greater),
            ("<", Operator::Less),
            ("=", Operator::Equal),
        ]
        .into_iter()
        .find_map(|(prefix, operator)| text.strip_prefix(prefix).map(|value| (operator, value)))
        .unwrap_or((Operator::Equal, text));
        Some(Self {
            operator,
            value: parse_value(value)?,
        })
    }

    fn matches(&self, value: T) -> bool {
        match self.operator {
            Operator::Greater => value > self.value,
            Operator::AtLeast => value >= self.value,
            Operator::Less => value < self.value,
            Operator::AtMost => value <= self.value,
            Operator::Equal => value == self.value,
        }
    }
}

fn bytes(value: &str) -> Option<u64> {
    let value = value.to_ascii_lowercase();
    let (digits, multiplier) = [("kb", 1024), ("mb", 1024 * 1024), ("b", 1)]
        .into_iter()
        .find_map(|(unit, multiplier)| value.strip_suffix(unit).map(|digits| (digits, multiplier)))
        .unwrap_or((&value, 1));
    digits.parse::<u64>().ok()?.checked_mul(multiplier)
}

fn duration(value: &str) -> Option<Duration> {
    let value = value.to_ascii_lowercase();
    if let Some(seconds) = value.strip_suffix('s').filter(|_| !value.ends_with("ms")) {
        return seconds.parse().ok().map(Duration::from_secs);
    }
    value
        .strip_suffix("ms")
        .unwrap_or(&value)
        .parse()
        .ok()
        .map(Duration::from_millis)
}

#[derive(Debug)]
pub(super) enum Kind {
    Failed,
    Pending,
    Tunnel,
    Replay,
    Https,
}

impl Kind {
    fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "failed" => Some(Self::Failed),
            "pending" => Some(Self::Pending),
            "tunnel" => Some(Self::Tunnel),
            "replay" => Some(Self::Replay),
            "https" => Some(Self::Https),
            _ => None,
        }
    }

    fn matches(&self, record: &Exchange) -> bool {
        match self {
            Self::Failed => record.outcome().is_failure(),
            Self::Pending => record.outcome().is_in_flight(),
            Self::Tunnel => matches!(record.outcome(), Outcome::Tunnel(_)),
            Self::Replay => record.replay_of().is_some(),
            Self::Https => {
                record.source() != CaptureSource::HttpProxy
                    || record
                        .request()
                        .target()
                        .get(..8)
                        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Comparison, Duration, duration};

    #[test]
    fn compares_duration_without_rounding_to_milliseconds() {
        let threshold = Comparison::parse(">500ms", duration);
        assert!(threshold.is_some_and(|comparison| {
            !comparison.matches(Duration::from_millis(500))
                && comparison.matches(Duration::from_nanos(500_000_001))
        }));
    }
}
