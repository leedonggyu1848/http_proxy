//! Structure of one body and the aggregate of every body in a group.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value;

use super::multipart::{Boundary, MAX_PARTS, PartObservation, parse_parts};
use super::shape::{FieldShape, MAX_SHAPE_FIELDS};
use super::value::bump;
use crate::content::detect::{self as content_detect, Basis, Detection};
use crate::content::format::BodyFormat;
use crate::url_map::percent_decode_display;
use crate::{MediaType, ReadBodyOutcome};

/// How a body is parsed for its structure, following the format
/// [`content_detect`] decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BodyParse {
    /// Declared JSON: `application/json` or a `+json` type.
    Json,
    /// Form fields: `application/x-www-form-urlencoded`.
    Form,
    /// `multipart/form-data` with a valid boundary.
    Multipart(Boundary),
    /// Sniffed JSON: described as JSON only when it parses as JSON.
    MaybeJson,
    /// Any other format; the body is counted but not parsed.
    Unstructured,
}

impl BodyParse {
    /// Follows `detection`; the raw `Content-Type` header still carries the
    /// multipart boundary parameter that [`MediaType`] drops.
    #[must_use]
    pub fn new(detection: Detection, raw_content_type: Option<&str>) -> Self {
        match (detection.format, detection.basis) {
            (BodyFormat::Json, Basis::ContentType) => Self::Json,
            (BodyFormat::Json, Basis::Sniffed) => Self::MaybeJson,
            (BodyFormat::Form, _) => Self::Form,
            (BodyFormat::Multipart, _) => raw_content_type
                .and_then(Boundary::from_content_type)
                .map_or(Self::Unstructured, Self::Multipart),
            (
                BodyFormat::Css
                | BodyFormat::Markup
                | BodyFormat::Xml
                | BodyFormat::JavaScript
                | BodyFormat::Text,
                _,
            ) => Self::Unstructured,
        }
    }

    /// Reports whether the body bytes must be read to describe it.
    #[must_use]
    pub const fn needs_bytes(&self) -> bool {
        !matches!(self, Self::Unstructured)
    }
}

/// Why a body was left out of structure aggregation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ExclusionReason {
    /// The body was cut short or not every byte was kept.
    Truncated,
    /// The decoded body was larger than the analysis limit.
    TooLarge,
    /// The `Content-Encoding` could not be removed.
    DecodeFailed,
    /// The stored body could not be read.
    Unavailable,
}

impl ExclusionReason {
    /// Label shown in the evidence line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Truncated => "truncated",
            Self::TooLarge => "too large",
            Self::DecodeFailed => "decode failed",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for ExclusionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Result of describing one body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BodyAnalysis {
    /// The body had no bytes.
    Empty,
    /// The body could not be read completely.
    Excluded(ExclusionReason),
    /// The body did not parse as its declared format.
    Invalid,
    /// The body was complete but its format carries no field structure.
    Unstructured,
    /// JSON or form fields.
    Fields(FieldShape),
    /// Multipart part names and types.
    Parts(Vec<PartObservation>),
}

impl BodyAnalysis {
    /// Describes a body parsed as `parse` from the outcome of reading it.
    #[must_use]
    pub fn describe(parse: &BodyParse, read: ReadBodyOutcome) -> Self {
        Self::describe_with(read, |_| parse.clone())
    }

    /// Describes a body whose `Content-Type` did not name a format, sniffing
    /// the read bytes with [`content_detect::detect`].
    #[must_use]
    pub fn describe_sniffed(media: Option<&MediaType>, read: ReadBodyOutcome) -> Self {
        Self::describe_with(read, |bytes| {
            BodyParse::new(content_detect::detect(media, bytes), None)
        })
    }

