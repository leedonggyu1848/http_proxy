//! Decodes image files to RGBA pixels within fixed size limits.
//!
//! Every decoder receives the image file with `Content-Encoding` already
//! removed, reads the image size from the header, and rejects oversized
//! images before allocating pixel memory. Animated formats draw only their
//! first frame.

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::num::NonZeroU64;

use crate::ImageFormat;
use resvg::{tiny_skia, usvg};
use usvg::roxmltree;

/// Widest or tallest image the decoder accepts.
pub const MAX_IMAGE_SIDE: u32 = 8_192;
/// Most pixels the decoder accepts, which bounds the RGBA buffer to 64 MiB.
const MAX_IMAGE_PIXELS: u64 = 4_096 * 4_096;
/// Allocation ceiling handed to decoders: the RGBA frame plus working buffers.
const DECODER_MEMORY_BYTES: usize = 80 * 1024 * 1024;
/// Frames counted in a GIF before the count is reported as a lower bound.
const MAX_COUNTED_FRAMES: u32 = 1_000;
/// Longest side an SVG is rasterized at; larger drawings are scaled down.
const MAX_SVG_RASTER_SIDE: f32 = 2_048.0;
/// Most SVG elements, counting each `<use>` copy of its target again.
const MAX_SVG_ELEMENTS: usize = 50_000;
/// Most XML nodes, including text and comments, an SVG source may have.
const MAX_SVG_XML_NODES: u32 = 200_000;
/// Deepest nesting of SVG elements and `<use>` references.
const MAX_SVG_DEPTH: usize = 256;
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// A decoded image frame as unmultiplied 8-bit RGBA pixels.
#[derive(Debug)]
pub struct PreviewImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Facts about the file the pixels alone do not show, such as the number
    /// of animation frames.
    pub notes: Vec<String>,
}

/// Why an image file could not be drawn.
#[derive(Debug, thiserror::Error)]
pub enum ImageError {
    #[error(
        "The image is {width}×{height} px, larger than the {MAX_IMAGE_SIDE} px per side or 16 megapixel preview limit."
    )]
    TooLarge { width: u32, height: u32 },
    #[error(
        "The SVG has more than {MAX_SVG_ELEMENTS} elements or {MAX_SVG_DEPTH} nesting levels after expanding <use> references, or a <use> reference cycle."
    )]
    TooComplex,
    #[error("The body is not a valid {format} image: {message}", format = .0.name(), message = .1)]
    Invalid(ImageFormat, String),
}

/// Decodes an image file of `format` to its first frame.
pub fn decode(format: ImageFormat, bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    match format {
        ImageFormat::Png => decode_png(bytes),
        ImageFormat::Jpeg => decode_jpeg(bytes),
        ImageFormat::Gif => decode_gif(bytes),
        ImageFormat::WebP => decode_webp(bytes),
        ImageFormat::Svg => decode_svg(bytes),
        ImageFormat::Bmp => decode_bmp(bytes),
        ImageFormat::Ico => decode_ico(bytes),
    }
}

fn check_size(width: u32, height: u32) -> Result<(), ImageError> {
    if width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(ImageError::TooLarge { width, height });
    }
    Ok(())
}

fn to_usize(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    rgb.as_chunks::<3>()
        .0
        .iter()
        .flat_map(|&[r, g, b]| [r, g, b, 255])
        .collect()
}

fn decode_png(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let invalid =
        |error: png::DecodingError| ImageError::Invalid(ImageFormat::Png, error.to_string());
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(bytes),
        png::Limits {
            bytes: DECODER_MEMORY_BYTES,
        },
    );
    let (width, height) = decoder.read_header_info().map_err(invalid)?.size();
    check_size(width, height)?;
    // ALPHA expands palettes and low bit depths and always adds an alpha
    // channel, so the output is 8-bit gray+alpha or RGBA.
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(invalid)?;
    let size = reader
        .output_buffer_size()
        .ok_or(ImageError::TooLarge { width, height })?;
    let mut buffer = vec![0; size];
    let frame = reader.next_frame(&mut buffer).map_err(invalid)?;
    buffer.truncate(frame.buffer_size());
    let rgba = match frame.color_type {
        png::ColorType::Rgba => buffer,
        png::ColorType::GrayscaleAlpha => buffer
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[gray, alpha]| [gray, gray, gray, alpha])
            .collect(),
        other => {
            return Err(ImageError::Invalid(
                ImageFormat::Png,
                format!("unexpected decoded color type {other:?}"),
            ));
        }
    };
    Ok(PreviewImage {
        width,
        height,
        rgba,
        notes: Vec::new(),
    })
}

