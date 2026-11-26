//! The image file an image body holds, and the facts the body view states
//! about it: its format, pixel size, and why it cannot be drawn yet.

use std::fmt::Write as _;

use http_inspector_exchange::capture::{IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES};
use http_inspector_exchange::{BodyLifecycle, BodySnapshot, ImageFormat};

use crate::decode::{self, DecodeError};
use crate::image::PreviewImage;

/// The image file a body holds once `Content-Encoding` is removed.
#[derive(Debug)]
pub struct ImageFile {
    /// Decoded file bytes.
    pub bytes: Vec<u8>,
    /// The format named by the leading bytes, or by `Content-Type` when the
    /// bytes have no known signature.
    pub format: ImageFormat,
}

impl ImageFile {
    /// Removes `Content-Encoding` and recognizes the format.
    pub fn decode_body(
        bytes: &[u8],
        encodings: &[&str],
        declared: ImageFormat,
    ) -> Result<Self, DecodeError> {
        let bytes = decode::decode_complete(bytes, encodings, IMAGE_PREVIEW_BYTES)?;
        let format = ImageFormat::from_signature(&bytes).unwrap_or(declared);
        Ok(Self { bytes, format })
    }
}

/// Explains that the body is drawn as another format than `Content-Type`
/// names, or `None` when they agree.
#[must_use]
pub fn mismatch_text(declared: ImageFormat, format: ImageFormat) -> Option<String> {
    (declared != format).then(|| {
        format!(
            "Content-Type names {}, but the body starts like {}; it is drawn as {}.",
            declared.name(),
            format.name(),
            format.name()
        )
    })
}

/// Describes the format, pixel size, file size, and notes of a decoded image.
#[must_use]
pub fn image_text(format: ImageFormat, image: &PreviewImage, file_bytes: usize) -> String {
    let mut text = format!(
        "{} · {}×{} px · {file_bytes} bytes",
        format.name(),
        image.width,
        image.height
    );
    for note in &image.notes {
        let _ = write!(text, " · {note}");
    }
    text
}

/// Notes the number of bytes received when `Content-Encoding` changed it.
#[must_use]
pub fn received_text(file_bytes: usize, received_bytes: u64) -> String {
    if u64::try_from(file_bytes).unwrap_or(u64::MAX) == received_bytes {
        String::new()
    } else {
        format!(" · {received_bytes} bytes received with Content-Encoding")
    }
}

/// Explains why a body cannot be drawn yet, or `None` when it can be decoded.
#[must_use]
pub fn unavailable_reason(snapshot: &BodySnapshot) -> Option<String> {
    if snapshot.lifecycle() == BodyLifecycle::Streaming {
        return Some(format!(
            "The image is drawn when the body finishes ({} bytes so far).",
            snapshot.observed_bytes()
        ));
    }
    if matches!(
        snapshot.lifecycle(),
        BodyLifecycle::Interrupted | BodyLifecycle::Cancelled
    ) {
        return Some("The image body ended early. Use Hex to inspect the kept bytes.".to_owned());
    }
    (!snapshot.is_complete()).then(|| {
        format!(
            "The image cannot be drawn because only {} of {} bytes were kept. Image previews keep up to {} MiB per response and {} MiB across all responses; use Hex to inspect the kept bytes.",
            snapshot.bytes().len(),
            snapshot.observed_bytes(),
            IMAGE_PREVIEW_BYTES / (1024 * 1024),
            IMAGE_PREVIEW_BUDGET_BYTES / (1024 * 1024),
        )
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::Write;

    use http_inspector_exchange::ImageFormat;

    use super::{ImageFile, mismatch_text};

    #[test]
    fn removes_content_encoding_and_prefers_the_signature() -> Result<(), Box<dyn Error>> {
        let gif = include_bytes!("../../../tests/fixtures/images/animated.gif");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(gif)?;
        let file = ImageFile::decode_body(&encoder.finish()?, &["gzip"], ImageFormat::Png)?;
        assert_eq!(file.bytes, gif);
        assert_eq!(file.format, ImageFormat::Gif);
        assert!(mismatch_text(ImageFormat::Png, file.format).is_some());

        // Bytes without a known signature keep the declared format.
        let file = ImageFile::decode_body(b"broken", &[], ImageFormat::WebP)?;
        assert_eq!(file.format, ImageFormat::WebP);
        assert_eq!(mismatch_text(ImageFormat::WebP, file.format), None);
        assert!(ImageFile::decode_body(b"\x1f\x8b broken", &["gzip"], ImageFormat::Png).is_err());
        Ok(())
    }

    #[test]
    fn an_early_end_keeps_the_image_preview_unavailable() -> Result<(), Box<dyn Error>> {
        let body = http_inspector_exchange::BodyPreviewWriter::new(32);
        body.observe(b"image prefix");
        body.interrupt();
        let snapshot = body.preview().snapshot().ok_or("missing snapshot")?;
        assert_eq!(
            super::unavailable_reason(&snapshot).as_deref(),
            Some("The image body ended early. Use Hex to inspect the kept bytes.")
        );
        Ok(())
    }
}
