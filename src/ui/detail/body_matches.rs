//! Search matches in a shown body: where they are, which one the user moved
//! to, and the controls that move between them.

use std::ops::Range;

use eframe::egui::{self, text::LayoutJob};
use http_proxy::analysis::search::Matcher;

use crate::ui::detail::search::highlight::{self, MatchColors};

/// One match: the line it is on (0 for text shown without line numbers)
/// and its byte range in that line or text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Hit {
    pub(super) line: usize,
    pub(super) range: Range<usize>,
}

/// Matches of the active query in the text a body view shows.
#[derive(Debug)]
pub(super) struct BodyMatches {
    hits: Vec<Hit>,
    current: usize,
    /// The view should scroll the current match into sight.
    scroll: bool,
    colors: MatchColors,
}

impl BodyMatches {
    /// Finds matches in `lines` when the text is shown line by line, or in
    /// `job` otherwise. Matches across line ends are not marked in a line
    /// view.
    pub(super) fn find(
        matcher: &Matcher,
        job: &LayoutJob,
        lines: Option<&[LayoutJob]>,
        colors: MatchColors,
    ) -> Self {
        let hits = lines.map_or_else(
            || hits_in(0, matcher, &job.text),
            |lines| {
                lines
                    .iter()
                    .enumerate()
                    .flat_map(|(line, job)| hits_in(line, matcher, &job.text))
                    .collect()
            },
        );
        Self {
            scroll: !hits.is_empty(),
            hits,
            current: 0,
            colors,
        }
    }

    #[cfg(test)]
    pub(super) const fn len(&self) -> usize {
        self.hits.len()
    }

    /// Returns `job` and `lines` with every match marked.
    pub(super) fn mark(
        &self,
        job: &LayoutJob,
        lines: Option<&[LayoutJob]>,
    ) -> (LayoutJob, Option<Vec<LayoutJob>>) {
        let current = Some(self.current);
        let Some(lines) = lines else {
            let mut job = job.clone();
            let ranges: Vec<_> = self.hits.iter().map(|hit| hit.range.clone()).collect();
            highlight::mark(&mut job, &ranges, current, self.colors);
            return (job, None);
        };
        let mut marked = lines.to_vec();
        let mut index = 0;
        while let Some(first) = self.hits.get(index) {
            let line = first.line;
            let end = self
                .hits
                .iter()
                .skip(index)
                .take_while(|hit| hit.line == line)
                .count()
                + index;
            let ranges: Vec<_> = self
                .hits
                .get(index..end)
                .unwrap_or_default()
                .iter()
                .map(|hit| hit.range.clone())
                .collect();
            let local = self
                .current
                .checked_sub(index)
                .filter(|local| *local < ranges.len());
            if let Some(job) = marked.get_mut(line) {
                highlight::mark(job, &ranges, local, self.colors);
            }
            index = end;
        }
        (job.clone(), Some(marked))
    }

    /// The current match, once, after it changed, so the view can scroll
    /// to it.
    pub(super) fn take_scroll(&mut self) -> Option<Hit> {
        if !std::mem::take(&mut self.scroll) {
            return None;
        }
        self.hits.get(self.current).cloned()
    }

    /// Shows the position among matches with Previous and Next buttons.
    /// Returns `true` when the current match changed.
    pub(super) fn controls(&mut self, ui: &mut egui::Ui) -> bool {
        let count = self.hits.len();
        if count == 0 {
            ui.weak("No search matches in the shown text.");
            return false;
        }
        let mut step = None;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Match {} of {count}", self.current + 1));
            if ui
                .small_button("◀ Previous")
                .on_hover_text("Previous match")
                .clicked()
            {
                step = Some(count - 1);
            }
            if ui
                .small_button("Next ▶")
                .on_hover_text("Next match")
                .clicked()
            {
                step = Some(1);
            }
        });
        let Some(step) = step else {
            return false;
        };
        self.current = (self.current + step) % count;
        self.scroll = true;
        true
    }

    #[cfg(test)]
    pub(super) fn step(&mut self) {
        self.current = (self.current + 1) % self.hits.len().max(1);
        self.scroll = true;
    }
}

fn hits_in(line: usize, matcher: &Matcher, text: &str) -> Vec<Hit> {
    highlight::ranges(matcher, text)
        .into_iter()
        .map(|range| Hit { line, range })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::style::theme::Theme;
    use eframe::egui::text::TextFormat;
    use http_proxy::analysis::search::MatchOptions;

    fn job(text: &str) -> LayoutJob {
        let mut job = LayoutJob::default();
        job.append(text, 0.0, TextFormat::default());
        job
    }

    fn backgrounds(job: &LayoutJob) -> Vec<egui::Color32> {
        job.sections
            .iter()
            .map(|section| section.format.background)
            .filter(|color| *color != egui::Color32::TRANSPARENT)
            .collect()
    }

    #[test]
    fn moves_between_matches_across_lines() -> Result<(), Box<dyn std::error::Error>> {
        let query = Matcher::new("id", MatchOptions::default())?;
        let colors = MatchColors::for_theme(Theme::Light);
        let lines = [job("id id"), job("none"), job("ID")];
        let mut matches = BodyMatches::find(&query, &job(""), Some(&lines), colors);
        assert_eq!(matches.len(), 3);
        assert_eq!(matches.take_scroll().map(|hit| hit.line), Some(0));
        assert_eq!(matches.take_scroll(), None);

        matches.step();
        matches.step();
        assert_eq!(
            matches.take_scroll(),
            Some(Hit {
                line: 2,
                range: 0..2
            })
        );
        let (_, marked) = matches.mark(&job(""), Some(&lines));
        let marked = marked.unwrap_or_default();
        assert_eq!(
            marked.iter().map(backgrounds).collect::<Vec<_>>(),
            [
                vec![colors.other, colors.other],
                vec![],
                vec![colors.current],
            ]
        );
        matches.step();
        assert_eq!(matches.take_scroll().map(|hit| hit.range), Some(0..2));
        Ok(())
    }

    #[test]
    fn marks_text_shown_without_line_numbers() -> Result<(), Box<dyn std::error::Error>> {
        let query = Matcher::new("b", MatchOptions::default())?;
        let colors = MatchColors::for_theme(Theme::Dark);
        let text = job("abcb");
        let matches = BodyMatches::find(&query, &text, None, colors);
        let (marked, lines) = matches.mark(&text, None);
        assert!(lines.is_none());
        assert_eq!(backgrounds(&marked), [colors.current, colors.other]);
        Ok(())
    }
}