    fn describe_with(read: ReadBodyOutcome, parse: impl FnOnce(&[u8]) -> BodyParse) -> Self {
        match read {
            ReadBodyOutcome::Complete(bytes) if bytes.is_empty() => Self::Empty,
            ReadBodyOutcome::Complete(bytes) => Self::parse(&parse(&bytes), &bytes),
            ReadBodyOutcome::Incomplete => Self::Excluded(ExclusionReason::Truncated),
            ReadBodyOutcome::TooLarge => Self::Excluded(ExclusionReason::TooLarge),
            ReadBodyOutcome::DecodeFailed => Self::Excluded(ExclusionReason::DecodeFailed),
            ReadBodyOutcome::Unavailable | ReadBodyOutcome::NotFound => {
                Self::Excluded(ExclusionReason::Unavailable)
            }
        }
    }

    fn parse(parse: &BodyParse, bytes: &[u8]) -> Self {
        match parse {
            BodyParse::Json => serde_json::from_slice::<Value>(bytes)
                .map_or(Self::Invalid, |value| {
                    Self::Fields(FieldShape::from_json(&value))
                }),
            BodyParse::MaybeJson => serde_json::from_slice::<Value>(bytes)
                .map_or(Self::Unstructured, |value| {
                    Self::Fields(FieldShape::from_json(&value))
                }),
            BodyParse::Form => std::str::from_utf8(bytes)
                .map_or(Self::Invalid, |text| Self::Fields(form_fields(text))),
            BodyParse::Multipart(boundary) => {
                parse_parts(bytes, boundary).map_or(Self::Invalid, Self::Parts)
            }
            BodyParse::Unstructured => Self::Unstructured,
        }
    }
}

fn form_fields(text: &str) -> FieldShape {
    let decode = |raw: &str| percent_decode_display(&raw.replace('+', " "));
    let pairs = text.split('&').filter(|pair| !pair.is_empty()).map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (decode(name), decode(value))
    });
    let owned: Vec<(String, String)> = pairs.collect();
    FieldShape::from_text_fields(
        owned
            .iter()
            .map(|(name, value)| (name.clone(), value.as_str())),
    )
}

/// Aggregate for one multipart part name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PartStats {
    seen: u64,
    files: u64,
    content_types: BTreeMap<String, u64>,
}

impl PartStats {
    /// Bodies that contained this part.
    #[must_use]
    pub const fn seen(&self) -> u64 {
        self.seen
    }

    /// Bodies in which this part carried a file.
    #[must_use]
    pub const fn files(&self) -> u64 {
        self.files
    }

    /// Declared part `Content-Type` essences with counts.
    #[must_use]
    pub const fn content_types(&self) -> &BTreeMap<String, u64> {
        &self.content_types
    }

    fn merge(&mut self, other: &Self) {
        self.seen = self.seen.saturating_add(other.seen);
        self.files = self.files.saturating_add(other.files);
        for (content_type, &count) in &other.content_types {
            bump(&mut self.content_types, content_type.clone(), count);
        }
    }
}

/// Aggregated structure of every body sharing a method, status and `Content-Type`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BodySchema {
    bodies: u64,
    empty: u64,
    invalid: u64,
    unstructured: u64,
    excluded: BTreeMap<ExclusionReason, u64>,
    fields: FieldShape,
    field_entries: usize,
    parts: BTreeMap<String, PartStats>,
    part_samples: u64,
    omitted_parts: u64,
}

impl BodySchema {
    /// Adds one described body.
    pub fn record(&mut self, analysis: &BodyAnalysis) {
        self.bodies = self.bodies.saturating_add(1);
        match analysis {
            BodyAnalysis::Empty => self.empty = self.empty.saturating_add(1),
            BodyAnalysis::Excluded(reason) => bump(&mut self.excluded, *reason, 1),
            BodyAnalysis::Invalid => self.invalid = self.invalid.saturating_add(1),
            BodyAnalysis::Unstructured => self.unstructured = self.unstructured.saturating_add(1),
            BodyAnalysis::Fields(shape) => self.merge_fields(shape),
            BodyAnalysis::Parts(parts) => self.record_parts(parts),
        }
    }

