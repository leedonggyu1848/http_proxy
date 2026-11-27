//! Image response bodies drawn as images, with source and hexadecimal views.

use std::fmt::Write as _;

use crate::ui::style::tokens::{Layout, Radius};

use eframe::egui::{self, TextureHandle, TextureOptions};
use http_inspector_exchange::{BodyLifecycle, BodySnapshot, ImageFormat, RecordId};

use crate::ui::i18n::{self, Language, domain};
use crate::ui::style::theme::Palette;
use http_inspector_content::decode as body_view;
use http_inspector_content::hex::{self, HEX_VIEW_BYTES};
use http_inspector_content::image::{self as image_decode, PreviewImage};
use http_inspector_content::image_file::ImageFile;

/// SVG source bytes shown in the source view.
const SOURCE_BYTES: usize = 256 * 1024;
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

impl ImageViewError {
    fn localized(&self, lang: Language) -> String {
        match self {
            Self::DecodeBody(error) => domain::decode_error(lang, error),
            Self::DecodeImage(error) => domain::image_error(lang, error),
            Self::TextureTooLarge(TextureTooLargeError {
                width,
                height,
                max_side,
            }) => i18n::detail::image_texture_too_large(lang, *width, *height, *max_side),
        }
    }
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
    format: ImageFormat,
    width: u32,
    height: u32,
    notes: Vec<String>,
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

/// Image body inputs for [`ImagePanel::show`].
pub(super) struct ImageBody<'a> {
    pub(super) record: RecordId,
    pub(super) snapshot: &'a BodySnapshot,
    pub(super) encodings: &'a [&'a str],
    pub(super) declared: ImageFormat,
}

impl ImagePanel {
    pub(super) fn show(&mut self, ui: &mut egui::Ui, body: &ImageBody<'_>, lang: Language) {
        let ImageBody {
            record,
            snapshot,
            encodings,
            declared,
        } = *body;
        let unavailable = i18n::detail::image_unavailable_reason(lang, snapshot);
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
        let detail = lang.detail();
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut view, View::Image, detail.image);
            if svg {
                ui.selectable_value(&mut view, View::Source, detail.source);
            }
            ui.selectable_value(&mut view, View::Hex, "Hex");
        });
        if let Some(format) = loaded.and_then(|loaded| loaded.format)
            && let Some(text) = i18n::detail::image_mismatch(lang, declared, format)
        {
            ui.colored_label(Palette::of(ui).warning, text);
        }
        match (view, loaded) {
            (View::Hex, _) => show_hex(ui, snapshot, lang),
            (_, None) => {
                if let Some(message) = unavailable {
                    ui.colored_label(Palette::of(ui).warning, message);
                }
            }
            (View::Source, Some(loaded)) => show_source(ui, loaded, lang),
            (View::Image, Some(loaded)) => {
                if let Some(switch) = show_image(ui, loaded, snapshot.observed_bytes(), svg, lang) {
                    view = switch;
                }
            }
        }
        self.view = Some((record, view));
    }
}

/// Draws the decoded image or its error; returns the view a button asked for.
fn show_image(
    ui: &mut egui::Ui,
    loaded: &Loaded,
    received_bytes: u64,
    svg: bool,
    lang: Language,
) -> Option<View> {
    match &loaded.drawn {
        Ok(drawn) => {
            ui.label(format!(
                "{}{}",
                image_text(drawn, loaded.file_bytes, lang),
                i18n::detail::image_received_text(lang, loaded.file_bytes, received_bytes)
            ));
            let size = egui::vec2(ui.available_width(), Layout::IMAGE_DISPLAY_HEIGHT);
            let image = egui::Image::from_texture(&drawn.texture).max_size(size);
            let image_size = image.calc_size(size, Some(drawn.texture.size_vec2()));
            let (rect, _) = ui.allocate_exact_size(image_size, egui::Sense::hover());
            checkerboard(ui, rect);
            image.paint_at(ui, rect);
            ui.painter().rect_stroke(
                rect,
                Radius::PANE,
                crate::ui::style::theme::image_border(ui),
                egui::StrokeKind::Outside,
            );
            None
        }
        Err(error) => {
            let detail = lang.detail();
            ui.colored_label(Palette::of(ui).error, error.localized(lang));
            ui.horizontal_wrapped(|ui| {
                let source =
                    (svg && ui.button(detail.show_svg_source).clicked()).then_some(View::Source);
                let hex = ui
                    .button(detail.show_bytes_as_hex)
                    .clicked()
                    .then_some(View::Hex);
                source.or(hex)
            })
            .inner
        }
    }
}

