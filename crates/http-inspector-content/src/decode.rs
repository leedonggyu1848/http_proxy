use std::io::{self, Read};

const PREVIEW_BYTES: usize = 8 * 1024;
const MAX_ENCODINGS: usize = 4;
const INTERMEDIATE_BYTES: u64 = 64 * 1024;

/// Reason a `Content-Encoding` body could not be decoded.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("Too many Content-Encoding layers to preview safely.")]
    TooManyLayers,
    #[error("Unsupported Content-Encoding: {encoding:?}. Raw bytes are retained in the full log.")]
    UnsupportedEncoding { encoding: String },
    #[error("Could not decompress the body: {source}")]
    Init {
        #[source]
        source: io::Error,
    },
    #[error("Could not decompress the body ({}): {source}", encodings.join(", "))]
    Decompress {
        encodings: Vec<String>,
        #[source]
        source: io::Error,
    },
    #[error("The decompressed body is larger than {} MiB.", limit / (1024 * 1024))]
    LimitExceeded { limit: usize },
}

#[derive(Debug)]
pub enum BodyText {
    Ready {
        text: String,
        notice: Option<String>,
    },
    Unavailable(String),
}

#[must_use]
pub fn decode(bytes: &[u8], encodings: &[&str], complete: bool) -> BodyText {
    let encodings = match content_encodings(encodings) {
        Ok(encodings) => encodings,
        Err(error) => return BodyText::Unavailable(error.to_string()),
    };
    if encodings.is_empty() {
        // PNG responses retain more than the text preview shows.
        let limited = bytes.len() > PREVIEW_BYTES;
        let (shown, _) = bytes.split_at(bytes.len().min(PREVIEW_BYTES));
        return BodyText::Ready {
            text: text(shown, limited || !complete),
            notice: limited.then(|| "first 8 KiB of retained bytes".to_owned()),
        };
    }
    match read_decoded(bytes, &encodings, PREVIEW_BYTES, INTERMEDIATE_BYTES) {
        Ok((mut decoded, error)) => {
            let limited = decoded.len() > PREVIEW_BYTES;
            decoded.truncate(PREVIEW_BYTES);
            if let Some(error) = error
                && (complete || decoded.is_empty())
            {
                return BodyText::Unavailable(format!(
                    "Could not decompress preview ({}): {error}. The captured data may be incomplete, invalid, or exceed preview limits.",
                    encodings.join(", ")
                ));
            }
            let detail = if limited {
                "first 8 KiB of decoded bytes"
            } else if complete {
                "complete decoded preview"
            } else {
                "decoded prefix; compressed capture is incomplete"
            };
            BodyText::Ready {
                text: text(&decoded, limited || !complete),
                notice: Some(format!(
                    "Content-Encoding: {} · {detail}",
                    encodings.join(", ")
                )),
            }
        }
        Err(error) => BodyText::Unavailable(format!("Could not decompress preview: {error}")),
    }
}

/// Decoded bytes of a retained body, bounded like the text preview.
#[derive(Debug, Eq, PartialEq)]
pub struct DecodedPrefix {
    pub bytes: Vec<u8>,
    /// More decoded bytes exist than the preview limit keeps.
    pub cut: bool,
}

/// Decodes at most the first 8 KiB of `bytes` under the same limits as the
/// text preview, so search sees what the preview shows.
///
/// A decoding error after some output keeps that output, as an incomplete capture would.
pub fn decode_prefix(bytes: &[u8], encodings: &[&str]) -> Result<DecodedPrefix, DecodeError> {
    let encodings = content_encodings(encodings)?;
    let mut decoded = if encodings.is_empty() {
        bytes.get(..=PREVIEW_BYTES).unwrap_or(bytes).to_vec()
    } else {
        let (decoded, error) = read_decoded(bytes, &encodings, PREVIEW_BYTES, INTERMEDIATE_BYTES)
            .map_err(|source| DecodeError::Init { source })?;
        match error {
            Some(source) if decoded.is_empty() => {
                return Err(DecodeError::Decompress { encodings, source });
            }
            _ => decoded,
        }
    };
    let cut = decoded.len() > PREVIEW_BYTES;
    decoded.truncate(PREVIEW_BYTES);
    Ok(DecodedPrefix {
        bytes: decoded,
        cut,
    })
}