fn decode_jpeg(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let invalid =
        |error: jpeg_decoder::Error| ImageError::Invalid(ImageFormat::Jpeg, error.to_string());
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.set_max_decoding_buffer_size(DECODER_MEMORY_BYTES);
    decoder.read_info().map_err(invalid)?;
    let info = decoder
        .info()
        .ok_or_else(|| ImageError::Invalid(ImageFormat::Jpeg, "missing JPEG header".into()))?;
    let (width, height) = (u32::from(info.width), u32::from(info.height));
    check_size(width, height)?;
    let pixels = decoder.decode().map_err(invalid)?;
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => rgb_to_rgba(&pixels),
        jpeg_decoder::PixelFormat::L8 => pixels
            .iter()
            .flat_map(|&gray| [gray, gray, gray, 255])
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[c, m, y, k]| {
                let channel = |ink: u8| {
                    u8::try_from((255 - u16::from(ink)) * (255 - u16::from(k)) / 255).unwrap_or(0)
                };
                [channel(c), channel(m), channel(y), 255]
            })
            .collect(),
        other @ jpeg_decoder::PixelFormat::L16 => {
            return Err(ImageError::Invalid(
                ImageFormat::Jpeg,
                format!("unsupported JPEG pixel format {other:?}"),
            ));
        }
    };
    Ok(PreviewImage {
        width,
        height,
        rgba,
        notes: Vec::new(),
    })
}

fn gif_options() -> gif::DecodeOptions {
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    if let Some(limit) = NonZeroU64::new(DECODER_MEMORY_BYTES as u64) {
        options.set_memory_limit(gif::MemoryLimit::Bytes(limit));
    }
    options
}

/// Draws the first GIF frame onto a transparent canvas of the logical screen
/// size and counts the frames without decoding the rest.
fn decode_gif(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let invalid =
        |error: gif::DecodingError| ImageError::Invalid(ImageFormat::Gif, error.to_string());
    let mut decoder = gif_options()
        .read_info(Cursor::new(bytes))
        .map_err(invalid)?;
    let (width, height) = (u32::from(decoder.width()), u32::from(decoder.height()));
    check_size(width, height)?;
    let mut rgba = vec![0; to_usize(width) * to_usize(height) * 4];
    let frame = decoder
        .read_next_frame()
        .map_err(invalid)?
        .ok_or_else(|| ImageError::Invalid(ImageFormat::Gif, "the file has no frames".into()))?;
    place_gif_frame(&mut rgba, to_usize(width), frame);
    let mut notes = Vec::new();
    if let Some(frames) = count_gif_frames(bytes) {
        notes.push(frames);
    }
    Ok(PreviewImage {
        width,
        height,
        rgba,
        notes,
    })
}

/// Copies an RGBA frame into the canvas at its offset, clipping what lies
/// outside the logical screen.
fn place_gif_frame(canvas: &mut [u8], canvas_width: usize, frame: &gif::Frame<'_>) {
    let (left, top) = (usize::from(frame.left), usize::from(frame.top));
    let frame_width = usize::from(frame.width);
    if frame_width == 0 || left >= canvas_width {
        return;
    }
    let copied = frame_width.min(canvas_width - left) * 4;
    for (row, line) in frame.buffer.chunks_exact(frame_width * 4).enumerate() {
        let start = ((top + row) * canvas_width + left) * 4;
        let (Some(target), Some(source)) =
            (canvas.get_mut(start..start + copied), line.get(..copied))
        else {
            break;
        };
        target.copy_from_slice(source);
    }
}

