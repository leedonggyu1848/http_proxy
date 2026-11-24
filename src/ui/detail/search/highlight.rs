//! Backgrounds that mark search matches in laid-out text.

use std::ops::Range;

use eframe::egui::text::{ByteIndex, LayoutJob, LayoutSection, TextFormat};
use eframe::egui::{Color32, FontId};
use http_proxy::analysis::search::Matcher;

use crate::ui::style::theme::Theme;

/// Background of every match, and of the match the user moved to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MatchColors {
    pub(crate) other: Color32,
    pub(crate) current: Color32,
}

impl MatchColors {
    /// Pale backgrounds in light mode and dark ones in dark mode, so the
    /// syntax colors of each theme stay readable on them.
    pub(crate) const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self {
                other: Color32::from_rgb(255, 236, 150),
                current: Color32::from_rgb(255, 196, 110),
            },
            Theme::Dark => Self {
                other: Color32::from_rgb(92, 72, 0),
                current: Color32::from_rgb(150, 82, 0),
            },
        }
    }
}

/// Byte ranges of the matches in `text`, on character boundaries.
pub(crate) fn ranges(matcher: &Matcher, text: &str) -> Vec<Range<usize>> {
    matcher
        .find_all(text.as_bytes())
        .into_iter()
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
        .collect()
}

/// Gives the bytes of `ranges` in `job` a background; the range at index
/// `current` gets the stronger one. `ranges` must be sorted and disjoint.
pub(crate) fn mark(
    job: &mut LayoutJob,
    ranges: &[Range<usize>],
    current: Option<usize>,
    colors: MatchColors,
) {
    if ranges.is_empty() {
        return;
    }
    let mut marked = Vec::with_capacity(job.sections.len() + ranges.len() * 2);
    let mut next = 0;
    for section in std::mem::take(&mut job.sections) {
        let end = section.byte_range.end.0;
        let mut start = section.byte_range.start.0;
        let mut leading_space = section.leading_space;
        let mut push = |range: Range<usize>, format: TextFormat| {
            if !range.is_empty() {
                marked.push(LayoutSection {
                    leading_space: std::mem::take(&mut leading_space),
                    byte_range: ByteIndex(range.start)..ByteIndex(range.end),
                    format,
                });
            }
        };
        while let Some(range) = ranges.get(next) {
            if range.start >= end {
                break;
            }
            let from = range.start.max(start);
            let to = range.end.min(end);
            push(start..from, section.format.clone());
            let mut format = section.format.clone();
            format.background = if current == Some(next) {
                colors.current
            } else {
                colors.other
            };
            push(from..to, format);
            start = to;
            if range.end > end {
                break;
            }
            next += 1;
        }
        push(start..end, section.format);
    }
    job.sections = marked;
}

/// One line of plain text with its matches marked.
pub(crate) fn plain(
    text: &str,
    matcher: &Matcher,
    font: FontId,
    color: Color32,
    colors: MatchColors,
) -> LayoutJob {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    mark(&mut job, &ranges(matcher, text), None, colors);
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_proxy::analysis::search::MatchOptions;

    fn sections(job: &LayoutJob) -> Vec<(&str, bool)> {
        job.sections
            .iter()
            .map(|section| {
                (
                    job.text
                        .get(section.byte_range.start.0..section.byte_range.end.0)
                        .unwrap_or_default(),
                    section.format.background != Color32::TRANSPARENT,
                )
            })
            .collect()
    }

    #[test]
    fn marks_matches_that_cross_styled_sections() -> Result<(), Box<dyn std::error::Error>> {
        let matcher = Matcher::new("ab", MatchOptions::default())?;
        let mut job = LayoutJob::default();
        // Different colors keep the pieces in separate sections.
        for (piece, gray) in [("xa", 10), ("by", 20), ("AB", 30)] {
            let format = TextFormat {
                color: Color32::from_gray(gray),
                ..TextFormat::default()
            };
            job.append(piece, 0.0, format);
        }
        let ranges = ranges(&matcher, &job.text);
        assert_eq!(ranges, [1..3, 4..6]);
        let colors = MatchColors::for_theme(Theme::Light);
        mark(&mut job, &ranges, Some(1), colors);
        assert_eq!(
            sections(&job),
            [
                ("x", false),
                ("a", true),
                ("b", true),
                ("y", false),
                ("AB", true)
            ]
        );
        assert_eq!(
            job.sections.last().map(|section| section.format.background),
            Some(colors.current)
        );
        Ok(())
    }

    #[test]
    fn plain_text_keeps_non_ascii_boundaries() -> Result<(), Box<dyn std::error::Error>> {
        let matcher = Matcher::new("주문", MatchOptions::default())?;
        let job = plain(
            "새 주문",
            &matcher,
            FontId::monospace(12.0),
            Color32::WHITE,
            MatchColors::for_theme(Theme::Dark),
        );
        assert_eq!(sections(&job), [("새 ", false), ("주문", true)]);
        Ok(())
    }
}
