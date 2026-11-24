//! Part names and types of a `multipart/form-data` body.
//!
//! Only each part's name, whether it carries a file, and its declared
//! `Content-Type` are read. Part contents, including file names, are skipped.

use memchr::memmem;

/// Most parts described in one body.
pub const MAX_PARTS: usize = 64;
/// Longest part name kept, in bytes.
const MAX_PART_NAME_BYTES: usize = 64;
/// RFC 2046 limits a boundary to 70 characters.
const MAX_BOUNDARY_BYTES: usize = 70;

/// A validated `multipart` boundary parameter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Boundary(String);

impl Boundary {
    /// Extracts the `boundary` parameter from a raw `Content-Type` value.
    #[must_use]
    pub fn from_content_type(value: &str) -> Option<Self> {
        let raw = value.split(';').skip(1).find_map(|parameter| {
            let (name, value) = parameter.split_once('=')?;
            name.trim()
                .eq_ignore_ascii_case("boundary")
                .then(|| value.trim().trim_matches('"'))
        })?;
        let valid = !raw.is_empty()
            && raw.len() <= MAX_BOUNDARY_BYTES
            && raw
                .bytes()
                .all(|byte| byte.is_ascii_graphic() || byte == b' ');
        valid.then(|| Self(raw.to_owned()))
    }
}

/// One observed part of a multipart body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartObservation {
    /// Form field name from `Content-Disposition`.
    pub name: String,
    /// Declared part `Content-Type` essence, if any.
    pub content_type: Option<String>,
    /// Whether the part declared a `filename`.
    pub is_file: bool,
}

/// Lists the parts of a complete multipart body, or `None` when the body
/// does not contain the boundary.
#[must_use]
pub fn parse_parts(body: &[u8], boundary: &Boundary) -> Option<Vec<PartObservation>> {
    let delimiter = format!("--{}", boundary.0);
    let finder = memmem::Finder::new(delimiter.as_bytes());
    let positions: Vec<usize> = finder.find_iter(body).collect();
    if positions.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for window in positions.windows(2) {
        let [start, end] = window else {
            continue;
        };
        let Some(part) = body.get(start + delimiter.len()..*end) else {
            continue;
        };
        if parts.len() >= MAX_PARTS {
            break;
        }
        if let Some(observation) = parse_part(part) {
            parts.push(observation);
        }
    }
    Some(parts)
}

fn parse_part(part: &[u8]) -> Option<PartObservation> {
    let header_end = memmem::find(part, b"\r\n\r\n")?;
    let head = std::str::from_utf8(part.get(..header_end)?).ok()?;
    let mut name = None;
    let mut is_file = false;
    let mut content_type = None;
    for line in head.split("\r\n") {
        let Some((field, value)) = line.split_once(':') else {
            continue;
        };
        if field.trim().eq_ignore_ascii_case("content-disposition") {
            for parameter in value.split(';').skip(1) {
                let Some((key, raw)) = parameter.split_once('=') else {
                    continue;
                };
                match key.trim().to_ascii_lowercase().as_str() {
                    "name" => name = Some(bounded(raw.trim().trim_matches('"'))),
                    "filename" | "filename*" => is_file = true,
                    _ => {}
                }
            }
        } else if field.trim().eq_ignore_ascii_case("content-type") {
            content_type = http_inspector_exchange::MediaType::parse(value)
                .map(|media| media.essence().to_owned());
        }
    }
    Some(PartObservation {
        name: name?,
        content_type,
        is_file,
    })
}

fn bounded(name: &str) -> String {
    if name.len() <= MAX_PART_NAME_BYTES {
        return name.to_owned();
    }
    let cut = name.floor_char_boundary(MAX_PART_NAME_BYTES);
    format!("{}…", name.get(..cut).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_part_names_and_types_without_contents() {
        let boundary = Boundary::from_content_type("multipart/form-data; boundary=\"XyZ\"");
        assert_eq!(boundary, Some(Boundary("XyZ".to_owned())));
        let body = b"--XyZ\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nSECRET_TEXT\r\n--XyZ\r\nContent-Disposition: form-data; name=\"avatar\"; filename=\"me.png\"\r\nContent-Type: image/png\r\n\r\n\x89PNG\r\n--XyZ--\r\n";
        let parts = boundary.and_then(|boundary| parse_parts(body, &boundary));
        assert_eq!(
            parts,
            Some(vec![
                PartObservation {
                    name: "title".to_owned(),
                    content_type: None,
                    is_file: false,
                },
                PartObservation {
                    name: "avatar".to_owned(),
                    content_type: Some("image/png".to_owned()),
                    is_file: true,
                },
            ])
        );
        assert!(!format!("{parts:?}").contains("SECRET_TEXT"));
        assert!(!format!("{parts:?}").contains("me.png"));
    }

    #[test]
    fn rejects_missing_or_invalid_boundaries() {
        assert_eq!(Boundary::from_content_type("multipart/form-data"), None);
        assert_eq!(
            Boundary::from_content_type(&format!(
                "multipart/form-data; boundary={}",
                "a".repeat(71)
            )),
            None
        );
        let boundary = Boundary("b".to_owned());
        assert_eq!(parse_parts(b"no delimiter here", &boundary), None);
    }
}
