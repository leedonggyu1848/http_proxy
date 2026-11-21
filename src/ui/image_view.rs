//! PNG response bodies drawn as images, with a hexadecimal fallback.

use std::fmt::Write as _;
use std::io::Cursor;

use eframe::egui::{self, TextureHandle, TextureOptions};
use http_proxy::{
    BodyLifecycle, BodyRetention, BodySnapshot, IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES,
    RecordId,
};

use crate::ui::body_view;

/// Widest or tallest image the decoder accepts.
const MAX_IMAGE_SIDE: u32 = 8_192;
/// Most pixels the decoder accepts, which bounds the RGBA buffer to 64 MiB.
const MAX_IMAGE_PIXELS: u64 = 4_096 * 4_096;
/// Allocation ceiling handed to the PNG decoder: the RGBA frame plus row buffers.
const DECODER_MEMORY_BYTES: usize = 80 * 1024 * 1024;
/// Raw bytes shown in the hexadecimal view.
const HEX_BYTES: usize = 8 * 1024;
/// Height at which drawn images are scaled down to keep the panel readable.
const MAX_DISPLAY_HEIGHT: f32 = 480.0;

/// A decoded PNG frame as unmultiplied 8-bit RGBA pixels.
#[derive(Debug)]
pub(in crate::ui) struct PngImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Length of the PNG file after `Content-Encoding` was removed.
    file_bytes: usize,
}

/// Why a PNG body could not be drawn.
#[derive(Debug, thiserror::Error)]
pub(in crate::ui) enum PngError {
    #[error("{0}")]
    Encoding(String),
    #[error(
        "The image is {width}×{height} px, larger than the {MAX_IMAGE_SIDE} px per side or 16 megapixel preview limit."
    )]
    TooLarge { width: u32, height: u32 },
    #[error("The body is not a valid PNG image: {0}")]
    Invalid(String),
}

/// Decodes the first frame of a PNG body after removing `Content-Encoding`.
///
/// The image size is read from the header and checked before any pixel
/// memory is allocated.
pub(in crate::ui) fn decode_png(bytes: &[u8], encodings: &[&str]) -> Result<PngImage, PngError> {
    let png_bytes = body_view::decode_complete(bytes, encodings, IMAGE_PREVIEW_BYTES)
        .map_err(PngError::Encoding)?;
    let invalid = |error: png::DecodingError| PngError::Invalid(error.to_string());
    let mut decoder = png::Decoder::new_with_limits(
        Cursor::new(png_bytes.as_slice()),
        png::Limits {
            bytes: DECODER_MEMORY_BYTES,
        },
    );
    let (width, height) = decoder.read_header_info().map_err(invalid)?.size();
    if width > MAX_IMAGE_SIDE
        || height > MAX_IMAGE_SIDE
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(PngError::TooLarge { width, height });
    }
    // ALPHA expands palettes and low bit depths and always adds an alpha
    // channel, so the output is 8-bit gray+alpha or RGBA.
    decoder.set_transformations(png::Transformations::ALPHA | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(invalid)?;
    let size = reader
        .output_buffer_size()
        .ok_or(PngError::TooLarge { width, height })?;
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
            return Err(PngError::Invalid(format!(
                "unexpected decoded color type {other:?}"
            )));
        }
    };
    Ok(PngImage {
        width,
        height,
        rgba,
        file_bytes: png_bytes.len(),
    })
}

/// Formats `bytes` as offset, hexadecimal and ASCII columns, 16 bytes per line.
pub(in crate::ui) fn hex_dump(bytes: &[u8]) -> String {
    let mut text = String::new();
    for (line, chunk) in bytes.chunks(16).enumerate() {
        let _ = write!(text, "{:08x} ", line * 16);
        for column in 0..16 {
            match chunk.get(column) {
                Some(byte) => {
                    let _ = write!(text, " {byte:02x}");
                }
                None => text.push_str("   "),
            }
        }
        text.push_str("  |");
        text.extend(chunk.iter().map(|&byte| {
            if byte.is_ascii_graphic() || byte == b' ' {
                char::from(byte)
            } else {
                '.'
            }
        }));
        text.push_str("|\n");
    }
    text
}

/// Which body the cached decode belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BodyKey {
    record: RecordId,
    observed_bytes: u64,
    lifecycle: BodyLifecycle,
}

struct Drawn {
    texture: TextureHandle,
    width: u32,
    height: u32,
    file_bytes: usize,
}

