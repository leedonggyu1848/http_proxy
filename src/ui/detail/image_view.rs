//! Image response bodies drawn as images, with source and hexadecimal views.

use std::fmt::Write as _;

use eframe::egui::{self, TextureHandle, TextureOptions};
use http_proxy::exchange::{BodyLifecycle, BodyRetention, BodySnapshot, ImageFormat, RecordId};
use http_proxy::interception::{IMAGE_PREVIEW_BUDGET_BYTES, IMAGE_PREVIEW_BYTES};

use crate::ui::style::theme::Palette;
use http_proxy::content::decode as body_view;
use http_proxy::content::image::{self as image_decode, PreviewImage};

/// Raw bytes shown in the hexadecimal view.
const HEX_BYTES: usize = 8 * 1024;
/// SVG source bytes shown in the source view.
const SOURCE_BYTES: usize = 256 * 1024;
/// Height at which drawn images are scaled down to keep the panel readable.
const MAX_DISPLAY_HEIGHT: f32 = 480.0;

/// Reason an image body could not be prepared for display.
#[derive(Debug, thiserror::Error)]
enum ImageViewError {
    #[error(transparent)]
    DecodeBody(#[from] body_view::DecodeError),
    #[error(transparent)]
    DecodeImage(#[from] image_decode::ImageError),
    #[error(transparent)]
    TextureTooLarge(#[from] TextureTooLargeError),
}

#[derive(Debug, thiserror::Error)]
#[error(
    "The image is {width}×{height} px, larger than this display's {max_side} px texture limit."
)]
struct TextureTooLargeError {
    width: u32,
    height: u32,
    max_side: usize,
}

/// The image file a body holds once `Content-Encoding` is removed.
struct ImageFile {
    bytes: Vec<u8>,
    /// The format named by the leading bytes, or by `Content-Type` when the
    /// bytes have no known signature.
    format: ImageFormat,
}

impl ImageFile {
    /// Removes `Content-Encoding` and recognizes the format.
    fn decode_body(
        bytes: &[u8],
        encodings: &[&str],
        declared: ImageFormat,
    ) -> Result<Self, body_view::DecodeError> {
        let bytes = body_view::decode_complete(bytes, encodings, IMAGE_PREVIEW_BYTES)?;
        let format = ImageFormat::from_signature(&bytes).unwrap_or(declared);
        Ok(Self { bytes, format })
    }
}

/// Explains that the body is drawn as another format than `Content-Type`
/// names, or `None` when they agree.
fn mismatch_text(declared: ImageFormat, format: ImageFormat) -> Option<String> {
    (declared != format).then(|| {
        format!(
            "Content-Type names {}, but the body starts like {}; it is drawn as {}.",
            declared.name(),
            format.name(),
            format.name()
        )
    })
}

/// Formats `bytes` as offset, hexadecimal and ASCII columns, 16 bytes per line.
pub(super) fn hex_dump(bytes: &[u8]) -> String {
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

/// How the selected image body is shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum View {
    #[default]
    Image,
    /// SVG markup as text.
    Source,
    Hex,
}

struct Drawn {
    texture: TextureHandle,
    image_text: String,
}

/// One decode of a body, kept so it is not repeated every frame.
struct Loaded {
    /// The recognized format, or `None` when `Content-Encoding` could not be
    /// removed.
    format: Option<ImageFormat>,
    drawn: Result<Drawn, ImageViewError>,
    /// Lossy UTF-8 text of an SVG file, cut to [`SOURCE_BYTES`].
    source: Option<String>,
    file_bytes: usize,
}

/// Image, source, or hexadecimal view of the selected image response body,
/// with the last decoded texture kept so it is not decoded again every frame.
#[derive(Default)]
pub(crate) struct ImagePanel {
    cached: Option<(BodyKey, Loaded)>,
    view: Option<(RecordId, View)>,
}

impl ImagePanel {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        record: RecordId,
        snapshot: &BodySnapshot,
        encodings: &[&str],
        declared: ImageFormat,
    ) {
        let unavailable = unavailable_reason(snapshot);
        let key = BodyKey {
            record,
            observed_bytes: snapshot.observed_bytes(),
            lifecycle: snapshot.lifecycle(),
        };
        if unavailable.is_none()
            && self
                .cached
                .as_ref()
                .is_none_or(|(cached, _)| *cached != key)
        {
            let loaded = load(ui.ctx(), record, snapshot.bytes(), encodings, declared);
            self.cached = Some((key, loaded));
        }
        let loaded = self
            .cached
            .as_ref()
            .filter(|(cached, _)| unavailable.is_none() && *cached == key)
            .map(|(_, loaded)| loaded);
        let svg = loaded.and_then(|loaded| loaded.format).unwrap_or(declared) == ImageFormat::Svg;

        let mut view = self
            .view
            .filter(|(selected, _)| *selected == record)
            .map_or(View::Image, |(_, view)| view);
        if view == View::Source && !svg {
            view = View::Image;
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut view, View::Image, "Image");
            if svg {
                ui.selectable_value(&mut view, View::Source, "Source");
            }
            ui.selectable_value(&mut view, View::Hex, "Hex");
        });
        if let Some(format) = loaded.and_then(|loaded| loaded.format)
            && let Some(text) = mismatch_text(declared, format)
        {
            ui.colored_label(Palette::of(ui).warning, text);
        }
        match (view, loaded) {
            (View::Hex, _) => show_hex(ui, snapshot),
            (_, None) => {
                if let Some(message) = unavailable {
                    ui.colored_label(Palette::of(ui).warning, message);
                }
            }
            (View::Source, Some(loaded)) => show_source(ui, loaded),
            (View::Image, Some(loaded)) => {
                if let Some(switch) = show_image(ui, loaded, snapshot.observed_bytes(), svg) {
                    view = switch;
                }
            }
        }
        self.view = Some((record, view));
    }
}

