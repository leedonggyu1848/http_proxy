//! Image formats whose response bodies can be drawn as images.

/// How far into a body an SVG root element is searched for.
const SVG_SNIFF_BYTES: usize = 4 * 1024;

/// An image format the inspector can draw.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Svg,
    Bmp,
    Ico,
}

impl ImageFormat {
    /// Every format, in the order they are documented.
    pub const ALL: [Self; 7] = [
        Self::Png,
        Self::Jpeg,
        Self::Gif,
        Self::WebP,
        Self::Svg,
        Self::Bmp,
        Self::Ico,
    ];

    /// Names the format of a `Content-Type` value, ignoring case and
    /// media-type parameters, or `None` when it cannot be drawn.
    #[must_use]
    pub fn from_media_type(content_type: &str) -> Option<Self> {
        let essence = content_type.split(';').next()?.trim();
        Self::ALL.into_iter().find(|format| {
            format
                .media_types()
                .iter()
                .any(|media_type| essence.eq_ignore_ascii_case(media_type))
        })
    }

    /// Recognizes the format from the leading bytes of a decoded body.
    ///
    /// SVG has no binary signature; a body is taken as SVG when it starts
    /// with markup and has an `<svg` tag near the start.
    #[must_use]
    pub fn from_signature(bytes: &[u8]) -> Option<Self> {
        let format = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if bytes.starts_with(b"\xff\xd8\xff") {
            Self::Jpeg
        } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
            Self::Gif
        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
            Self::WebP
        } else if bytes.starts_with(b"BM") && bytes.len() >= 26 {
            Self::Bmp
        } else if bytes.starts_with(b"\x00\x00\x01\x00") {
            Self::Ico
        } else if looks_like_svg(bytes) {
            Self::Svg
        } else {
            return None;
        };
        Some(format)
    }

    /// Short display name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::WebP => "WebP",
            Self::Svg => "SVG",
            Self::Bmp => "BMP",
            Self::Ico => "ICO",
        }
    }

    /// `Content-Type` values that name this format.
    #[must_use]
    pub const fn media_types(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["image/png"],
            Self::Jpeg => &["image/jpeg"],
            Self::Gif => &["image/gif"],
            Self::WebP => &["image/webp"],
            Self::Svg => &["image/svg+xml"],
            Self::Bmp => &["image/bmp"],
            Self::Ico => &["image/x-icon", "image/vnd.microsoft.icon"],
        }
    }
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let start = bytes.iter().position(|byte| !byte.is_ascii_whitespace());
    let (head, _) = bytes.split_at(bytes.len().min(SVG_SNIFF_BYTES));
    start.is_some_and(|start| head.get(start) == Some(&b'<'))
        && head.windows(4).any(|window| window == b"<svg")
}

#[cfg(test)]
mod tests {
    use super::ImageFormat;

    #[test]
    fn names_formats_from_media_types() {
        for (value, format) in [
            ("image/png", ImageFormat::Png),
            (" IMAGE/JPEG ; q=1", ImageFormat::Jpeg),
            ("image/gif", ImageFormat::Gif),
            ("image/webp", ImageFormat::WebP),
            ("image/svg+xml; charset=utf-8", ImageFormat::Svg),
            ("image/bmp", ImageFormat::Bmp),
            ("image/x-icon", ImageFormat::Ico),
            ("image/vnd.microsoft.icon", ImageFormat::Ico),
        ] {
            assert_eq!(ImageFormat::from_media_type(value), Some(format), "{value}");
        }
        for value in ["image/avif", "image/svg", "text/plain; type=image/png", ""] {
            assert_eq!(ImageFormat::from_media_type(value), None, "{value}");
        }
    }

    #[test]
    fn recognizes_signatures() {
        for (bytes, format) in [
            (b"\x89PNG\r\n\x1a\n....".as_slice(), ImageFormat::Png),
            (b"\xff\xd8\xff\xe0", ImageFormat::Jpeg),
            (b"GIF89a\x01\x00", ImageFormat::Gif),
            (b"RIFF\x10\x00\x00\x00WEBPVP8L", ImageFormat::WebP),
            (b"BM00000000000000000000000000", ImageFormat::Bmp),
            (b"\x00\x00\x01\x00\x01\x00", ImageFormat::Ico),
            (
                b"\xef\xbb\xbf  <?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                ImageFormat::Svg,
            ),
        ] {
            assert_eq!(ImageFormat::from_signature(bytes), Some(format));
        }
        for bytes in [
            b"".as_slice(),
            b"RIFF\x10\x00\x00\x00WAVE",
            b"BM",
            b"plain <svg> text",
            b"\x00\x00\x02\x00",
        ] {
            assert_eq!(ImageFormat::from_signature(bytes), None, "{bytes:?}");
        }
    }
}
