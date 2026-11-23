//! Choosing a safe file name for a saved body.
//!
//! Names come from untrusted headers and URLs, so every candidate is reduced
//! to one plain path component before it is used.

use crate::exchange::BodySide;
use crate::exchange::MediaType;

/// Longest file name, in bytes, before format suffixes are added. Common
/// file systems allow 255 bytes, which leaves room for the suffixes.
const MAX_NAME_BYTES: usize = 180;
/// Longest extension kept whole when a long name is shortened.
const MAX_EXTENSION_BYTES: usize = 16;
const PARTIAL_SUFFIX: &str = ".partial";

/// What a saved body file name is derived from.
#[derive(Debug)]
pub(super) struct NameSource<'a> {
    pub(super) record: u64,
    pub(super) side: BodySide,
    /// The absolute request target, used for response bodies.
    pub(super) target: &'a str,
    pub(super) disposition: Option<&'a str>,
    pub(super) media_type: Option<&'a MediaType>,
    /// `Content-Encoding` layers kept in the file, in the order they were applied.
    pub(super) kept_encodings: &'a [String],
    /// First and last byte of a partial response, added before the extension.
    pub(super) range: Option<(u64, u64)>,
    pub(super) partial: bool,
}

/// Chooses the file name for a saved body.
///
/// The order is `Content-Disposition` (`filename*`, then `filename`), the
/// last URL path segment for response bodies, then `response-<id>` or
/// `request-<id>`. The extension is corrected from `Content-Type`, kept
/// content encodings add their own suffix, and incomplete bodies end in
/// `.partial`.
pub(super) fn file_name(source: &NameSource<'_>) -> String {
    let from_url = || match source.side {
        BodySide::Response => url_file_name(source.target),
        BodySide::Request => None,
    };
    let mut name = source
        .disposition
        .and_then(disposition_file_name)
        .and_then(|name| sanitize(&name))
        .or_else(|| from_url().and_then(|name| sanitize(&name)))
        .unwrap_or_else(|| match source.side {
            BodySide::Response => format!("response-{}", source.record),
            BodySide::Request => format!("request-{}", source.record),
        });
    if let Some(media_type) = source.media_type {
        name = with_media_extension(name, media_type);
    }
    if let Some((start, end)) = source.range {
        name = with_range(&name, start, end);
    }
    for encoding in source.kept_encodings {
        if let Some(suffix) = encoding_suffix(encoding)
            && !name.ends_with(suffix)
        {
            name.push_str(suffix);
        }
    }
    if source.partial {
        name.push_str(PARTIAL_SUFFIX);
    }
    name
}

/// Inserts `.bytes-<start>-<end>` before the extension of `name`.
fn with_range(name: &str, start: u64, end: u64) -> String {
    let (stem, extension) = split_extension(name);
    let extension = extension.map_or_else(String::new, |extension| format!(".{extension}"));
    format!("{stem}.bytes-{start}-{end}{extension}")
}

/// Returns `name` with ` (n)` before its extension, keeping `.partial` last.
pub(super) fn numbered(name: &str, number: u32) -> String {
    let (base, partial) = name
        .strip_suffix(PARTIAL_SUFFIX)
        .map_or((name, ""), |base| (base, PARTIAL_SUFFIX));
    let (stem, extension) = split_extension(base);
    let extension = extension.map_or_else(String::new, |extension| format!(".{extension}"));
    format!("{stem} ({number}){extension}{partial}")
}

/// Most folders created below the save folder for one URL.
const MAX_DIRECTORY_DEPTH: usize = 16;

/// Returns safe folder names for a target: its host, then each folder of
/// its path, without the last segment. Each name is one plain component.
pub(super) fn url_directories(target: &str) -> Vec<String> {
    let Some((_, rest)) = target.split_once("://") else {
        return Vec::new();
    };
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit('@').next().unwrap_or_default();
    let folders = path.rsplit_once('/').map_or("", |(folders, _)| folders);
    std::iter::once(host.to_owned())
        .chain(
            folders
                .split('/')
                .map(|segment| String::from_utf8_lossy(&percent_decode(segment)).into_owned()),
        )
        .filter_map(|segment| sanitize(&segment))
        .take(MAX_DIRECTORY_DEPTH)
        .collect()
}

/// Reads `filename*` (RFC 8187) or else `filename` from a `Content-Disposition` value.
fn disposition_file_name(value: &str) -> Option<String> {
    let parameters = parameters(value);
    let extended = parameters
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("filename*"))
        .and_then(|(_, value)| extended_value(value));
    extended.or_else(|| {
        parameters
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("filename"))
            .map(|(_, value)| value.clone())
    })
}

