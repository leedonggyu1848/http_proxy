//! The media type named by a `Content-Type` header value.

/// A `Content-Type` value reduced to its essence: `type/subtype` in ASCII
/// lowercase, without parameters or surrounding whitespace.
///
/// Header values are parsed once with [`MediaType::parse`], at the point
/// where headers are captured, so other code matches on this type instead of
/// re-reading the raw text.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MediaType {
    essence: Box<str>,
    /// Byte index of the `/` between type and subtype.
    slash: usize,
}

impl MediaType {
    /// Parses a `Content-Type` value, ignoring case, parameters after `;`
    /// and surrounding whitespace.
    ///
    /// Returns `None` when the value before `;` is not `type/subtype` with
    /// both parts made of HTTP token characters.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let essence = value.split(';').next()?.trim();
        let (kind, subtype) = essence.split_once('/')?;
        if !is_token(kind) || !is_token(subtype) {
            return None;
        }
        Some(Self {
            essence: essence.to_ascii_lowercase().into_boxed_str(),
            slash: kind.len(),
        })
    }

    /// Returns `type/subtype` in lowercase, for example `application/json`.
    #[must_use]
    pub fn essence(&self) -> &str {
        &self.essence
    }

    /// Returns the top-level type, for example `application`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.essence[..self.slash]
    }

    /// Returns the subtype, for example `problem+json`.
    #[must_use]
    pub fn subtype(&self) -> &str {
        &self.essence[self.slash + 1..]
    }

    /// Returns the structured syntax suffix after the last `+` of the
    /// subtype, for example `json` in `application/problem+json`, or `None`
    /// when the subtype has no suffix.
    #[must_use]
    pub fn suffix(&self) -> Option<&str> {
        let (_, suffix) = self.subtype().rsplit_once('+')?;
        (!suffix.is_empty()).then_some(suffix)
    }

    /// Reports whether the body is JSON: subtype `json` or a `+json` suffix.
    #[must_use]
    pub fn is_json(&self) -> bool {
        self.subtype() == "json" || self.suffix() == Some("json")
    }

    /// Reports whether the subtype carries the `+xml` suffix.
    #[must_use]
    pub fn has_xml_suffix(&self) -> bool {
        self.suffix() == Some("xml")
    }
}

/// RFC 9110 `token`: one or more visible ASCII characters other than
/// delimiters.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::MediaType;

    fn essence(value: &str) -> Option<String> {
        MediaType::parse(value).map(|media| media.essence().to_owned())
    }

    #[test]
    fn lowercases_and_drops_parameters_and_whitespace() {
        for (value, expected) in [
            ("text/html", "text/html"),
            ("Text/HTML", "text/html"),
            ("  IMAGE/PNG  ", "image/png"),
            ("application/json; charset=utf-8", "application/json"),
            (" text/plain ; charset=\"a;b\"", "text/plain"),
            ("text/plain;", "text/plain"),
        ] {
            assert_eq!(essence(value).as_deref(), Some(expected), "{value:?}");
        }
    }

    #[test]
    fn splits_type_subtype_and_suffix() {
        let media = MediaType::parse("Application/Problem+JSON; charset=utf-8");
        let media = media.as_ref();
        assert_eq!(media.map(MediaType::kind), Some("application"));
        assert_eq!(media.map(MediaType::subtype), Some("problem+json"));
        assert_eq!(media.and_then(MediaType::suffix), Some("json"));
        for (value, suffix) in [
            ("application/json", None),
            ("application/atom+xml", Some("xml")),
            ("application/vnd.a+b+json", Some("json")),
            ("application/odd+", None),
        ] {
            let media = MediaType::parse(value);
            assert_eq!(
                media.as_ref().and_then(MediaType::suffix),
                suffix,
                "{value}"
            );
        }
    }

    #[test]
    fn classifies_json_and_xml_suffixes() {
        let check = |value: &str| {
            MediaType::parse(value).map(|media| (media.is_json(), media.has_xml_suffix()))
        };
        assert_eq!(check("application/json"), Some((true, false)));
        assert_eq!(check("application/LD+JSON"), Some((true, false)));
        assert_eq!(check("image/svg+xml"), Some((false, true)));
        assert_eq!(check("application/xml"), Some((false, false)));
        assert_eq!(check("application/jsonx"), Some((false, false)));
    }

    #[test]
    fn rejects_values_that_are_not_type_slash_subtype() {
        for value in [
            "",
            "   ",
            "; charset=utf-8",
            "jsonish",
            "text/",
            "/plain",
            "text/plain/extra",
            "text /plain",
            "te xt/plain",
            "text/pl\u{e4}in",
        ] {
            assert_eq!(essence(value), None, "{value:?}");
        }
    }
}
