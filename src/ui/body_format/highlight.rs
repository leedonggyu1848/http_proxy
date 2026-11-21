//! Theme colors for [`TokenKind`]s and conversion of [`Styled`] text into an
//! egui [`LayoutJob`].

use super::{Styled, TokenKind};
use crate::ui::theme::Theme;
use eframe::egui::{Color32, FontId, TextFormat, text::LayoutJob};

/// Syntax colors that keep at least 4.5:1 contrast on the panel, text field,
/// stripe, hover, and selection backgrounds of their theme. Color only adds
/// to cues already in the text: keys end with `:` or `=`, strings and
/// attribute values keep their quotes, tags keep their angle brackets,
/// namespace prefixes end with `:`, entities keep `&` and `;`, `CDATA` and
/// instructions keep their `<![CDATA[` and `<?` delimiters, and comments are
/// also italic.
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct SyntaxPalette {
    pub(in crate::ui) key: Color32,
    pub(in crate::ui) string: Color32,
    pub(in crate::ui) number: Color32,
    pub(in crate::ui) literal: Color32,
    pub(in crate::ui) punctuation: Color32,
    pub(in crate::ui) tag: Color32,
    pub(in crate::ui) comment: Color32,
}

impl SyntaxPalette {
    pub(in crate::ui) const fn for_theme(theme: Theme) -> Self {
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

/// Builds the highlighted layout once; callers cache it per selection.
pub(in crate::ui) fn layout_job(
    styled: &Styled,
    palette: &SyntaxPalette,
    text_color: Color32,
    font_id: &FontId,
) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (piece, kind) in styled.pieces() {
        job.append(
            piece,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: palette.color(kind, text_color),
                italics: kind == TokenKind::Comment,
                ..TextFormat::default()
            },
        );
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme;

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
    fn layout_keeps_text_and_marks_comments() {
        let mut styled = Styled::default();
        styled.push("<!-- a -->", TokenKind::Comment);
        styled.push(" b", TokenKind::Text);
        let palette = SyntaxPalette::for_theme(Theme::Dark);
        let job = layout_job(&styled, &palette, Color32::WHITE, &FontId::monospace(12.0));
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
}