/// Describes the frame count of an animated GIF, or `None` for one frame.
/// Frames are read without LZW decoding and counting stops at a limit.
fn count_gif_frames(bytes: &[u8]) -> Option<String> {
    let mut options = gif_options();
    options.skip_frame_decoding(true);
    let mut decoder = options.read_info(Cursor::new(bytes)).ok()?;
    let mut frames = 0;
    while frames < MAX_COUNTED_FRAMES + 1 {
        match decoder.read_next_frame() {
            Ok(Some(_)) => frames += 1,
            Ok(None) | Err(_) => break,
        }
    }
    animation_note(frames)
}

fn animation_note(frames: u32) -> Option<String> {
    match frames {
        0 | 1 => None,
        frames if frames > MAX_COUNTED_FRAMES => Some(format!(
            "more than {MAX_COUNTED_FRAMES} frames, first frame shown"
        )),
        frames => Some(format!("{frames} frames, first frame shown")),
    }
}

fn decode_webp(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let invalid = |error: image_webp::DecodingError| {
        ImageError::Invalid(ImageFormat::WebP, error.to_string())
    };
    let mut decoder = image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(invalid)?;
    decoder.set_memory_limit(DECODER_MEMORY_BYTES);
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    let size = decoder
        .output_buffer_size()
        .ok_or(ImageError::TooLarge { width, height })?;
    let mut buffer = vec![0; size];
    // For an animation this composes only the first frame.
    decoder.read_image(&mut buffer).map_err(invalid)?;
    let rgba = if decoder.has_alpha() {
        buffer
    } else {
        rgb_to_rgba(&buffer)
    };
    let notes = decoder
        .is_animated()
        .then(|| animation_note(decoder.num_frames()))
        .flatten()
        .into_iter()
        .collect();
    Ok(PreviewImage {
        width,
        height,
        rgba,
        notes,
    })
}

/// Decodes through the `image` crate after checking the size and limiting
/// allocations.
fn decode_with_image(
    format: ImageFormat,
    mut decoder: impl image::ImageDecoder,
    notes: Vec<String>,
) -> Result<PreviewImage, ImageError> {
    let invalid = |error: image::ImageError| ImageError::Invalid(format, error.to_string());
    let (width, height) = decoder.dimensions();
    check_size(width, height)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(DECODER_MEMORY_BYTES as u64);
    decoder.set_limits(limits).map_err(invalid)?;
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(invalid)?
        .into_rgba8()
        .into_raw();
    Ok(PreviewImage {
        width,
        height,
        rgba,
        notes,
    })
}

fn decode_bmp(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let decoder = image::codecs::bmp::BmpDecoder::new(Cursor::new(bytes))
        .map_err(|error| ImageError::Invalid(ImageFormat::Bmp, error.to_string()))?;
    decode_with_image(ImageFormat::Bmp, decoder, Vec::new())
}

/// Draws the largest image of an icon file.
fn decode_ico(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let decoder = image::codecs::ico::IcoDecoder::new(Cursor::new(bytes))
        .map_err(|error| ImageError::Invalid(ImageFormat::Ico, error.to_string()))?;
    let count = bytes
        .get(4..6)
        .and_then(|count| count.try_into().ok())
        .map_or(0, u16::from_le_bytes);
    let notes = (count > 1)
        .then(|| format!("{count} images in the file, largest shown"))
        .into_iter()
        .collect();
    decode_with_image(ImageFormat::Ico, decoder, notes)
}