/// Removes the `Content-Encoding` layers from a complete body and returns at
/// most `limit` decoded bytes. Unencoded bodies are returned as they are.
///
/// Fails when an encoding is unsupported, the data is invalid, or decoding
/// would produce more than `limit` bytes.
pub fn decode_complete(
    bytes: &[u8],
    encodings: &[&str],
    limit: usize,
) -> Result<Vec<u8>, DecodeError> {
    let encodings = content_encodings(encodings)?;
    if encodings.is_empty() {
        return Ok(bytes.to_vec());
    }
    // One byte past the limit on every layer, so an oversized body is
    // detected instead of silently cut.
    let intermediate = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
    let (decoded, error) = read_decoded(bytes, &encodings, limit, intermediate)
        .map_err(|source| DecodeError::Init { source })?;
    if let Some(source) = error {
        return Err(DecodeError::Decompress { encodings, source });
    }
    if decoded.len() > limit {
        return Err(DecodeError::LimitExceeded { limit });
    }
    Ok(decoded)
}

/// Splits `Content-Encoding` values into lowercase layers in the order they
/// were applied, without `identity`, and rejects layers it cannot decode.
fn content_encodings(values: &[&str]) -> Result<Vec<String>, DecodeError> {
    let encodings: Vec<_> = values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.eq_ignore_ascii_case("identity"))
        .take(MAX_ENCODINGS + 1)
        .map(str::to_ascii_lowercase)
        .collect();
    if encodings.len() > MAX_ENCODINGS {
        return Err(DecodeError::TooManyLayers);
    }
    if let Some(unsupported) = encodings.iter().find(|encoding| {
        !matches!(
            encoding.as_str(),
            "gzip" | "x-gzip" | "deflate" | "br" | "zstd"
        )
    }) {
        return Err(DecodeError::UnsupportedEncoding {
            encoding: unsupported.clone(),
        });
    }
    Ok(encodings)
}

/// Reads at most `limit + 1` decoded bytes so callers can tell whether the
/// output was cut, and stops each inner layer after `intermediate` bytes.
fn read_decoded(
    bytes: &[u8],
    encodings: &[String],
    limit: usize,
    intermediate: u64,
) -> io::Result<(Vec<u8>, Option<io::Error>)> {
    let mut reader: Box<dyn Read + '_> = Box::new(bytes);
    for encoding in encodings.iter().rev() {
        // Bound hidden expansion while inner decoders consume headers or skippable frames.
        reader = Box::new(decoder(reader, encoding)?.take(intermediate));
    }
    let mut decoded = Vec::new();
    let error = reader
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut decoded)
        .err();
    Ok((decoded, error))
}

fn decoder<'a>(reader: Box<dyn Read + 'a>, encoding: &str) -> io::Result<Box<dyn Read + 'a>> {
    match encoding {
        "gzip" | "x-gzip" => Ok(Box::new(flate2::read::MultiGzDecoder::new(reader))),
        "deflate" => Ok(Box::new(flate2::read::ZlibDecoder::new(reader))),
        "br" => Ok(Box::new(brotli_decompressor::Decompressor::new(
            reader, 4096,
        ))),
        "zstd" => {
            let mut decoder = zstd::stream::read::Decoder::new(reader)?;
            // HTTP zstd permits an 8 MiB window; do not accept arbitrary frame allocations.
            decoder.window_log_max(23)?;
            Ok(Box::new(decoder))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unsupported Content-Encoding",
        )),
    }
}

