//! Theme colors for [`TokenKind`]s and conversion of [`Styled`] text into an
//! egui [`LayoutJob`].

use crate::ui::style::theme::Theme;
use eframe::egui::{self, Color32, FontId, TextFormat, text::LayoutJob};
use http_inspector_content::format::{LineIndent, Styled, TokenKind};
use std::collections::HashMap;

/// Syntax colors that keep at least 4.5:1 contrast on the panel, text field,
/// stripe, hover, and selection backgrounds of their theme. Color only adds
/// to cues already in the text: keys end with `:` or `=`, strings and
/// attribute values keep their quotes, tags keep their angle brackets,
/// namespace prefixes end with `:`, entities keep `&` and `;`, `CDATA` and
/// instructions keep their `<![CDATA[` and `<?` delimiters, and comments are
/// also italic.
#[derive(Clone, Copy, Debug)]
pub(super) struct SyntaxPalette {
    pub(super) key: Color32,
    pub(super) string: Color32,
    pub(super) number: Color32,
    pub(super) literal: Color32,
    pub(super) punctuation: Color32,
    pub(super) tag: Color32,
    pub(super) comment: Color32,
}

impl SyntaxPalette {
    pub(super) const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self {
                key: Color32::from_rgb(120, 30, 125),
                string: Color32::from_rgb(0, 95, 35),
                number: Color32::from_rgb(0, 70, 160),
                literal: Color32::from_rgb(150, 45, 0),
                punctuation: Color32::from_gray(75),
                tag: Color32::from_rgb(140, 20, 55),
                comment: Color32::from_gray(80),
            },
            Theme::Dark => Self {
                key: Color32::from_rgb(230, 175, 255),
                string: Color32::from_rgb(150, 230, 160),
                number: Color32::from_rgb(150, 205, 255),
                literal: Color32::from_rgb(255, 195, 135),
                punctuation: Color32::from_gray(190),
                tag: Color32::from_rgb(255, 165, 185),
                comment: Color32::from_gray(175),
            },
        }
    }

    const fn color(&self, kind: TokenKind, text: Color32) -> Color32 {
        match kind {
            TokenKind::Text => text,
            TokenKind::Key | TokenKind::AttributeName => self.key,
            TokenKind::String | TokenKind::AttributeValue | TokenKind::CData => self.string,
            TokenKind::Number | TokenKind::Entity => self.number,
            TokenKind::Literal | TokenKind::Namespace => self.literal,
            TokenKind::Punctuation => self.punctuation,
            TokenKind::Tag => self.tag,
            TokenKind::Comment | TokenKind::Instruction => self.comment,
        }
    }
}

/// Caches measured logical widths of alignment prefixes for one layout build.
struct PrefixWidths<'a> {
    ctx: &'a egui::Context,
    font: &'a FontId,
    cache: HashMap<&'a str, f32>,
}

impl<'a> PrefixWidths<'a> {
    fn new(ctx: &'a egui::Context, font: &'a FontId) -> Self {
        Self {
            ctx,
            font,
            cache: HashMap::new(),
        }
    }

    fn width(&mut self, prefix: &'a str) -> f32 {
        if prefix.is_empty() {
            return 0.0;
        }
        if let Some(&cached) = self.cache.get(prefix) {
            return cached;
        }
        let measured = self.ctx.fonts_mut(|fonts| {
            let galley =
                fonts.layout_no_wrap(prefix.to_owned(), self.font.clone(), Color32::TRANSPARENT);
            galley
                .pos_from_cursor(egui::text::CCursor::new(prefix.chars().count()))
                .min
                .x
        });
        self.cache.insert(prefix, measured);
        measured
    }
}

