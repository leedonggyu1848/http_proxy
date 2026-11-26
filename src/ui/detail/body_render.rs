//! Draws prepared body text and trees and manages widget selection and scrolling.

mod layout;
mod view_cache;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use eframe::egui::{self, text::LayoutJob};
use http_inspector_content::format::{BodyFormat, Notice, NoticeLevel, ViewMode};
use http_inspector_exchange::BodySide;

use crate::ui::detail::body_cache::BodySource;
use crate::ui::detail::body_matches::Hit;
use crate::ui::detail::tree_view::TreeView;
use crate::ui::style::theme::Palette;
use crate::ui::style::tokens::Layout;

pub(crate) use view_cache::BodyViewCache;
use view_cache::{Content, RenderStyle};

impl BodyViewCache {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: &BodySource<'_>,
        mode: ViewMode,
    ) -> Option<String> {
        let style = RenderStyle {
            theme: ui.ctx().theme(),
            font: egui::TextStyle::Monospace.resolve(ui.style()),
        };
        let side = self.side(source.side);
        if side.update(ui.ctx(), source, mode, &style) {
            reset_text_selection(ui, source.side);
        }
        render(
            ui,
            side.shown.as_mut()?,
            source.side,
            mode,
            &style,
            side.tree.as_mut(),
        )
    }
}

fn render(
    ui: &mut egui::Ui,
    content: &mut Content,
    side: BodySide,
    mode: ViewMode,
    style: &RenderStyle,
    tree: Option<&mut TreeView>,
) -> Option<String> {
    let (format, notices, job, lines, matches, galley) = match content {
        Content::Unavailable(message) => {
            ui.colored_label(Palette::of(ui).warning, message.as_str());
            return None;
        }
        Content::Tree { format, notices } => {
            render_header(ui, *format, notices, mode);
            return ui
                .push_id(("body-tree", side), |ui| {
                    tree.and_then(|view| view.show(ui, style.theme, &style.font))
                })
                .inner;
        }
        Content::Ready {
            format,
            notices,
            job,
            lines,
            matches,
            galley,
        } => (*format, &*notices, job, lines, matches, galley),
    };
    render_header(ui, format, notices, mode);
    let scroll = matches.as_mut().and_then(|marked| {
        if marked.matches.controls(ui) {
            (*job, *lines) = marked.matches.mark(&marked.job, marked.lines.as_deref());
            *galley = None;
        }
        marked.matches.take_scroll()
    });
    if let Some(lines) = lines {
        render_numbered_lines(ui, lines, side, scroll.map(|hit| hit.line));
        return None;
    }
    let mut text = job.text.as_str();
    let mut layouter = |ui: &egui::Ui, _: &dyn egui::TextBuffer, wrap_width: f32| {
        let width = wrap_width.to_bits();
        match galley {
            Some((cached, galley)) if *cached == width => Arc::clone(galley),
            _ => {
                let laid_out = layout::wrapped_galley(job, wrap_width, |wrapped| {
                    ui.ctx().fonts_mut(|fonts| fonts.layout_job(wrapped))
                });
                *galley = Some((width, Arc::clone(&laid_out)));
                laid_out
            }
        }
    };
    let output = egui::TextEdit::multiline(&mut text)
        .id(body_text_id(ui, side))
        .font(egui::TextStyle::Monospace)
        .desired_rows(8)
        .desired_width(f32::INFINITY)
        .layouter(&mut layouter)
        .show(ui);
    if let Some(hit) = scroll {
        scroll_to_hit(ui, &output, &job.text, &hit);
    }
    None
}

/// Persistent widget id for one side's non-numbered read-only body text view.
fn body_text_id(ui: &egui::Ui, side: BodySide) -> egui::Id {
    ui.make_persistent_id(("body-text", side))
}

/// Clears cached selection state and focus for `side` so switching records or
/// view modes never carries stale cursor offsets into different text.
fn reset_text_selection(ui: &egui::Ui, side: BodySide) {
    let id = body_text_id(ui, side);
    ui.ctx().data_mut(|data| {
        data.remove::<egui::text_edit::TextEditState>(id);
    });
    ui.memory_mut(|memory| {
        if memory.has_focus(id) {
            memory.surrender_focus(id);
        }
    });
}

/// Scrolls the enclosing scroll area so the match `hit` in `text` is in view.
fn scroll_to_hit(ui: &egui::Ui, output: &egui::text_edit::TextEditOutput, text: &str, hit: &Hit) {
    let characters = text
        .get(..hit.range.start)
        .map_or(0, |prefix| prefix.chars().count());
    let rect = output
        .galley
        .pos_from_cursor(egui::text::CCursor::new(characters))
        .translate(output.galley_pos.to_vec2());
    ui.scroll_to_rect(rect, Some(egui::Align::Center));
}

/// The format and view mode, then each notice in order.
fn render_header(ui: &mut egui::Ui, format: BodyFormat, notices: &[Notice], mode: ViewMode) {
    ui.weak(format!("Format: {} · {}", format.label(), mode.label()));
    for notice in notices {
        match notice.level {
            NoticeLevel::Info => {
                ui.weak(notice.text.as_str());
            }
            NoticeLevel::Warning => {
                ui.colored_label(Palette::of(ui).warning, notice.text.as_str());
            }
        }
    }
}

/// Draws `lines` with a line number gutter, laying out only the rows that
/// are scrolled into view. Long lines scroll sideways instead of wrapping so
/// each row is one source line.
fn render_numbered_lines(
    ui: &mut egui::Ui,
    lines: &[LayoutJob],
    side: BodySide,
    scroll_to: Option<usize>,
) {
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let digits = lines.len().to_string().len();
    let mut area = egui::ScrollArea::both()
        .id_salt(("body-lines", side))
        .max_height(Layout::BODY_LINE_VIEW_HEIGHT)
        .auto_shrink([false, true]);
    if let Some(line) = scroll_to {
        // `show_rows` places rows `row_height` plus item spacing apart.
        let spacing = ui.spacing().item_spacing.y;
        #[expect(
            clippy::cast_precision_loss,
            reason = "line counts of an 8 KiB preview fit in f32"
        )]
        let top = line as f32 * (row_height + spacing);
        area = area.vertical_scroll_offset((top - Layout::BODY_LINE_VIEW_HEIGHT / 2.0).max(0.0));
    }
    area.show_rows(ui, row_height, lines.len(), |ui, rows| {
        for number in rows {
            let Some(line) = lines.get(number) else {
                break;
            };
            // `horizontal` would pad rows to the button height and break
            // the fixed row height `show_rows` relies on.
            let row_size = egui::vec2(ui.available_width(), row_height);
            let layout = egui::Layout::left_to_right(egui::Align::Min);
            ui.allocate_ui_with_layout(row_size, layout, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!("{:>digits$}", number + 1))
                            .font(font.clone())
                            .weak(),
                    )
                    .selectable(false),
                );
                ui.add(egui::Label::new(line.clone()).wrap_mode(egui::TextWrapMode::Extend));
            });
        }
    });
}