/// Rasterizes an SVG without scripts, external resources, or fonts.
///
/// The source must be UTF-8. Element count and nesting are checked before the
/// drawing is converted, `<image>` references are never loaded, and the
/// raster is at most [`MAX_SVG_RASTER_SIDE`] px per side.
fn decode_svg(bytes: &[u8]) -> Result<PreviewImage, ImageError> {
    let invalid = |message: String| ImageError::Invalid(ImageFormat::Svg, message);
    let text =
        std::str::from_utf8(bytes).map_err(|error| invalid(format!("not UTF-8: {error}")))?;
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        nodes_limit: MAX_SVG_XML_NODES,
        ..roxmltree::ParsingOptions::default()
    };
    let document = roxmltree::Document::parse_with_options(text, options)
        .map_err(|error| invalid(error.to_string()))?;
    check_svg_complexity(&document)?;
    let tree = usvg::Tree::from_xmltree(&document, &svg_options())
        .map_err(|error| invalid(error.to_string()))?;
    let size = tree.size();
    let scale = (MAX_SVG_RASTER_SIDE / size.width().max(size.height())).min(1.0);
    let (width, height) = (
        raster_side(size.width() * scale),
        raster_side(size.height() * scale),
    );
    let mut pixmap = tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| invalid(format!("cannot draw at {width}×{height} px")))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.red(), color.green(), color.blue(), color.alpha()]
        })
        .collect();
    Ok(PreviewImage {
        width,
        height,
        rgba,
        notes: svg_notes(&document, size, scale),
    })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 1..=MAX_SVG_RASTER_SIDE first"
)]
const fn raster_side(side: f32) -> u32 {
    side.ceil().clamp(1.0, MAX_SVG_RASTER_SIDE) as u32
}

/// Parsing options that load nothing outside the SVG source.
///
/// Both image resolvers return `None`, so `<image>` with a file path, URL, or
/// data URL draws nothing; the default resolver would read local files.
/// Without the `text` feature no fonts are loaded and text is not drawn.
fn svg_options() -> usvg::Options<'static> {
    usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        style_sheet: None,
        ..usvg::Options::default()
    }
}

fn svg_notes(document: &roxmltree::Document<'_>, size: usvg::Size, scale: f32) -> Vec<String> {
    let mut notes = Vec::new();
    if scale < 1.0 {
        notes.push(format!(
            "drawn scaled down from {:.0}×{:.0} px",
            size.width(),
            size.height()
        ));
    }
    let has = |name: &str| {
        document
            .descendants()
            .any(|node| node.is_element() && node.tag_name().name() == name)
    };
    if has("text") {
        notes.push("text is not drawn; use Source to read it".to_owned());
    }
    if has("image") || has("foreignObject") {
        notes.push("embedded images and foreign content are not loaded".to_owned());
    }
    notes
}

/// Rejects an SVG whose elements, with every `<use>` counted as a copy of
/// its target, exceed [`MAX_SVG_ELEMENTS`] or nest deeper than
/// [`MAX_SVG_DEPTH`]. This bounds the work of the converter, which expands
/// `<use>` references, before it runs.
fn check_svg_complexity(document: &roxmltree::Document<'_>) -> Result<(), ImageError> {
    let ids = document
        .descendants()
        .filter_map(|node| Some((node.attribute("id")?, node)))
        .collect::<HashMap<_, _>>();
    let mut counter = SvgCounter {
        ids,
        counted: HashMap::new(),
        visiting: HashSet::new(),
    };
    let elements = counter.count(document.root_element(), 0)?;
    if elements > MAX_SVG_ELEMENTS {
        return Err(ImageError::TooComplex);
    }
    Ok(())
}

struct SvgCounter<'a, 'input> {
    ids: HashMap<&'a str, roxmltree::Node<'a, 'input>>,
    counted: HashMap<roxmltree::NodeId, usize>,
    visiting: HashSet<roxmltree::NodeId>,
}