    /// Adds every body of `other`.
    pub fn merge(&mut self, other: &Self) {
        self.bodies = self.bodies.saturating_add(other.bodies);
        self.empty = self.empty.saturating_add(other.empty);
        self.invalid = self.invalid.saturating_add(other.invalid);
        self.unstructured = self.unstructured.saturating_add(other.unstructured);
        for (&reason, &count) in &other.excluded {
            bump(&mut self.excluded, reason, count);
        }
        self.merge_fields(&other.fields);
        self.part_samples = self.part_samples.saturating_add(other.part_samples);
        self.omitted_parts = self.omitted_parts.saturating_add(other.omitted_parts);
        for (name, stats) in &other.parts {
            self.merge_part(name, stats);
        }
    }

    /// Every body observed in this group.
    #[must_use]
    pub const fn bodies(&self) -> u64 {
        self.bodies
    }

    /// Bodies whose structure was aggregated (fields or parts).
    #[must_use]
    pub const fn structured_samples(&self) -> u64 {
        self.fields.seen().saturating_add(self.part_samples)
    }

    /// Bodies with no bytes.
    #[must_use]
    pub const fn empty(&self) -> u64 {
        self.empty
    }

    /// Bodies that did not parse as their declared format.
    #[must_use]
    pub const fn invalid(&self) -> u64 {
        self.invalid
    }

    /// Complete bodies of a type without field structure.
    #[must_use]
    pub const fn unstructured(&self) -> u64 {
        self.unstructured
    }

    /// Bodies left out of aggregation, by reason.
    #[must_use]
    pub const fn excluded(&self) -> &BTreeMap<ExclusionReason, u64> {
        &self.excluded
    }

    /// Total bodies left out of aggregation.
    #[must_use]
    pub fn excluded_total(&self) -> u64 {
        self.excluded
            .values()
            .fold(0, |sum, count| sum.saturating_add(*count))
    }

    /// Field structure of JSON and form bodies (root position).
    #[must_use]
    pub const fn fields(&self) -> &FieldShape {
        &self.fields
    }

    /// Multipart parts by name.
    #[must_use]
    pub const fn parts(&self) -> &BTreeMap<String, PartStats> {
        &self.parts
    }

    /// Multipart bodies whose parts were listed.
    #[must_use]
    pub const fn part_samples(&self) -> u64 {
        self.part_samples
    }

    /// Part names not kept because the group reached [`MAX_PARTS`].
    #[must_use]
    pub const fn omitted_parts(&self) -> u64 {
        self.omitted_parts
    }

    fn merge_fields(&mut self, shape: &FieldShape) {
        if shape.seen() == 0 {
            return;
        }
        let mut budget = MAX_SHAPE_FIELDS.saturating_sub(self.field_entries);
        self.fields.merge(shape, &mut budget);
        self.field_entries = self.fields.entry_count();
    }

    fn record_parts(&mut self, parts: &[PartObservation]) {
        self.part_samples = self.part_samples.saturating_add(1);
        let mut per_body: BTreeMap<&str, PartStats> = BTreeMap::new();
        for part in parts {
            let stats = per_body.entry(part.name.as_str()).or_default();
            stats.seen = 1;
            if part.is_file {
                stats.files = 1;
            }
            if let Some(content_type) = &part.content_type {
                let _ = stats.content_types.insert(content_type.clone(), 1);
            }
        }
        for (name, stats) in per_body {
            self.merge_part(name, &stats);
        }
    }