/// Image or hexadecimal view of the selected PNG response body, with the
/// last decoded texture kept so it is not decoded again every frame.
#[derive(Default)]
pub(in crate::ui) struct ImagePanel {
    cached: Option<(BodyKey, Result<Drawn, String>)>,
    hex_record: Option<RecordId>,
}

impl ImagePanel {
    pub(in crate::ui) fn show(
        &mut self,
        ui: &mut egui::Ui,
        record: RecordId,
        snapshot: &BodySnapshot,
        encodings: &[&str],
    ) {
        let mut hex = self.hex_record == Some(record);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut hex, false, "Image");
            ui.selectable_value(&mut hex, true, "Hex");
        });
        if hex {
            self.hex_record = Some(record);
            show_hex(ui, snapshot);
            return;
        }
        self.hex_record = None;
        if let Some(message) = unavailable_reason(snapshot) {
            ui.colored_label(crate::ui::theme::Palette::of(ui).warning, message);
            return;
        }
        let key = BodyKey {
            record,
            observed_bytes: snapshot.observed_bytes(),
            lifecycle: snapshot.lifecycle(),
        };
        if self
            .cached
            .as_ref()
            .is_none_or(|(cached, _)| *cached != key)
        {
            let drawn = draw(ui.ctx(), record, snapshot.bytes(), encodings);
            self.cached = Some((key, drawn));
        }
        let mut switch_to_hex = false;
        match self.cached.as_ref().map(|(_, drawn)| drawn) {
            Some(Ok(drawn)) => {
                ui.label(size_text(drawn, snapshot.observed_bytes()));
                let size = egui::vec2(ui.available_width(), MAX_DISPLAY_HEIGHT);
                let image = egui::Image::from_texture(&drawn.texture).max_size(size);
                let image_size = image.calc_size(size, Some(drawn.texture.size_vec2()));
                let (rect, _) = ui.allocate_exact_size(image_size, egui::Sense::hover());
                checkerboard(ui, rect);
                image.paint_at(ui, rect);
                ui.painter().rect_stroke(
                    rect,
                    0.0,
                    ui.visuals().window_stroke,
                    egui::StrokeKind::Outside,
                );
            }
            Some(Err(message)) => {
                ui.colored_label(crate::ui::theme::Palette::of(ui).error, message);
                switch_to_hex = ui
                    .horizontal(|ui| ui.button("Show bytes as hex").clicked())
                    .inner;
            }
            None => {}
        }
        if switch_to_hex {
            self.hex_record = Some(record);
        }
    }
}

/// Describes the image size, the PNG file size, and, when `Content-Encoding`
/// changed it, the number of bytes received.
fn size_text(drawn: &Drawn, received_bytes: u64) -> String {
    let file_bytes = u64::try_from(drawn.file_bytes).unwrap_or(u64::MAX);
    let received = if file_bytes == received_bytes {
        String::new()
    } else {
        format!(" · {received_bytes} bytes received with Content-Encoding")
    };
    format!(
        "PNG · {}×{} px · {file_bytes} bytes{received}",
        drawn.width, drawn.height
    )
}

/// Explains why a body cannot be drawn yet, or `None` when it can be decoded.
fn unavailable_reason(snapshot: &BodySnapshot) -> Option<String> {
    if snapshot.lifecycle() == BodyLifecycle::Streaming {
        return Some(format!(
            "The image is drawn when the body finishes ({} bytes so far).",
            snapshot.observed_bytes()
        ));
    }
    (snapshot.retention() != BodyRetention::Prefix).then(|| {
        format!(
            "The image cannot be drawn because only {} of {} bytes were kept. PNG previews keep up to {} MiB per response and {} MiB across all responses; use Hex to inspect the kept bytes.",
            snapshot.bytes().len(),
            snapshot.observed_bytes(),
            IMAGE_PREVIEW_BYTES / (1024 * 1024),
            IMAGE_PREVIEW_BUDGET_BYTES / (1024 * 1024),
        )
    })
}