/// Splits `disposition; name=value; name="quoted"` into unquoted parameters.
fn parameters(value: &str) -> Vec<(String, String)> {
    let mut parameters = Vec::new();
    let mut rest = value.split_once(';').map_or("", |(_, rest)| rest);
    while !rest.is_empty() {
        let Some((name, after_name)) = rest.split_once('=') else {
            break;
        };
        let after_name = after_name.trim_start();
        let (parameter, remaining) = after_name.strip_prefix('"').map_or_else(
            || {
                let (token, remaining) = after_name.split_once(';').unwrap_or((after_name, ""));
                (token.trim().to_owned(), remaining)
            },
            quoted_string,
        );
        parameters.push((name.trim().to_owned(), parameter));
        rest = remaining
            .trim_start()
            .strip_prefix(';')
            .unwrap_or(remaining);
    }
    parameters
}

/// Reads a quoted string after its opening quote and returns it with the
/// text left after the closing quote.
fn quoted_string(text: &str) -> (String, &str) {
    let mut value = String::new();
    let mut characters = text.char_indices();
    while let Some((index, character)) = characters.next() {
        match character {
            '"' => return (value, text.get(index + 1..).unwrap_or_default()),
            '\\' => {
                if let Some((_, escaped)) = characters.next() {
                    value.push(escaped);
                }
            }
            other => value.push(other),
        }
    }
    (value, "")
}

/// Decodes `charset'language'percent-encoded` from RFC 8187.
fn extended_value(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let charset = parts.next()?;
    let _language = parts.next()?;
    let bytes = percent_decode(parts.next()?);
    if charset.eq_ignore_ascii_case("utf-8") {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        Some(bytes.iter().copied().map(char::from).collect())
    } else {
        None
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let escaped = (byte == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(value) = escaped {
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    decoded
}

/// Returns the decoded last path segment of an absolute or origin-form target.
fn url_file_name(target: &str) -> Option<String> {
    let path_start = target.find("://").map_or(Some(0), |scheme_end| {
        let authority = scheme_end + 3;
        target
            .get(authority..)
            .and_then(|rest| rest.find('/'))
            .map(|slash| authority + slash)
    })?;
    let path = target.get(path_start..)?;
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let segment = path.rsplit('/').next().unwrap_or_default();
    (!segment.is_empty()).then(|| String::from_utf8_lossy(&percent_decode(segment)).into_owned())
}

/// Reduces an untrusted name to one plain file name, or `None` when nothing usable remains.
///
/// Directory parts, control characters and characters that other systems
/// reject are removed, leading and trailing dots and spaces are trimmed so
/// `.`, `..` and hidden names cannot result, and the name is shortened.
fn sanitize(name: &str) -> Option<String> {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .filter(|character| !character.is_control() && !is_bidi_control(*character))
        .map(|character| match character {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let trimmed =
        cleaned.trim_matches(|character: char| character == '.' || character.is_whitespace());
    (!trimmed.is_empty()).then(|| shorten(trimmed))
}

/// Reports Unicode bidirectional formatting characters, which can make a
/// name such as `photo\u{202E}gpj.exe` display with a fake extension.
const fn is_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

/// Shortens a name to [`MAX_NAME_BYTES`], keeping a short extension whole.
fn shorten(name: &str) -> String {
    if name.len() <= MAX_NAME_BYTES {
        return name.to_owned();
    }
    let (stem, extension) = split_extension(name);
    let extension = extension
        .filter(|extension| extension.len() <= MAX_EXTENSION_BYTES)
        .map_or_else(String::new, |extension| format!(".{extension}"));
    let keep = stem.floor_char_boundary(MAX_NAME_BYTES - extension.len());
    format!("{}{extension}", stem.get(..keep).unwrap_or_default())
}

/// Splits `name.ext` into `name` and `ext`; a name without a stem has no extension.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem, Some(extension))
        }
        _ => (name, None),
    }
}

/// Adds or replaces the extension when it does not fit the media type.
///
/// Generic types such as `text/plain` only add an extension to a name that
/// has none, so `data.json` sent as `text/plain` keeps its name.
fn with_media_extension(name: String, media_type: &MediaType) -> String {
    let Some((extensions, strict)) = media_extensions(media_type.essence()) else {
        return name;
    };
    let Some(&preferred) = extensions.first() else {
        return name;
    };
    let (stem, extension) = split_extension(&name);
    match extension {
        None => format!("{name}.{preferred}"),
        Some(extension)
            if extensions
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension)) =>
        {
            name
        }
        Some(_) if strict => format!("{stem}.{preferred}"),
        Some(_) => name,
    }
}