    fn merge_part(&mut self, name: &str, stats: &PartStats) {
        if let Some(existing) = self.parts.get_mut(name) {
            existing.merge(stats);
        } else if self.parts.len() < MAX_PARTS {
            let _ = self.parts.insert(name.to_owned(), stats.clone());
        } else {
            self.omitted_parts = self.omitted_parts.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(value: &str) -> Option<MediaType> {
        MediaType::parse(value)
    }

    #[test]
    fn parses_as_the_detected_format() {
        let parse = |value: &str, head: &[u8]| {
            let media = media(value);
            BodyParse::new(content_detect::detect(media.as_ref(), head), Some(value))
        };
        assert_eq!(parse("application/problem+json", b""), BodyParse::Json);
        assert_eq!(
            parse("application/x-www-form-urlencoded", b""),
            BodyParse::Form
        );
        assert_eq!(parse("multipart/form-data", b""), BodyParse::Unstructured);
        assert!(matches!(
            parse("multipart/form-data; boundary=B", b""),
            BodyParse::Multipart(_)
        ));
        assert_eq!(parse("text/plain", b"{}"), BodyParse::MaybeJson);
        assert_eq!(parse("text/plain", b"hello"), BodyParse::Unstructured);
        assert_eq!(
            BodyParse::new(content_detect::detect(None, b"[1]"), None),
            BodyParse::MaybeJson
        );
        assert_eq!(parse("text/html", b"{}"), BodyParse::Unstructured);
        assert!(!BodyParse::Unstructured.needs_bytes());
    }

    #[test]
    fn groups_count_samples_exclusions_and_invalid_bodies() {
        let mut group = BodySchema::default();
        let json = |text: &str| {
            BodyAnalysis::describe(&BodyParse::Json, ReadBodyOutcome::Complete(text.into()))
        };
        group.record(&json(r#"{"a":1}"#));
        group.record(&json(r#"{"a":null,"b":"x"}"#));
        group.record(&json("{not json"));
        group.record(&BodyAnalysis::describe(
            &BodyParse::Json,
            ReadBodyOutcome::Incomplete,
        ));
        group.record(&BodyAnalysis::describe(
            &BodyParse::Json,
            ReadBodyOutcome::Complete(Vec::new()),
        ));
        assert_eq!(group.bodies(), 5);
        assert_eq!(group.structured_samples(), 2);
        assert_eq!(group.invalid(), 1);
        assert_eq!(group.empty(), 1);
        assert_eq!(group.excluded_total(), 1);
        assert_eq!(group.excluded().get(&ExclusionReason::Truncated), Some(&1));
        assert_eq!(
            group
                .fields()
                .fields()
                .get("a")
                .map(|a| a.types().summary()),
            Some("null | integer".to_owned())
        );

        let mut merged = BodySchema::default();
        merged.merge(&group);
        merged.merge(&group);
        assert_eq!(merged.structured_samples(), 4);
        assert_eq!(merged.excluded_total(), 2);
    }

    #[test]
    fn form_bodies_keep_names_and_types_only() {
        let analysis = BodyAnalysis::describe(
            &BodyParse::Form,
            ReadBodyOutcome::Complete(b"user=SECRET_USER&remember=true&age=33&q=a+b".to_vec()),
        );
        let BodyAnalysis::Fields(shape) = &analysis else {
            unreachable!("form bodies describe fields");
        };
        assert_eq!(
            shape
                .fields()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["age", "q", "remember", "user"]
        );
        assert!(!format!("{analysis:?}").contains("SECRET_USER"));
    }

    #[test]
    fn multipart_groups_count_part_presence() {
        let Some(boundary) = Boundary::from_content_type("multipart/form-data; boundary=B") else {
            unreachable!("valid boundary");
        };
        let parse = BodyParse::Multipart(boundary);
        let body = b"--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nx\r\n--B--".to_vec();
        let mut group = BodySchema::default();
        group.record(&BodyAnalysis::describe(
            &parse,
            ReadBodyOutcome::Complete(body),
        ));
        let part = group.parts().get("file");
        assert_eq!(part.map(PartStats::files), Some(1));
        assert_eq!(group.part_samples(), 1);
        assert_eq!(group.structured_samples(), 1);
    }
}