fn draw(
    context: &egui::Context,
    record: RecordId,
    bytes: &[u8],
    encodings: &[&str],
) -> Result<Drawn, String> {
    let image = decode_png(bytes, encodings).map_err(|error| error.to_string())?;
    let max_side = context.input(|input| input.max_texture_side);
    let (width, height) = (image.width, image.height);
    let pixels = [width, height].map(|side| usize::try_from(side).unwrap_or(usize::MAX));
    if pixels.iter().any(|&side| side > max_side) {
        return Err(format!(
            "The image is {width}×{height} px, larger than this display's {max_side} px texture limit."
        ));
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(pixels, &image.rgba);
    let texture = context.load_texture(
        format!("png-body-{record:?}"),
        color,
        TextureOptions::LINEAR,
    );
    Ok(Drawn {
        texture,
        width,
        height,
        file_bytes: image.file_bytes,
    })
}

fn show_hex(ui: &mut egui::Ui, snapshot: &BodySnapshot) {
    let bytes = snapshot.bytes();
    let (shown, _) = bytes.split_at(bytes.len().min(HEX_BYTES));
    ui.weak(format!(
        "First {} of {} kept bytes, as received (before Content-Encoding is removed).",
        shown.len(),
        bytes.len()
    ));
    let mut text = hex_dump(shown);
    egui::ScrollArea::vertical()
        .id_salt("png-body-hex")
        .max_height(MAX_DISPLAY_HEIGHT)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .interactive(false),
            );
        });
}

fn checkerboard(ui: &egui::Ui, rect: egui::Rect) {
    let mut y = rect.top();
    let mut row = false;
    loop {
        if y >= rect.bottom() {
            break;
        }
        let mut x = rect.left();
        let mut alternate = row;
        loop {
            if x >= rect.right() {
                break;
            }
            let tile = egui::Rect::from_min_max(
                egui::pos2(x, y),
                egui::pos2((x + 12.0).min(rect.right()), (y + 12.0).min(rect.bottom())),
            );
            let fill = if alternate {
                ui.visuals().extreme_bg_color
            } else {
                ui.visuals().widgets.hovered.bg_fill
            };
            ui.painter().rect_filled(tile, 0.0, fill);
            alternate = !alternate;
            x += 12.0;
        }
        row = !row;
        y += 12.0;
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::Write;

    use super::{PngError, decode_png, hex_dump};

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

    #[test]
    fn decodes_rgb_and_gray_images_to_rgba() -> Result<(), Box<dyn Error>> {
        let rgb = png(2, 1, png::ColorType::Rgb, &[255, 0, 0, 0, 0, 255]);
        let image = decode_png(&rgb, &[])?;
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.rgba, [255, 0, 0, 255, 0, 0, 255, 255]);

        let gray = png(1, 2, png::ColorType::Grayscale, &[7, 9]);
        assert_eq!(decode_png(&gray, &[])?.rgba, [7, 7, 7, 255, 9, 9, 9, 255]);
        Ok(())
    }

    #[test]
    fn removes_content_encoding_before_decoding() -> Result<(), Box<dyn Error>> {
        let original = png(1, 1, png::ColorType::Rgba, &[1, 2, 3, 4]);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&original)?;
        let image = decode_png(&encoder.finish()?, &["gzip"])?;
        assert_eq!(image.rgba, [1, 2, 3, 4]);
        assert_eq!(image.file_bytes, original.len());
        Ok(())
    }

    #[test]
    fn reports_truncated_and_corrupt_images() {
        let original = png(4, 4, png::ColorType::Rgb, &[200; 48]);
        let (truncated, _) = original.split_at(original.len() / 2);
        assert!(matches!(
            decode_png(truncated, &[]),
            Err(PngError::Invalid(_))
        ));
        assert!(matches!(
            decode_png(b"not an image", &[]),
            Err(PngError::Invalid(_))
        ));
        let mut corrupt = original;
        if let Some(byte) = corrupt.get_mut(40) {
            *byte ^= 0xff;
        }
        assert!(decode_png(&corrupt, &[]).is_err());
        assert!(matches!(
            decode_png(b"\x1f\x8b broken", &["gzip"]),
            Err(PngError::Encoding(_))
        ));
    }

    #[test]
    fn rejects_huge_dimensions_before_allocating_pixels() {
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
            decode_png(&header, &[]),
            Err(PngError::TooLarge {
                width: 100_000,
                height: 100_000
            })
        ));
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

    #[test]
    fn formats_hex_with_offsets_and_ascii() {
        assert_eq!(
            hex_dump(b"\x89PNG\r\n\x1a\n0123456789"),
            "00000000  89 50 4e 47 0d 0a 1a 0a 30 31 32 33 34 35 36 37  |.PNG....01234567|\n\
             00000010  38 39                                            |89|\n"
        );
    }
}