/// Returns the extensions for a media type, preferred first, and whether a
/// different extension should be replaced.
fn media_extensions(essence: &str) -> Option<(&'static [&'static str], bool)> {
    let strict: &'static [&'static str] = match essence {
        "video/mp2t" => &["ts"],
        "video/mp4" => &["mp4", "m4v"],
        "audio/mp4" => &["m4a", "mp4"],
        "video/iso.segment" => &["m4s"],
        "video/webm" | "audio/webm" => &["webm"],
        "video/ogg" => &["ogv", "ogg"],
        "audio/ogg" => &["ogg", "oga", "opus"],
        "audio/mpeg" => &["mp3"],
        "audio/aac" => &["aac"],
        "application/pdf" => &["pdf"],
        "application/zip" => &["zip"],
        "application/gzip" | "application/x-gzip" => &["gz", "tgz"],
        "application/x-tar" => &["tar"],
        "application/x-7z-compressed" => &["7z"],
        "application/vnd.rar" => &["rar"],
        "application/wasm" => &["wasm"],
        "application/vnd.apple.mpegurl" | "application/x-mpegurl" => &["m3u8"],
        "application/dash+xml" => &["mpd"],
        "image/png" => &["png"],
        "image/jpeg" => &["jpg", "jpeg"],
        "image/gif" => &["gif"],
        "image/webp" => &["webp"],
        "image/avif" => &["avif"],
        "image/svg+xml" => &["svg"],
        "image/bmp" => &["bmp"],
        "image/x-icon" | "image/vnd.microsoft.icon" => &["ico"],
        "font/woff" => &["woff"],
        "font/woff2" => &["woff2"],
        "font/ttf" => &["ttf"],
        "font/otf" => &["otf"],
        _ => return lenient_extensions(essence).map(|extensions| (extensions, false)),
    };
    Some((strict, true))
}

fn lenient_extensions(essence: &str) -> Option<&'static [&'static str]> {
    Some(match essence {
        "text/plain" => &["txt"],
        "text/html" => &["html", "htm"],
        "text/css" => &["css"],
        "text/csv" => &["csv"],
        "text/javascript" | "application/javascript" => &["js", "mjs"],
        "application/json" => &["json"],
        "application/xml" | "text/xml" => &["xml"],
        _ => return None,
    })
}