impl<'a, 'input> SvgCounter<'a, 'input> {
    /// Counts the elements `node` draws, including the targets of `<use>`.
    /// A reference cycle is rejected: the converter would expand it until its
    /// own limit of a million nodes.
    fn count(
        &mut self,
        node: roxmltree::Node<'a, 'input>,
        depth: usize,
    ) -> Result<usize, ImageError> {
        if depth > MAX_SVG_DEPTH {
            return Err(ImageError::TooComplex);
        }
        if let Some(&count) = self.counted.get(&node.id()) {
            return Ok(count);
        }
        if !self.visiting.insert(node.id()) {
            return Err(ImageError::TooComplex);
        }
        let mut total: usize = 1;
        for child in node.children().filter(roxmltree::Node::is_element) {
            total = total.saturating_add(self.count(child, depth + 1)?);
            if total > MAX_SVG_ELEMENTS {
                return Err(ImageError::TooComplex);
            }
        }
        if node.tag_name().name() == "use"
            && let Some(target) = node
                .attribute((XLINK_NS, "href"))
                .or_else(|| node.attribute("href"))
                .and_then(|href| href.strip_prefix('#'))
                .and_then(|id| self.ids.get(id).copied())
        {
            total = total.saturating_add(self.count(target, depth + 1)?);
        }
        self.visiting.remove(&node.id());
        if total > MAX_SVG_ELEMENTS {
            return Err(ImageError::TooComplex);
        }
        self.counted.insert(node.id(), total);
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt::Write as _;
    use std::io::ErrorKind;
    use std::net::TcpListener;

    use crate::ImageFormat;

    use super::{ImageError, PreviewImage, decode};

    const RED: [u8; 4] = [240, 20, 10, 255];
    const BLUE: [u8; 4] = [10, 20, 240, 255];
    /// `RED` after the `FFmpeg` color source passed it through YUV.
    const FIXTURE_RED: [u8; 4] = [238, 19, 9, 255];

    fn png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let written = encoder
            .write_header()
            .and_then(|mut writer| writer.write_image_data(pixels));
        assert!(written.is_ok(), "{written:?}");
        bytes
    }

    fn assert_solid(image: &PreviewImage, size: (u32, u32), color: [u8; 4], tolerance: u8) {
        assert_eq!((image.width, image.height), size);
        assert_eq!(image.rgba.len(), (size.0 * size.1 * 4) as usize);
        for pixel in image.rgba.as_chunks::<4>().0 {
            assert!(
                pixel
                    .iter()
                    .zip(color)
                    .all(|(&actual, wanted)| actual.abs_diff(wanted) <= tolerance),
                "{pixel:?} is not {color:?}"
            );
        }
    }

    fn assert_invalid(format: ImageFormat, bytes: &[u8]) {
        assert!(
            matches!(decode(format, bytes), Err(ImageError::Invalid(failed, _)) if failed == format),
            "{format:?} accepted {} invalid bytes",
            bytes.len()
        );
    }

    #[test]
    fn decodes_png_color_types_to_rgba() -> Result<(), Box<dyn Error>> {
        let rgb = png(2, 1, png::ColorType::Rgb, &[255, 0, 0, 0, 0, 255]);
        let image = decode(ImageFormat::Png, &rgb)?;
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [255, 0, 0, 255, 0, 0, 255, 255]);