fn show_source(ui: &mut egui::Ui, loaded: &Loaded, lang: Language) {
    let Some(source) = &loaded.source else {
        if let Err(error) = &loaded.drawn {
            ui.colored_label(Palette::of(ui).error, error.localized(lang));
        }
        return;
    };
    let summary = i18n::detail::svg_source_summary(lang, loaded.file_bytes, SOURCE_BYTES);
    ui.weak(summary);
    let mut text = source.as_str();
    egui::ScrollArea::vertical()
        .id_salt("image-body-source")
        .max_height(Layout::IMAGE_DISPLAY_HEIGHT)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY),
            );
        });
}

/// Describes the format, pixel size, file size, and notes of a decoded image.
fn image_text(drawn: &Drawn, file_bytes: usize, lang: Language) -> String {
    let mut text = i18n::detail::image_meta_summary(
        lang,
        drawn.format.name(),
        drawn.width,
        drawn.height,
        file_bytes,
    );
    for note in &drawn.notes {
        let _ = write!(text, " · {}", domain::localize_content_notice(lang, note));
    }
    text
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
            let width = image.width;
            let height = image.height;
            let notes = image.notes.clone();
            texture(context, record, &image)
                .map_err(ImageViewError::TextureTooLarge)
                .map(|texture| Drawn {
                    texture,
                    format: file.format,
                    width,
                    height,
                    notes,
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

fn show_hex(ui: &mut egui::Ui, snapshot: &BodySnapshot, lang: Language) {
    let bytes = snapshot.bytes();
    let (shown, _) = bytes.split_at(bytes.len().min(HEX_VIEW_BYTES));
    let summary = i18n::detail::hex_view_summary(lang, shown.len(), bytes.len());
    ui.weak(summary);
    let dump = hex::dump(shown);
    let mut text = dump.as_str();
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _: f32| {
        ui.painter()
            .layout_no_wrap(text.as_str().to_owned(), font.clone(), Palette::of(ui).text)
    };
    egui::ScrollArea::both()
        .auto_shrink([false, true])
        .id_salt("image-body-hex")
        .max_height(Layout::IMAGE_DISPLAY_HEIGHT)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .layouter(&mut layouter),
            );
        });
}

fn checkerboard(ui: &egui::Ui, rect: egui::Rect) {
    let [inset, hover] = crate::ui::style::theme::image_checker_colors(ui);
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
                egui::pos2(
                    (x + Layout::IMAGE_CHECKER_TILE).min(rect.right()),
                    (y + Layout::IMAGE_CHECKER_TILE).min(rect.bottom()),
                ),
            );
            let fill = if alternate { inset } else { hover };
            ui.painter().rect_filled(tile, Radius::PANE, fill);
            alternate = !alternate;
            x += Layout::IMAGE_CHECKER_TILE;
        }
        row = !row;
        y += Layout::IMAGE_CHECKER_TILE;
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use http_inspector_exchange::ImageFormat;

    use super::Language;
    use crate::ui::i18n::detail::{image_mismatch, image_unavailable_reason};

    #[test]
    fn localizes_mismatch_and_unavailable_messages_in_korean() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            image_mismatch(Language::Korean, ImageFormat::Png, ImageFormat::Gif).as_deref(),
            Some(
                "Content-Type은 PNG(으)로 선언되었지만 본문 시작은 GIF 형식이어서 GIF(으)로 표시합니다."
            )
        );
        assert_eq!(
            image_mismatch(Language::Korean, ImageFormat::WebP, ImageFormat::WebP),
            None
        );

        let body = http_inspector_exchange::BodyPreviewWriter::new(32);
        body.observe(b"image prefix");
        body.interrupt();
        let snapshot = body.preview().snapshot().ok_or("missing snapshot")?;
        assert_eq!(
            image_unavailable_reason(Language::English, &snapshot).as_deref(),
            Some("The image body ended early. Use Hex to inspect the kept bytes.")
        );
        assert_eq!(
            image_unavailable_reason(Language::Korean, &snapshot).as_deref(),
            Some("이미지 본문이 중간에 종료되었습니다. 보관된 바이트를 보려면 Hex를 사용하세요.")
        );
        Ok(())
    }
}