const fn encoding_suffix(encoding: &str) -> Option<&'static str> {
    match encoding.as_bytes() {
        b"gzip" | b"x-gzip" => Some(".gz"),
        b"br" => Some(".br"),
        b"zstd" => Some(".zst"),
        b"deflate" => Some(".zz"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source<'a>(target: &'a str, disposition: Option<&'a str>) -> NameSource<'a> {
        NameSource {
            record: 42,
            side: BodySide::Response,
            target,
            disposition,
            media_type: None,
            kept_encodings: &[],
            range: None,
            partial: false,
        }
    }

    fn named(target: &str, disposition: Option<&str>, media_type: Option<&str>) -> String {
        let media_type = media_type.and_then(MediaType::parse);
        file_name(&NameSource {
            media_type: media_type.as_ref(),
            ..source(target, disposition)
        })
    }

    #[test]
    fn prefers_extended_disposition_then_plain_then_url_then_fallback() {
        let target = "http://example.test/files/from-url.pdf?x=1#top";
        assert_eq!(
            named(
                target,
                Some("attachment; filename=\"plain.pdf\"; filename*=UTF-8''%E2%82%AC%20rates.pdf"),
                None
            ),
            "€ rates.pdf"
        );
        assert_eq!(
            named(
                target,
                Some("attachment; filename=\"a \\\"quoted\\\" name.pdf\""),
                None
            ),
            "a _quoted_ name.pdf"
        );
        assert_eq!(
            named(target, Some("attachment; filename=token.pdf"), None),
            "token.pdf"
        );
        assert_eq!(named(target, Some("inline"), None), "from-url.pdf");
        assert_eq!(
            named("http://example.test/a%20b.txt", None, None),
            "a b.txt"
        );
        assert_eq!(named("http://example.test/", None, None), "response-42");
        assert_eq!(named("http://example.test", None, None), "response-42");
        assert_eq!(named("/relative/only.bin", None, None), "only.bin");
        assert_eq!(
            named(
                target,
                Some("attachment; filename*=ISO-8859-1''caf%E9.txt"),
                None
            ),
            "café.txt"
        );
    }

    #[test]
    fn request_bodies_do_not_use_the_url() {
        let name = file_name(&NameSource {
            side: BodySide::Request,
            ..source("http://example.test/upload/form.php", None)
        });
        assert_eq!(name, "request-42");
    }

    #[test]
    fn malicious_names_stay_one_plain_component() {
        for (disposition, expected) in [
            ("attachment; filename=\"../../etc/passwd\"", "passwd"),
            ("attachment; filename=\"/absolute/path.txt\"", "path.txt"),
            (
                "attachment; filename=\"..\\\\..\\\\windows\\\\evil.dll\"",
                "evil.dll",
            ),
            (
                "attachment; filename*=UTF-8''..%2F..%2Fescape.sh",
                "escape.sh",
            ),
            ("attachment; filename*=UTF-8''a%00b%0Ac%1Bd.txt", "abcd.txt"),
            ("attachment; filename=\"..\"", "response-42"),
            ("attachment; filename=\".hidden\"", "hidden"),
            ("attachment; filename=\"   \"", "response-42"),
            ("attachment; filename=\"con:name?.txt\"", "con_name_.txt"),
            (
                "attachment; filename*=UTF-8''photo%E2%80%AEgpj.exe",
                "photogpj.exe",
            ),
            (
                "attachment; filename*=UTF-8''a%E2%81%A6b%E2%81%A9c%E2%80%8Fd%D8%9Ce.txt",
                "abcde.txt",
            ),
        ] {
            let name = named("http://example.test/", Some(disposition), None);
            assert_eq!(name, expected, "{disposition}");
            assert!(!name.contains('/') && !name.contains('\\'));
        }
    }

    #[test]
    fn long_names_are_shortened_and_keep_the_extension() {
        let long = format!("attachment; filename=\"{}.mp4\"", "é".repeat(400));
        let name = named("http://example.test/", Some(&long), None);
        assert!(name.len() <= MAX_NAME_BYTES, "{}", name.len());
        assert_eq!(split_extension(&name).1, Some("mp4"));
        let no_extension = format!("attachment; filename=\"{}\"", "x".repeat(1_000));
        assert_eq!(
            named("http://example.test/", Some(&no_extension), None).len(),
            MAX_NAME_BYTES
        );
    }

    #[test]
    fn corrects_extensions_from_the_media_type() {
        let url = |path: &str| format!("http://example.test/{path}");
        assert_eq!(
            named(&url("segment"), None, Some("video/mp2t")),
            "segment.ts"
        );
        assert_eq!(named(&url("seg.php"), None, Some("video/MP2T")), "seg.ts");
        assert_eq!(
            named(&url("doc.PDF"), None, Some("application/pdf")),
            "doc.PDF"
        );
        assert_eq!(
            named(&url("photo.jpeg"), None, Some("image/jpeg")),
            "photo.jpeg"
        );
        assert_eq!(
            named(&url("data.json"), None, Some("text/plain")),
            "data.json"
        );
        assert_eq!(
            named(&url("readme"), None, Some("text/plain")),
            "readme.txt"
        );
        assert_eq!(
            named(&url(""), None, Some("application/json")),
            "response-42.json"
        );
        assert_eq!(
            named(&url("blob"), None, Some("application/octet-stream")),
            "blob"
        );
    }

    #[test]
    fn kept_encodings_and_partial_bodies_add_suffixes() {
        let media_type = MediaType::parse("application/json");
        let encodings = ["br".to_owned(), "gzip".to_owned()];
        let name = file_name(&NameSource {
            media_type: media_type.as_ref(),
            kept_encodings: &encodings,
            partial: true,
            ..source("http://example.test/data", None)
        });
        assert_eq!(name, "data.json.br.gz.partial");
        let already = ["gzip".to_owned()];
        let name = file_name(&NameSource {
            kept_encodings: &already,
            ..source("http://example.test/archive.gz", None)
        });
        assert_eq!(name, "archive.gz");
    }

    #[test]
    fn url_directories_are_safe_components() {
        assert_eq!(
            url_directories("https://user@cdn.example.test:8443/v/a%20b/seg-1.ts?x=/y"),
            ["cdn.example.test_8443", "v", "a b"]
        );
        assert_eq!(
            url_directories("http://example.test/../..%2F../etc/passwd"),
            ["example.test", "etc"]
        );
        assert_eq!(
            url_directories("http://example.test/file"),
            ["example.test"]
        );
        assert_eq!(url_directories("/origin/only"), Vec::<String>::new());
        let deep = format!("http://h/{}leaf", "d/".repeat(40));
        assert_eq!(url_directories(&deep).len(), MAX_DIRECTORY_DEPTH);
    }

    #[test]
    fn partial_responses_name_their_byte_range() {
        let media_type = MediaType::parse("video/mp4");
        let name = file_name(&NameSource {
            media_type: media_type.as_ref(),
            range: Some((0, 1_048_575)),
            partial: true,
            ..source("http://example.test/movie.mp4", None)
        });
        assert_eq!(name, "movie.bytes-0-1048575.mp4.partial");
    }

    #[test]
    fn numbers_names_before_the_extension() {
        assert_eq!(numbered("clip.ts", 1), "clip (1).ts");
        assert_eq!(numbered("clip.ts.partial", 2), "clip (2).ts.partial");
        assert_eq!(numbered("response-1", 3), "response-1 (3)");
        assert_eq!(numbered("data.json.gz", 1), "data.json (1).gz");
    }
}