        let gray = png(1, 2, png::ColorType::Grayscale, &[7, 9]);
        assert_eq!(
            decode(ImageFormat::Png, &gray)?.rgba,
            [7, 7, 7, 255, 9, 9, 9, 255]
        );
        Ok(())
    }

    #[test]
    fn reports_truncated_and_corrupt_png() {
        let original = png(4, 4, png::ColorType::Rgb, &[200; 48]);
        let (truncated, _) = original.split_at(original.len() / 2);
        assert_invalid(ImageFormat::Png, truncated);
        assert_invalid(ImageFormat::Png, b"not an image");
        let mut corrupt = original;
        if let Some(byte) = corrupt.get_mut(40) {
            *byte ^= 0xff;
        }
        assert!(decode(ImageFormat::Png, &corrupt).is_err());
    }

    #[test]
    fn rejects_huge_png_before_allocating_pixels() {
        // A header that claims 100,000 × 100,000 pixels with almost no data.
        let mut header = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        let fields = [
            100_000_u32.to_be_bytes().as_slice(),
            100_000_u32.to_be_bytes().as_slice(),
            &[8, 6, 0, 0, 0],
        ]
        .concat();
        let mut chunk = b"IHDR".to_vec();
        chunk.extend_from_slice(&fields);
        header.extend_from_slice(&fields);
        header.extend_from_slice(&crc32(&chunk).to_be_bytes());
        assert!(matches!(
            decode(ImageFormat::Png, &header),
            Err(ImageError::TooLarge {
                width: 100_000,
                height: 100_000
            })
        ));
    }

    #[test]
    fn decodes_jpeg_color_models_and_progressive() -> Result<(), Box<dyn Error>> {
        for (bytes, expected) in [
            (
                include_bytes!("../../tests/fixtures/jpeg/rgb.jpg").as_slice(),
                RED,
            ),
            (
                include_bytes!("../../tests/fixtures/jpeg/progressive.jpg").as_slice(),
                RED,
            ),
            (
                include_bytes!("../../tests/fixtures/jpeg/gray.jpg").as_slice(),
                [120, 120, 120, 255],
            ),
            (
                include_bytes!("../../tests/fixtures/jpeg/cmyk.jpg").as_slice(),
                [255, 0, 0, 255],
            ),
        ] {
            assert_solid(&decode(ImageFormat::Jpeg, bytes)?, (16, 12), expected, 3);
        }
        Ok(())
    }

    #[test]
    fn rejects_invalid_and_oversized_jpeg() {
        let original = include_bytes!("../../tests/fixtures/jpeg/rgb.jpg");
        assert_invalid(ImageFormat::Jpeg, b"broken JPEG");
        let (truncated, _) = original.split_at(original.len() / 2);
        assert!(decode(ImageFormat::Jpeg, truncated).is_err());
        for (width, height) in [(8193_u16, 12_u16), (8192, 8192)] {
            let mut bytes = original.to_vec();
            let offset = bytes.windows(2).position(|pair| pair == [0xff, 0xc0]);
            assert!(offset.is_some());
            if let Some(offset) = offset
                && let Some(fields) = bytes.get_mut(offset + 5..offset + 9)
            {
                fields.copy_from_slice(&[height.to_be_bytes(), width.to_be_bytes()].concat());
            }
            assert!(matches!(
                decode(ImageFormat::Jpeg, &bytes),
                Err(ImageError::TooLarge { .. })
            ));
        }
    }

    #[test]
    fn decodes_first_gif_frame_and_counts_frames() -> Result<(), Box<dyn Error>> {
        let bytes = include_bytes!("../../tests/fixtures/images/animated.gif");
        let image = decode(ImageFormat::Gif, bytes)?;
        assert_solid(&image, (16, 12), FIXTURE_RED, 0);
        assert_eq!(image.notes, ["2 frames, first frame shown"]);
        Ok(())
    }

    #[test]
    fn rejects_invalid_and_oversized_gif() {
        let original = include_bytes!("../../tests/fixtures/images/animated.gif");
        assert_invalid(ImageFormat::Gif, b"GIF89a broken");
        let (truncated, _) = original.split_at(40);
        assert_invalid(ImageFormat::Gif, truncated);
        // The logical screen size follows the signature, little-endian.
        let mut huge = original.to_vec();
        if let Some(fields) = huge.get_mut(6..10) {
            fields.copy_from_slice(&[0xff, 0xff, 0x10, 0x00]);
        }
        assert!(matches!(
            decode(ImageFormat::Gif, &huge),
            Err(ImageError::TooLarge {
                width: 65_535,
                height: 16
            })
        ));
    }

    #[test]
    fn decodes_lossy_lossless_and_animated_webp() -> Result<(), Box<dyn Error>> {
        let lossy = decode(
            ImageFormat::WebP,
            include_bytes!("../../tests/fixtures/images/lossy.webp"),
        )?;
        assert_solid(&lossy, (16, 12), FIXTURE_RED, 8);
        let lossless = decode(
            ImageFormat::WebP,
            include_bytes!("../../tests/fixtures/images/lossless.webp"),
        )?;
        assert_solid(&lossless, (16, 12), FIXTURE_RED, 0);
        assert_eq!(lossless.notes, Vec::<String>::new());
        let animated = decode(
            ImageFormat::WebP,
            include_bytes!("../../tests/fixtures/images/animated.webp"),
        )?;
        assert_solid(&animated, (16, 12), FIXTURE_RED, 0);
        assert_eq!(animated.notes, ["2 frames, first frame shown"]);
        Ok(())
    }

    #[test]
    fn rejects_invalid_and_oversized_webp() {
        let original = include_bytes!("../../tests/fixtures/images/animated.webp");
        assert_invalid(ImageFormat::WebP, b"RIFF\x04\x00\x00\x00WEBP");
        let lossless = include_bytes!("../../tests/fixtures/images/lossless.webp");
        let (truncated, _) = lossless.split_at(lossless.len() - 6);
        assert!(decode(ImageFormat::WebP, truncated).is_err());
        // The VP8X chunk stores canvas width − 1 and height − 1 in 24 bits each.
        let mut huge = original.to_vec();
        if let Some(fields) = huge.get_mut(24..30) {
            fields.copy_from_slice(&[0xff, 0x3f, 0x00, 0xff, 0x3f, 0x00]);
        }
        let r = decode(ImageFormat::WebP, &huge);
        assert!(
            matches!(
                r,
                Err(ImageError::TooLarge {
                    width: 16_384,
                    height: 16_384
                })
            ),
            "{r:?}"
        );
    }

    #[test]
    fn decodes_bmp_and_rejects_invalid_or_oversized() -> Result<(), Box<dyn Error>> {
        let original = include_bytes!("../../tests/fixtures/images/rgb24.bmp");
        assert_solid(
            &decode(ImageFormat::Bmp, original)?,
            (16, 12),
            FIXTURE_RED,
            0,
        );
        assert_invalid(ImageFormat::Bmp, b"BM broken");
        let (truncated, _) = original.split_at(80);
        assert!(decode(ImageFormat::Bmp, truncated).is_err());
        // BITMAPINFOHEADER width and height, little-endian.
        let mut huge = original.to_vec();
        if let Some(fields) = huge.get_mut(18..26) {
            fields.copy_from_slice(&[0x00, 0x40, 0, 0, 0x00, 0x40, 0, 0]);
        }
        assert!(matches!(
            decode(ImageFormat::Bmp, &huge),
            Err(ImageError::TooLarge {
                width: 16_384,
                height: 16_384
            })
        ));
        Ok(())
    }

    fn ico(sizes: &[(u32, [u8; 4])]) -> Result<Vec<u8>, Box<dyn Error>> {
        let pngs = sizes
            .iter()
            .map(|&(side, color)| {
                let pixels = color.repeat((side * side) as usize);
                (side, png(side, side, png::ColorType::Rgba, &pixels))
            })
            .collect::<Vec<_>>();
        let frames = pngs
            .iter()
            .map(|(side, bytes)| {
                image::codecs::ico::IcoFrame::with_encoded(
                    bytes.as_slice(),
                    *side,
                    *side,
                    image::ExtendedColorType::Rgba8,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut bytes = Vec::new();
        image::codecs::ico::IcoEncoder::new(&mut bytes).encode_images(&frames)?;
        Ok(bytes)
    }

    #[test]
    fn decodes_the_largest_icon() -> Result<(), Box<dyn Error>> {
        let bytes = ico(&[(16, RED), (48, BLUE), (32, RED)])?;
        assert_eq!(ImageFormat::from_signature(&bytes), Some(ImageFormat::Ico));
        let image = decode(ImageFormat::Ico, &bytes)?;
        assert_solid(&image, (48, 48), BLUE, 0);
        assert_eq!(image.notes, ["3 images in the file, largest shown"]);
        assert_invalid(ImageFormat::Ico, b"\x00\x00\x01\x00\x01\x00broken");
        Ok(())
    }

    #[test]
    fn rasterizes_svg_and_scales_down_large_drawings() -> Result<(), Box<dyn Error>> {
        let svg = br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10">
  <rect width="20" height="10" fill="#f0140a"/>
</svg>"##;
        let image = decode(ImageFormat::Svg, svg)?;
        assert_solid(&image, (20, 10), RED, 0);
        assert_eq!(image.notes, Vec::<String>::new());

        let large = br##"<svg xmlns="http://www.w3.org/2000/svg" width="10000" height="5000">
  <rect width="10000" height="5000" fill="#0a14f0"/><text>label</text>
</svg>"##;
        let image = decode(ImageFormat::Svg, large)?;
        assert_solid(&image, (2048, 1024), BLUE, 0);
        assert_eq!(
            image.notes,
            [
                "drawn scaled down from 10000×5000 px",
                "text is not drawn; use Source to read it"
            ]
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_svg() {
        assert_invalid(ImageFormat::Svg, b"<svg");
        assert_invalid(ImageFormat::Svg, b"<html><body/></html>");
        assert_invalid(
            ImageFormat::Svg,
            b"<svg xmlns=\"http://www.w3.org/2000/svg\">\xff</svg>",
        );
    }

    #[test]
    fn rejects_svg_with_too_many_expanded_elements() {
        // Each group uses the previous one twice, doubling the drawn elements
        // per level: a "billion laughs" built from <use>.
        let mut svg = String::from(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><rect id="g0" width="1" height="1"/>"#,
        );
        for level in 1..=30 {
            let previous = level - 1;
            let _ = write!(
                svg,
                r##"<g id="g{level}"><use xlink:href="#g{previous}"/><use href="#g{previous}"/></g>"##
            );
        }
        svg.push_str(r##"</defs><use href="#g30"/></svg>"##);
        assert!(matches!(
            decode(ImageFormat::Svg, svg.as_bytes()),
            Err(ImageError::TooComplex)
        ));

        let deep = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg">{}{}</svg>"#,
            "<g>".repeat(300),
            "</g>".repeat(300)
        );
        assert!(matches!(
            decode(ImageFormat::Svg, deep.as_bytes()),
            Err(ImageError::TooComplex)
        ));

        // The converter would expand a reference cycle to a million nodes.
        let cycle = br##"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1">
  <g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g></svg>"##;
        assert!(matches!(
            decode(ImageFormat::Svg, cycle),
            Err(ImageError::TooComplex)
        ));
    }

    #[test]
    fn svg_loads_no_external_resources() -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let origin = format!("http://{}", listener.local_addr()?);
        let png = png(1, 1, png::ColorType::Rgba, &[0, 255, 0, 255]);
        let data_url = format!("data:image/png;base64,{}", base64(&png));
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4">
  <style>@import url("{origin}/style.css"); @font-face {{ font-family: x; src: url("{origin}/font.woff"); }}</style>
  <script>fetch("{origin}/script")</script>
  <image href="{origin}/image.png" width="4" height="4"/>
  <image xlink:href="/etc/hosts" width="4" height="4"/>
  <image href="{data_url}" width="4" height="4"/>
  <use href="{origin}/sprite.svg#icon"/>
  <rect width="4" height="4" fill="url({origin}/paint.svg#p)"/>
</svg>"#
        );
        let image = decode(ImageFormat::Svg, svg.as_bytes())?;
        assert_eq!((image.width, image.height), (4, 4));
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[1] != 255),
            "an <image> was drawn"
        );
        assert_eq!(
            image.notes,
            ["embedded images and foreign content are not loaded"]
        );
        match listener.accept() {
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(()),
            other => Err(format!("the SVG made a connection: {other:?}").into()),
        }
    }

    fn base64(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut text = String::new();
        for chunk in bytes.chunks(3) {
            let block = chunk
                .iter()
                .enumerate()
                .fold(0_u32, |block, (index, &byte)| {
                    block | u32::from(byte) << (16 - 8 * index)
                });
            for index in 0..4 {
                if index <= chunk.len() {
                    let sextet = (block >> (18 - 6 * index)) & 0x3f;
                    text.push(char::from(*ALPHABET.get(sextet as usize).unwrap_or(&b'A')));
                } else {
                    text.push('=');
                }
            }
        }
        text
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0_u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
}
