//! The Headers tab: query parameters, request and response headers, and
//! tunnel details as one key-value tree (D24).

use eframe::egui;
use http_inspector_analysis::filter::FilterKey;
use http_inspector_content::format::form;
use http_inspector_exchange::http_semantics::{REDACTED_VALUE, urlencoded};
use http_inspector_exchange::{Exchange, HeaderPreview, Outcome, TunnelPreview};

use crate::ui::AppAction;
use crate::ui::detail::search::{Query, highlight};
use crate::ui::i18n::{Language, domain};
use crate::ui::requests::filter_menu;
use crate::ui::requests::outcome_view;
use crate::ui::style::fonts;
use crate::ui::style::theme::{MatchColors, Palette};
use crate::ui::workbench::{text, tree};

/// Shows the groups of `record`. `search` marks matches in header names and
/// values when the active search covers headers. Each header row offers
/// filter terms in its context menu; returns the one the user picked.
pub(super) fn show(
    ui: &mut egui::Ui,
    record: &Exchange,
    search: Option<&Query>,
    field_menu_enabled: bool,
    lang: Language,
) -> Option<AppAction> {
    let rows = Rows {
        key_width: tree::key_width(ui.available_width()),
        search,
        field_menu_enabled,
        lang,
        filter: std::cell::Cell::new(None),
    };
    let request = record.request();
    if let Some(query) = urlencoded::split_target(request.target()).query {
        let pairs = form::pairs(query);
        if !pairs.is_empty() {
            tree::group(
                ui,
                "inspector-query",
                lang.detail().query,
                Some(pairs.len()),
                |ui| {
                    for (key, value) in &pairs {
                        let _ = rows.pair(ui, &form::visible(key), &form::visible(value), false);
                    }
                },
            );
        }
    }
    rows.headers(
        ui,
        ("inspector-request", FilterKey::RequestHeader),
        lang.shared().request,
        request.headers(),
    );
    match record.outcome() {
        Outcome::Response(response) => {
            rows.headers(
                ui,
                ("inspector-response", FilterKey::ResponseHeader),
                lang.shared().response,
                response.headers(),
            );
        }
        Outcome::Tunnel(tunnel) => tunnel_group(ui, &rows, tunnel),
        _ => {}
    }
    rows.filter.take()
}

/// How the key-value rows of one record are drawn.
struct Rows<'a> {
    key_width: f32,
    search: Option<&'a Query>,
    field_menu_enabled: bool,
    lang: Language,
    /// The filter term picked from a header row's context menu.
    filter: std::cell::Cell<Option<AppAction>>,
}

impl Rows<'_> {
    fn headers(
        &self,
        ui: &mut egui::Ui,
        (id, key): (&str, FilterKey),
        title: &str,
        headers: &HeaderPreview,
    ) {
        let entries = headers.entries();
        let lang = self.lang;
        tree::group(ui, id, title, Some(entries.len()), |ui| {
            for header in entries {
                let masked = header.value() == REDACTED_VALUE;
                let row = self.pair(ui, header.name(), header.value(), masked);
                row.context_menu(|ui| {
                    let term = format!("{}={}", header.name(), header.value());
                    if let Some(action) =
                        filter_menu::items(ui, key, &term, self.field_menu_enabled, lang)
                    {
                        self.filter.set(Some(action));
                    }
                });
            }
            if headers.omitted() {
                tree::row(ui, tree::TreeRow::new(1, ""), |ui| {
                    ui.label(
                        egui::RichText::new(lang.detail().headers_omitted)
                            .color(Palette::of(ui).warning),
                    );
                });
            }
        });
    }

    /// One key-value row at depth 1. A masked value is `text.secondary`, so
    /// it reads as a placeholder rather than data.
    fn pair(&self, ui: &mut egui::Ui, key: &str, value: &str, masked: bool) -> egui::Response {
        let palette = Palette::of(ui);
        let value_color = if masked {
            palette.secondary
        } else {
            palette.text
        };
        // The row takes clicks so that its context menu offers filter terms;
        // drags still select the text inside.
        tree::row(
            ui,
            tree::TreeRow::new(1, key).clickable(),
            |ui| match self.search {
                Some(query) => {
                    let colors = MatchColors::for_theme(ui.ctx().theme());
                    let key_job = highlight::plain(
                        key,
                        &query.terms,
                        fonts::control(),
                        palette.secondary,
                        colors,
                    );
                    let value_job =
                        highlight::plain(value, &query.terms, fonts::data(), value_color, colors);
                    tree::key_value(ui, self.key_width, key_job, value_job);
                }
                None => tree::key_value(
                    ui,
                    self.key_width,
                    tree::key_text(ui, key),
                    tree::value_text(ui, value).color(value_color),
                ),
            },
        )
        .response
    }
}

/// What a CONNECT tunnel recorded: its state, bytes each way, and duration.
fn tunnel_group(ui: &mut egui::Ui, rows: &Rows<'_>, tunnel: &TunnelPreview) {
    let lang = rows.lang;
    let detail = lang.detail();
    let snapshot = tunnel.snapshot();
    let duration = text::milliseconds(outcome_view::elapsed_millis(snapshot.duration()));
    let pairs = [
        (
            detail.tunnel_state,
            domain::tunnel_state(lang, snapshot.state()),
        ),
        (
            detail.tunnel_sent,
            text::byte_count(snapshot.client_to_upstream()),
        ),
        (
            detail.tunnel_received,
            text::byte_count(snapshot.upstream_to_client()),
        ),
        (detail.duration, duration),
    ];
    let header = tree::group(ui, "inspector-tunnel", lang.shared().tunnel, None, |ui| {
        for (key, value) in &pairs {
            let _ = rows.pair(ui, key, value, false);
        }
    });
    header.on_hover_text(lang.shared().tooltips.tunnel_hint);
}