/// Draws the decoded image or its error; returns the view a button asked for.
fn show_image(ui: &mut egui::Ui, loaded: &Loaded, received_bytes: u64, svg: bool) -> Option<View> {
    match &loaded.drawn {
        Ok(drawn) => {
            ui.label(format!(
                "{}{}",
                drawn.image_text,
                received_text(loaded.file_bytes, received_bytes)
            ));
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
            None
        }
        Err(error) => {
            ui.colored_label(Palette::of(ui).error, error.to_string());
            ui.horizontal_wrapped(|ui| {
                let source =
                    (svg && ui.button("Show SVG source").clicked()).then_some(View::Source);
                let hex = ui
                    .button("Show bytes as hex")
                    .clicked()
                    .then_some(View::Hex);
                source.or(hex)
            })
            .inner
        }
    }
}

fn show_source(ui: &mut egui::Ui, loaded: &Loaded) {
    let Some(source) = &loaded.source else {
        if let Err(error) = &loaded.drawn {
            ui.colored_label(Palette::of(ui).error, error.to_string());
        }
        return;
    };
    ui.weak(if loaded.file_bytes > SOURCE_BYTES {
        format!(
            "First {} KiB of {} bytes of SVG source, shown as text and never run.",
            SOURCE_BYTES / 1024,
            loaded.file_bytes
        )
    } else {
        format!(
            "{} bytes of SVG source, shown as text and never run.",
            loaded.file_bytes
        )
    });
    let mut text = source.as_str();
    egui::ScrollArea::vertical()
        .id_salt("image-body-source")
        .max_height(MAX_DISPLAY_HEIGHT)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
}

/// Describes the format, pixel size, file size, and notes of a decoded image.
fn image_text(format: ImageFormat, image: &PreviewImage, file_bytes: usize) -> String {
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
fn received_text(file_bytes: usize, received_bytes: u64) -> String {
    if u64::try_from(file_bytes).unwrap_or(u64::MAX) == received_bytes {
        String::new()
    } else {
        format!(" · {received_bytes} bytes received with Content-Encoding")
    }
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
            "The image cannot be drawn because only {} of {} bytes were kept. Image previews keep up to {} MiB per response and {} MiB across all responses; use Hex to inspect the kept bytes.",
            snapshot.bytes().len(),
            snapshot.observed_bytes(),
            IMAGE_PREVIEW_BYTES / (1024 * 1024),
            IMAGE_PREVIEW_BUDGET_BYTES / (1024 * 1024),
        )
    })
}

/// Removes `Content-Encoding`, decodes the image, and uploads it as a texture.
fn load(
    context: &egui::Context,
    record: RecordId,
    bytes: &[u8],
    encodings: &[&str],
    declared: ImageFormat,
) -> Loaded {
    let file = match ImageFile::decode_body(bytes, encodings, declared) {
        Ok(file) => file,
        Err(error) => {
            return Loaded {
                format: None,
                drawn: Err(ImageViewError::DecodeBody(error)),
                source: None,
                file_bytes: 0,
            };
        }
    };
    let source = (file.format == ImageFormat::Svg).then(|| {
        let (shown, _) = file.bytes.split_at(file.bytes.len().min(SOURCE_BYTES));
        String::from_utf8_lossy(shown).into_owned()
    });
    let drawn = image_decode::decode(file.format, &file.bytes)
        .map_err(ImageViewError::DecodeImage)
        .and_then(|image| {
            let image_text = image_text(file.format, &image, file.bytes.len());
            texture(context, record, &image)
                .map_err(ImageViewError::TextureTooLarge)
                .map(|texture| Drawn {
                    texture,
                    image_text,
                })
        });
    Loaded {
        format: Some(file.format),
        drawn,
        source,
        file_bytes: file.bytes.len(),
    }
}

fn texture(
    context: &egui::Context,
    record: RecordId,
    image: &PreviewImage,
) -> Result<TextureHandle, TextureTooLargeError> {
    let max_side = context.input(|input| input.max_texture_side);
    let (width, height) = (image.width, image.height);
    let pixels = [width, height].map(|side| usize::try_from(side).unwrap_or(usize::MAX));
    if pixels.iter().any(|&side| side > max_side) {
        return Err(TextureTooLargeError {
            width,
            height,
            max_side,
        });
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(pixels, &image.rgba);
    Ok(context.load_texture(
        format!("image-body-{record:?}"),
        color,
        TextureOptions::LINEAR,
    ))
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
        .id_salt("image-body-hex")
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

    use http_proxy::exchange::ImageFormat;

    use super::{ImageFile, hex_dump, mismatch_text};

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
    fn formats_hex_with_offsets_and_ascii() {
        assert_eq!(
            hex_dump(b"\x89PNG\r\n\x1a\n0123456789"),
            "00000000  89 50 4e 47 0d 0a 1a 0a 30 31 32 33 34 35 36 37  |.PNG....01234567|\n\
             00000010  38 39                                            |89|\n"
        );
    }
}