fn text(bytes: &[u8], prefix: bool) -> String {
    let bytes = if prefix {
        match std::str::from_utf8(bytes) {
            Err(error) if error.error_len().is_none() => bytes.split_at(error.valid_up_to()).0,
            _ => bytes,
        }
    } else {
        bytes
    };
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::Write;

    use super::{BodyText, PREVIEW_BYTES, decode, decode_complete};

    const EXPECTED: &[u8] = include_bytes!("../../tests/fixtures/body-preview.json");
    const BROTLI: &[u8] = include_bytes!("../../tests/fixtures/body-preview.json.br");
    const GZIP: &[u8] = include_bytes!("../../tests/fixtures/body-preview.json.gz");

    fn ready(result: BodyText) -> Result<(String, Option<String>), String> {
        match result {
            BodyText::Ready { text, notice } => Ok((text, notice)),
            BodyText::Unavailable(message) => Err(message),
        }
    }

    fn gzip(bytes: &[u8]) -> Result<Vec<u8>, std::io::Error> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes)?;
        encoder.finish()
    }

    #[test]
    fn displays_brotli_body_as_readable_json() -> Result<(), Box<dyn Error>> {
        let (text, _) = ready(decode(BROTLI, &["br"], true))?;
        assert_eq!(text.as_bytes(), EXPECTED);
        Ok(())
    }

    #[test]
    fn decodes_supported_browser_encodings() -> Result<(), Box<dyn Error>> {
        for (encoding, compressed) in [
            ("gzip", GZIP),
            (
                "deflate",
                include_bytes!("../../tests/fixtures/body-preview.json.deflate").as_slice(),
            ),
            (
                "zstd",
                include_bytes!("../../tests/fixtures/body-preview.json.zst").as_slice(),
            ),
        ] {
            let (text, notice) = ready(decode(compressed, &[encoding], true))?;
            assert_eq!(text.as_bytes(), EXPECTED, "{encoding}");
            assert!(notice.is_some_and(|value| value.contains("complete decoded preview")));
        }
        Ok(())
    }

    #[test]
    fn preserves_unencoded_text_and_reverses_encoding_layers() -> Result<(), Box<dyn Error>> {
        let (text, notice) = ready(decode(EXPECTED, &[], true))?;
        assert_eq!(text.as_bytes(), EXPECTED);
        assert!(notice.is_none());
        let nested = gzip(BROTLI)?;
        for encodings in [
            &[" BR, GZip "][..],
            &["br", "gzip"][..],
            &["identity, br", "gzip"][..],
        ] {
            let (text, _) = ready(decode(&nested, encodings, true))?;
            assert_eq!(text.as_bytes(), EXPECTED);
        }
        Ok(())
    }

    #[test]
    fn limits_expansion_and_preserves_utf8_boundaries() -> Result<(), Box<dyn Error>> {
        let original = "가".repeat(1_000_000);
        let compressed = gzip(original.as_bytes())?;
        assert!(compressed.len() < PREVIEW_BYTES);
        let (text, notice) = ready(decode(&compressed, &["gzip"], true))?;
        assert!(text.len() <= PREVIEW_BYTES);
        assert!(text.len() >= PREVIEW_BYTES - 3);
        assert!(original.starts_with(&text));
        assert!(notice.is_some_and(|value| value.contains("first 8 KiB")));
        Ok(())
    }

    #[test]
    fn limits_intermediate_expansion_in_stacked_encodings() -> Result<(), Box<dyn Error>> {
        let mut inner = 0x184D_2A50_u32.to_le_bytes().to_vec();
        inner.extend_from_slice(&1_000_000_u32.to_le_bytes());
        inner.resize(inner.len() + 1_000_000, 0);
        inner.extend_from_slice(include_bytes!("../../tests/fixtures/body-preview.json.zst"));
        let compressed = gzip(&inner)?;
        let result = decode(&compressed, &["zstd, gzip"], true);
        assert!(matches!(result, BodyText::Unavailable(_)), "{result:?}");
        Ok(())
    }

    #[test]
    fn rejects_zstd_windows_larger_than_http_limit() {
        let frame = [0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x88, 0x01, 0x00, 0x00];
        assert!(matches!(
            decode(&frame, &["zstd"], true),
            BodyText::Unavailable(_)
        ));
    }

    #[test]
    fn distinguishes_partial_capture_from_invalid_complete_data() -> Result<(), Box<dyn Error>> {
        let prefix = GZIP.split_at(GZIP.len() - 4).0;
        let (text, notice) = ready(decode(prefix, &["gzip"], false))?;
        assert_eq!(text.as_bytes(), EXPECTED);
        assert!(notice.is_some_and(|value| value.contains("incomplete")));
        assert!(matches!(
            decode(prefix, &["gzip"], true),
            BodyText::Unavailable(_)
        ));
        assert!(matches!(
            decode(b"invalid gzip", &["gzip"], true),
            BodyText::Unavailable(_)
        ));
        assert!(matches!(
            decode(EXPECTED, &["unknown"], true),
            BodyText::Unavailable(_)
        ));
        assert!(matches!(
            decode(GZIP, &["gzip,gzip,gzip,gzip,gzip"], true),
            BodyText::Unavailable(_)
        ));
        Ok(())
    }

    #[test]
    fn decodes_complete_bodies_within_a_limit() -> Result<(), Box<dyn Error>> {
        assert_eq!(decode_complete(GZIP, &["gzip"], EXPECTED.len())?, EXPECTED);
        assert_eq!(decode_complete(EXPECTED, &["identity"], 1)?, EXPECTED);
        assert!(matches!(
            decode_complete(GZIP, &["gzip"], EXPECTED.len() - 1),
            Err(super::DecodeError::LimitExceeded { limit }) if limit == EXPECTED.len() - 1
        ));
        assert!(matches!(
            decode_complete(GZIP.split_at(20).0, &["gzip"], 1 << 20),
            Err(super::DecodeError::Decompress { ref encodings, .. }) if encodings == &["gzip"]
        ));
        assert!(matches!(
            decode_complete(GZIP, &["compress"], 1 << 20),
            Err(super::DecodeError::UnsupportedEncoding { ref encoding }) if encoding == "compress"
        ));
        assert!(matches!(
            decode_complete(GZIP, &["gzip,gzip,gzip,gzip,gzip"], 1 << 20),
            Err(super::DecodeError::TooManyLayers)
        ));
        Ok(())
    }
}