/// Builds the highlighted layout once; callers cache it per selection.
/// Visual-only continuation indents on `styled` become section `leading_space`
/// so [`LayoutJob::text`] stays free of display padding.
pub(super) fn layout_job(
    ctx: &egui::Context,
    styled: &Styled,
    palette: &SyntaxPalette,
    text_color: Color32,
    font_id: &FontId,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    let indents = styled.indents();
    let mut widths = PrefixWidths::new(ctx, font_id);
    let mut indent_index = 0;
    let full = styled.text();
    for (range, kind) in styled.span_ranges() {
        let format = TextFormat {
            font_id: font_id.clone(),
            color: palette.color(kind, text_color),
            italics: kind == TokenKind::Comment,
            ..TextFormat::default()
        };
        let mut cursor = range.start;
        while let Some(indent) = indents.get(indent_index) {
            if indent.offset < cursor {
                indent_index += 1;
                continue;
            }
            if indent.offset >= range.end {
                break;
            }
            if indent.offset > cursor
                && let Some(piece) = full.get(cursor..indent.offset)
            {
                job.append(piece, 0.0, format.clone());
                cursor = indent.offset;
            }
            let leading = widths.width(&indent.align_to);
            indent_index += 1;
            let next_stop = indents
                .get(indent_index)
                .map_or(range.end, |next| next.offset.min(range.end));
            if let Some(piece) = full.get(cursor..next_stop) {
                job.append(piece, leading, format.clone());
            }
            cursor = next_stop;
        }
        if cursor < range.end
            && let Some(piece) = full.get(cursor..range.end)
        {
            job.append(piece, 0.0, format);
        }
    }
    job
}

/// Builds one layout per line of `styled`, without the line ends, so a
/// view can lay out and draw only the visible lines while preserving visual
/// continuation indentation.
pub(super) fn line_jobs(
    ctx: &egui::Context,
    styled: &Styled,
    palette: &SyntaxPalette,
    text_color: Color32,
    font_id: &FontId,
) -> Vec<LayoutJob> {
    let mut lines = vec![LayoutJob::default()];
    let indents = styled.indents();
    let mut widths = PrefixWidths::new(ctx, font_id);
    let mut indent_index = 0;
    let mut pending_indent = 0.0_f32;
    let full = styled.text();
    for (range, kind) in styled.span_ranges() {
        let Some(piece) = full.get(range.clone()) else {
            continue;
        };
        let format = TextFormat {
            font_id: font_id.clone(),
            color: palette.color(kind, text_color),
            italics: kind == TokenKind::Comment,
            ..TextFormat::default()
        };
        let mut pos = range.start;
        for (index, part) in piece.split('\n').enumerate() {
            if index > 0 {
                lines.push(LayoutJob::default());
                pending_indent = indent_at(indents, &mut indent_index, pos, &mut widths);
            }
            let clean = part.strip_suffix('\r').unwrap_or(part);
            if let Some(line) = lines.last_mut()
                && !clean.is_empty()
            {
                let leading = std::mem::take(&mut pending_indent);
                line.append(clean, leading, format.clone());
            }
            pos = pos.saturating_add(part.len()).saturating_add(1);
        }
    }
    lines
}

