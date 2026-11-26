//! Cached body presentation keyed by the capture, mode, search, and font.

use std::sync::Arc;

use eframe::egui::{self, Galley, text::LayoutJob};
use http_inspector_analysis::search::Query;
use http_inspector_content::format::{BodyFormat, Notice, Styled, ViewMode};
use http_inspector_exchange::BodySide;

use crate::ui::detail::body_cache::{BodyCache, BodySource, PreparedBody};
use crate::ui::detail::body_matches::BodyMatches;
use crate::ui::detail::highlight;
use crate::ui::detail::tree_view::TreeView;
use crate::ui::style::theme::{self, MatchColors, SyntaxPalette, Theme};

#[derive(Clone, PartialEq)]
pub(super) struct RenderStyle {
    pub(super) theme: Theme,
    pub(super) font: egui::FontId,
}

pub(super) enum Content {
    Unavailable(String),
    Tree {
        format: BodyFormat,
        notices: Vec<Notice>,
    },
    Ready {
        format: BodyFormat,
        /// Shown above the text in order.
        notices: Vec<Notice>,
        job: LayoutJob,
        /// One layout per line for formats shown with line numbers.
        lines: Option<Vec<LayoutJob>>,
        /// Search matches, with `job` and `lines` before they were marked.
        matches: Option<Box<Marked>>,
        /// The last laid-out galley and the wrap width it was laid out for.
        galley: Option<(u32, Arc<Galley>)>,
    },
}

/// Search matches in a body view and the text before marking, so moving to
/// another match marks it again from the original.
pub(super) struct Marked {
    pub(super) matches: BodyMatches,
    pub(super) job: LayoutJob,
    pub(super) lines: Option<Vec<LayoutJob>>,
}

#[derive(Default)]
pub(super) struct SideView {
    pub(super) cache: BodyCache<RenderStyle>,
    pub(super) shown: Option<Content>,
    pub(super) tree: Option<TreeView>,
}

/// Body text and widget state for the request and response shown together.
#[derive(Default)]
pub(crate) struct BodyViewCache {
    pub(super) request: SideView,
    pub(super) response: SideView,
}

impl BodyViewCache {
    pub(super) const fn side(&mut self, side: BodySide) -> &mut SideView {
        match side {
            BodySide::Request => &mut self.request,
            BodySide::Response => &mut self.response,
        }
    }
}

impl SideView {
    /// Builds a new presentation when inputs change and reports when the
    /// old selection refers to different text.
    pub(super) fn update(
        &mut self,
        ctx: &egui::Context,
        source: &BodySource<'_>,
        mode: ViewMode,
        style: &RenderStyle,
    ) -> bool {
        let Some(update) = self.cache.update(source, mode, style) else {
            return false;
        };
        self.shown = Some(match update.content {
            PreparedBody::Unavailable(message) => Content::Unavailable(message),
            PreparedBody::Tree {
                format,
                notices,
                model,
            } => {
                if self.tree.as_ref().is_none_or(|view| !view.displays(&model)) {
                    self.tree = Some(TreeView::new(model));
                }
                Content::Tree { format, notices }
            }
            PreparedBody::Text {
                format,
                notices,
                styled,
            } => layout_text(ctx, style, format, notices, &styled, source.search),
        });
        update.reset_selection
    }
}

fn layout_text(
    ctx: &egui::Context,
    style: &RenderStyle,
    format: BodyFormat,
    notices: Vec<Notice>,
    styled: &Styled,
    search: Option<&Query>,
) -> Content {
    // The frame style still belongs to the previous theme after a theme switch.
    let text_color = theme::text_in_theme(ctx, style.theme);
    let palette = SyntaxPalette::for_theme(style.theme);
    let job = highlight::layout_job(ctx, styled, &palette, text_color, &style.font);
    let lines = format
        .spec()
        .numbered
        .then(|| highlight::line_jobs(ctx, styled, &palette, text_color, &style.font));
    let Some(query) = search else {
        return Content::Ready {
            format,
            notices,
            job,
            lines,
            matches: None,
            galley: None,
        };
    };
    let matches = BodyMatches::find(
        &query.terms,
        &job,
        lines.as_deref(),
        MatchColors::for_theme(style.theme),
    );
    let (marked_job, marked_lines) = matches.mark(&job, lines.as_deref());
    Content::Ready {
        format,
        notices,
        job: marked_job,
        lines: marked_lines,
        matches: Some(Box::new(Marked {
            matches,
            job,
            lines,
        })),
        galley: None,
    }
}