fn indent_at<'a>(
    indents: &'a [LineIndent],
    indent_index: &mut usize,
    offset: usize,
    widths: &mut PrefixWidths<'a>,
) -> f32 {
    while let Some(indent) = indents.get(*indent_index) {
        match indent.offset.cmp(&offset) {
            std::cmp::Ordering::Less => *indent_index += 1,
            std::cmp::Ordering::Equal => {
                *indent_index += 1;
                return widths.width(&indent.align_to);
            }
            std::cmp::Ordering::Greater => break,
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::style::theme;

    #[test]
    fn syntax_colors_meet_aa_on_all_widget_backgrounds() {
        for theme in [Theme::Light, Theme::Dark] {
            let palette = SyntaxPalette::for_theme(theme);
            for color in [
                palette.key,
                palette.string,
                palette.number,
                palette.literal,
                palette.punctuation,
                palette.tag,
                palette.comment,
            ] {
                theme::assert_readable(theme, color);
            }
        }
    }

    #[test]
    fn splits_layout_into_lines_with_their_colors() {
        let ctx = egui::Context::default();
        let mut styled = Styled::default();
        styled.push("/* a\r\nb */", TokenKind::Comment);
        styled.push("\n\nc", TokenKind::Text);
        let palette = SyntaxPalette::for_theme(Theme::Light);
        let lines = line_jobs(
            &ctx,
            &styled,
            &palette,
            Color32::BLACK,
            &FontId::monospace(12.0),
        );
        let texts: Vec<_> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["/* a", "b */", "", "c"]);
        assert!(
            lines
                .get(1)
                .and_then(|line| line.sections.first())
                .is_some_and(|section| section.format.italics)
        );
    }

    #[test]
    fn layout_keeps_text_and_marks_comments() {
        let ctx = egui::Context::default();
        let mut styled = Styled::default();
        styled.push("<!-- a -->", TokenKind::Comment);
        styled.push(" b", TokenKind::Text);
        let palette = SyntaxPalette::for_theme(Theme::Dark);
        let job = layout_job(
            &ctx,
            &styled,
            &palette,
            Color32::WHITE,
            &FontId::monospace(12.0),
        );
        assert_eq!(job.text, "<!-- a --> b");
        assert_eq!(job.sections.len(), 2);
        assert!(
            job.sections
                .first()
                .is_some_and(|section| section.format.italics)
        );
        assert_eq!(
            job.sections.get(1).map(|section| section.format.color),
            Some(Color32::WHITE)
        );
    }

    fn char_x(galley: &egui::Galley, text: &str, needle: &str) -> Result<f32, String> {
        let byte_index = text
            .find(needle)
            .ok_or_else(|| format!("needle {needle:?} not found in {text:?}"))?;
        let char_index = text
            .get(..byte_index)
            .ok_or_else(|| format!("invalid byte offset {byte_index}"))?
            .chars()
            .count();
        Ok(galley
            .pos_from_cursor(egui::text::CCursor::new(char_index))
            .min
            .x)
    }

    #[test]
    fn aligns_multiline_continuation_glyphs_to_value_column_without_changing_text()
    -> Result<(), String> {
        let ctx = egui::Context::default();
        let palette = SyntaxPalette::for_theme(Theme::Light);
        let font = FontId::monospace(13.0);
        let mut result: Result<(), String> = Ok(());

        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            result = (|| {
                let mut styled = Styled::default();
                styled.push("    <메모>", TokenKind::Tag);
                styled.push_multiline("첫째줄\n\n둘째줄\r\n셋째줄", TokenKind::Text, "    <메모>");
                styled.push("</메모>\n    <next>ok</next>", TokenKind::Tag);

                let job = layout_job(ui.ctx(), &styled, &palette, Color32::BLACK, &font);
                assert_eq!(job.text, styled.text());

                let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
                let x1 = char_x(&galley, styled.text(), "첫째줄")?;
                let x2 = char_x(&galley, styled.text(), "둘째줄")?;
                let x3 = char_x(&galley, styled.text(), "셋째줄")?;
                let x_next = char_x(&galley, styled.text(), "<next>")?;

                assert!(
                    x1 > x_next,
                    "value {x1} should be right of next tag {x_next}"
                );
                assert!((x1 - x2).abs() < 0.5, "x1={x1}, x2={x2}");
                assert!((x1 - x3).abs() < 0.5, "x1={x1}, x3={x3}");

                let lines = line_jobs(ui.ctx(), &styled, &palette, Color32::BLACK, &font);
                let line_texts: Vec<_> = lines.iter().map(|line| line.text.as_str()).collect();
                assert_eq!(
                    line_texts,
                    [
                        "    <메모>첫째줄",
                        "",
                        "둘째줄",
                        "셋째줄</메모>",
                        "    <next>ok</next>",
                    ]
                );
                let first_line = lines.first().ok_or("missing line 0")?;
                let second_line = lines.get(2).ok_or("missing line 2")?;
                let third_line = lines.get(3).ok_or("missing line 3")?;
                let g0 = ui.fonts_mut(|fonts| fonts.layout_job(first_line.clone()));
                let g2 = ui.fonts_mut(|fonts| fonts.layout_job(second_line.clone()));
                let g3 = ui.fonts_mut(|fonts| fonts.layout_job(third_line.clone()));
                let first_x = char_x(&g0, &first_line.text, "첫째줄")?;
                let second_x = char_x(&g2, &second_line.text, "둘째줄")?;
                let third_x = char_x(&g3, &third_line.text, "셋째줄")?;
                assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
                assert!((first_x - third_x).abs() < 0.5, "{first_x} vs {third_x}");
                Ok(())
            })();
        });
        output.textures_delta.clear();
        result
    }
}
