//! Endpoint section of the Map detail pane: per-method request and response
//! structure for the selected path node.
//!
//! Privacy invariant: this view renders only names, types, formats and
//! counts from [`EndpointSchema`]. The schema holds no query, header,
//! cookie, credential or body value, so none can appear here.

use crate::ui::style::tokens::{Layout, Space};
use crate::ui::workbench::tree;
use std::fmt::Write as _;

use eframe::egui::{self, RichText};
use http::Method;
use http_inspector_analysis::endpoint_schema::{
    BodySchema, EndpointSchema, EndpointSchemas, FieldShape, ResponseKey, TypeCounts, ValueKind,
};
use http_inspector_analysis::url::{PathSegmentPattern, percent_decode_display};
use http_inspector_analysis::url_map::{NodeId, RequestScope, ViewNode, ViewNodeKey};

use super::MapAction;
use crate::ui::i18n::{self, Language, domain};
use crate::ui::style::theme::Palette;

/// Per-method endpoint aggregates merged for the selected node, rebuilt only
/// when the node, its folded raw nodes, or the aggregates change.
#[derive(Default)]
pub(super) struct EndpointCache {
    key: Option<ViewNodeKey>,
    raw_node_ids: Vec<NodeId>,
    version: Option<u64>,
    merged: Vec<(String, EndpointSchema)>,
}

impl EndpointCache {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn sync(
        &mut self,
        node: &ViewNode,
        schemas: &EndpointSchemas,
    ) -> &[(String, EndpointSchema)] {
        let fresh = self.key.as_ref() == Some(&node.key)
            && self.raw_node_ids == node.raw_node_ids
            && self.version == Some(schemas.version());
        if !fresh {
            self.key = Some(node.key.clone());
            self.raw_node_ids.clone_from(&node.raw_node_ids);
            self.version = Some(schemas.version());
            self.merged = schemas.merged(&node.raw_node_ids);
        }
        &self.merged
    }
}

/// Renders the endpoint group and returns a `Show in Requests` action.
pub(super) fn show(
    ui: &mut egui::Ui,
    node: &ViewNode,
    endpoints: &[(String, EndpointSchema)],
    skipped: u64,
    lang: Language,
) -> Option<MapAction> {
    let map_text = lang.map();
    let palette = Palette::of(ui);
    let mut action = None;
    tree::group(
        ui,
        "map-endpoints",
        map_text.endpoints,
        Some(endpoints.len()),
        |ui| {
            egui::Frame::new()
                .inner_margin(Layout::GROUP_BODY_MARGIN)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = Space::XS;
                    if skipped > 0 {
                        ui.label(
                            RichText::new(i18n::map::analysis_skipped(lang, skipped))
                                .color(palette.warning),
                        );
                    }
                    for (index, (method, schema)) in endpoints.iter().enumerate() {
                        let salt = format!("{}#{method}", node.key.display_url());
                        let title = i18n::map::endpoint_header(lang, method, schema.requests());
                        egui::CollapsingHeader::new(RichText::new(title).monospace().strong())
                            .id_salt(&salt)
                            .default_open(index == 0)
                            .show(ui, |ui| {
                                if let Some(clicked) =
                                    render_endpoint(ui, node, method, schema, &salt, lang)
                                {
                                    action = Some(clicked);
                                }
                            });
                    }
                });
        },
    )
    .on_hover_text(map_text.tooltips.endpoints_hint);
    action
}

fn render_endpoint(
    ui: &mut egui::Ui,
    node: &ViewNode,
    method: &str,
    schema: &EndpointSchema,
    salt: &str,
    lang: Language,
) -> Option<MapAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if let Ok(parsed) = Method::from_bytes(method.as_bytes()) {
            let label = i18n::map::show_method_in_requests(lang, method);
            if ui
                .small_button(label)
                .on_hover_text(lang.map().tooltips.show_method_in_requests_hint)
                .clicked()
            {
                action = Some(MapAction::ShowInRequests(RequestScope::endpoint(
                    node.key.clone(),
                    parsed,
                )));
            }
        }
    });
    render_evidence(ui, schema, lang);
    render_path_params(ui, &node.key, schema, lang);
    render_query(ui, schema, salt, lang);
    render_headers(ui, schema, salt, lang);
    render_auth(ui, schema, lang);
    render_request_bodies(ui, schema, salt, lang);
    render_responses(ui, schema, salt, lang);
    action
}

fn render_evidence(ui: &mut egui::Ui, schema: &EndpointSchema, lang: Language) {
    let palette = Palette::of(ui);
    let summary = i18n::map::endpoint_evidence(
        lang,
        schema.requests(),
        schema.structured_bodies(),
        schema.excluded_bodies(),
        schema.missing_responses(),
    );
    ui.label(RichText::new(summary).color(palette.secondary));
}

fn render_path_params(
    ui: &mut egui::Ui,
    key: &ViewNodeKey,
    schema: &EndpointSchema,
    lang: Language,
) {
    for (position, pattern) in key.path.iter().enumerate() {
        if *pattern != PathSegmentPattern::FoldedId {
            continue;
        }
        let kinds = schema.path_ids().get(&position).map_or_else(
            || "—".to_owned(),
            |kinds| {
                kinds
                    .iter()
                    .map(|(kind, count)| format!("{} ×{count}", domain::id_kind(lang, *kind)))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        ui.label(i18n::map::endpoint_path_param(lang, position + 1, &kinds));
    }
}

fn render_query(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    let map_text = lang.map();
    section_title(ui, map_text.endpoint_query_params);
    if schema.query().is_empty() {
        ui.weak(map_text.endpoint_none_observed);
        return;
    }
    let palette = Palette::of(ui);
    egui::Grid::new(format!("{salt}/query"))
        .num_columns(3)
        .spacing([Space::MD, Space::XXS])
        .show(ui, |ui| {
            for (name, stats) in schema.query() {
                ui.monospace(percent_decode_display(name));
                ui.weak(ratio(stats.seen(), schema.requests()));
                ui.label(types_text(stats.types(), &palette));
                ui.end_row();
            }
        });
    omitted_note(
        ui,
        schema.omitted_query(),
        map_text.endpoint_query_item,
        lang,
    );
}

fn render_headers(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    let map_text = lang.map();
    section_title(ui, map_text.endpoint_custom_headers);
    if schema.headers().is_empty() {
        ui.weak(map_text.endpoint_none_observed);
        return;
    }
    egui::Grid::new(format!("{salt}/headers"))
        .num_columns(2)
        .spacing([Space::MD, Space::XXS])
        .show(ui, |ui| {
            for (name, count) in schema.headers() {
                ui.monospace(name);
                ui.weak(ratio(*count, schema.requests()));
                ui.end_row();
            }
        });
    omitted_note(
        ui,
        schema.omitted_headers(),
        map_text.endpoint_header_item,
        lang,
    );
}

fn render_auth(ui: &mut egui::Ui, schema: &EndpointSchema, lang: Language) {
    let map_text = lang.map();
    section_title(ui, map_text.endpoint_auth);
    if schema.auth().is_empty() {
        ui.weak(map_text.endpoint_no_credentials);
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (scheme, count) in schema.auth() {
            ui.label(format!("{scheme} {}", ratio(*count, schema.requests())));
        }
    });
}

fn render_request_bodies(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    let map_text = lang.map();
    section_title(ui, map_text.endpoint_request_body);
    if schema.request_bodies().is_empty() {
        ui.weak(map_text.endpoint_no_request_bodies);
    }
    for (content_type, group) in schema.request_bodies() {
        let title = i18n::map::endpoint_request_body_title(lang, content_type, group.bodies());
        render_body_group(
            ui,
            RichText::new(title),
            &format!("{salt}/req/{content_type}"),
            group,
            lang,
        );
    }
    if schema.requests_without_body() > 0 && !schema.request_bodies().is_empty() {
        ui.weak(i18n::map::endpoint_requests_without_body(
            lang,
            schema.requests_without_body(),
        ));
    }
    omitted_note(
        ui,
        schema.omitted_groups(),
        map_text.endpoint_body_group_item,
        lang,
    );
}

fn render_responses(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    let map_text = lang.map();
    section_title(ui, map_text.endpoint_responses);
    if schema.responses().is_empty() {
        ui.weak(map_text.endpoint_no_responses);
        return;
    }
    let palette = Palette::of(ui);
    for (key, group) in schema.responses() {
        let title_text =
            i18n::map::endpoint_response_title(lang, key.status, &key.content_type, group.bodies());
        let title = RichText::new(title_text).color(status_color(key, &palette));
        render_body_group(
            ui,
            title,
            &format!("{salt}/res/{}/{}", key.status, key.content_type),
            group,
            lang,
        );
    }
}

fn render_body_group(
    ui: &mut egui::Ui,
    title: RichText,
    salt: &str,
    group: &BodySchema,
    lang: Language,
) {
    egui::CollapsingHeader::new(title)
        .id_salt(salt)
        .default_open(true)
        .show(ui, |ui| {
            render_body_evidence(ui, group, lang);
            let root = group.fields();
            if root.seen() > 0 {
                if root
                    .types()
                    .kinds()
                    .keys()
                    .any(|kind| *kind != ValueKind::Object)
                {
                    ui.label(i18n::map::endpoint_body_type(lang, &root.types().summary()));
                }
                render_children(ui, root, salt, lang);
            }
            render_parts(ui, group, lang);
        });
}

fn render_body_evidence(ui: &mut egui::Ui, group: &BodySchema, lang: Language) {
    let map_text = lang.map();
    let palette = Palette::of(ui);
    let described = group.structured_samples();
    let mut parts = vec![i18n::map::endpoint_described_count(lang, described)];
    let counts = [
        (group.empty(), map_text.endpoint_empty),
        (group.unstructured(), map_text.endpoint_unstructured),
        (group.invalid(), map_text.endpoint_invalid),
    ];
    for (count, label) in counts {
        if count > 0 {
            parts.push(i18n::map::endpoint_labeled_count(lang, count, label));
        }
    }
    let excluded = group.excluded_total();
    if excluded > 0 {
        let reasons: Vec<String> = group
            .excluded()
            .iter()
            .map(|(reason, count)| format!("{} {count}", domain::exclusion_reason(lang, *reason)))
            .collect();
        parts.push(i18n::map::endpoint_excluded_reasons(
            lang, excluded, &reasons,
        ));
    }
    let color = if excluded > 0 {
        palette.warning
    } else {
        palette.secondary
    };
    ui.label(RichText::new(parts.join(" · ")).color(color).small());
}

fn render_parts(ui: &mut egui::Ui, group: &BodySchema, lang: Language) {
    if group.parts().is_empty() {
        return;
    }
    let map_text = lang.map();
    for (name, part) in group.parts() {
        let types: Vec<&str> = part.content_types().keys().map(String::as_str).collect();
        let kind = if part.files() > 0 {
            map_text.endpoint_file
        } else {
            map_text.endpoint_field
        };
        let types = if types.is_empty() {
            String::new()
        } else {
            format!(" · {}", types.join(", "))
        };
        ui.monospace(format!(
            "{name}  {kind}{types}  {}",
            ratio(part.seen(), group.part_samples())
        ));
    }
    omitted_note(ui, group.omitted_parts(), map_text.endpoint_part, lang);
}

/// Renders an object's fields and an array's item structure.
fn render_children(ui: &mut egui::Ui, shape: &FieldShape, salt: &str, lang: Language) {
    let map_text = lang.map();
    let objects = shape.object_count();
    for (name, field) in shape.fields() {
        render_field(
            ui,
            name,
            field,
            Presence::Ratio(objects),
            &format!("{salt}/{name}"),
            lang,
        );
    }
    if let Some(items) = shape.items() {
        render_field(
            ui,
            "[items]",
            items,
            Presence::Items,
            &format!("{salt}/[]"),
            lang,
        );
    }
    omitted_note(ui, shape.omitted_fields(), map_text.endpoint_field, lang);
    if shape.depth_limited() {
        ui.weak(map_text.endpoint_depth_limited);
    }
}

/// How a field's frequency is shown: against its parent objects, or as an
/// element count for array items.
#[derive(Clone, Copy)]
enum Presence {
    Ratio(u64),
    Items,
}

fn render_field(
    ui: &mut egui::Ui,
    name: &str,
    field: &FieldShape,
    presence: Presence,
    salt: &str,
    lang: Language,
) {
    let palette = Palette::of(ui);
    let label = field_label(name, field, presence, &palette, lang);
    if field.fields().is_empty() && field.items().is_none() {
        ui.label(label);
        return;
    }
    egui::CollapsingHeader::new(label)
        .id_salt(salt)
        .default_open(true)
        .show(ui, |ui| render_children(ui, field, salt, lang));
}

fn field_label(
    name: &str,
    field: &FieldShape,
    presence: Presence,
    palette: &Palette,
    lang: Language,
) -> RichText {
    let map_text = lang.map();
    let types = field.types();
    let mut text = format!("{name}: {}", types.summary());
    match presence {
        Presence::Ratio(total) => {
            let _ = write!(text, "  {}", ratio(field.seen(), total));
            if field.seen() < total {
                text.push_str(map_text.endpoint_optional_suffix);
            }
        }
        Presence::Items => {
            text.push_str(&i18n::map::endpoint_item_count(lang, field.seen()));
        }
    }
    if types.count(ValueKind::Null) > 0 {
        text.push_str(map_text.endpoint_nullable_suffix);
    }
    if types.is_mixed() {
        text.push_str(map_text.endpoint_mixed_suffix);
        return RichText::new(text).monospace().color(palette.warning);
    }
    RichText::new(text).monospace()
}

fn types_text(types: &TypeCounts, palette: &Palette) -> RichText {
    let text = RichText::new(types.summary()).monospace();
    if types.is_mixed() {
        text.color(palette.warning)
    } else {
        text
    }
}

fn section_title(ui: &mut egui::Ui, title: &str) {
    ui.add_space(Space::XS);
    ui.label(RichText::new(title).strong().small());
}

fn omitted_note(ui: &mut egui::Ui, omitted: u64, what: &str, lang: Language) {
    if omitted > 0 {
        ui.weak(i18n::map::endpoint_omitted_note(lang, omitted, what));
    }
}

const fn status_color(key: &ResponseKey, palette: &Palette) -> egui::Color32 {
    match key.status {
        400..=499 => palette.warning,
        500..=599 => palette.error,
        _ => palette.text,
    }
}

/// Formats `seen` of `total` as `3/5 (60%)`.
fn ratio(seen: u64, total: u64) -> String {
    if total == 0 {
        return format!("{seen}");
    }
    let percent = seen.saturating_mul(100) / total;
    format!("{seen}/{total} ({percent}%)")
}

#[cfg(test)]
mod tests {
    use super::ratio;

    #[test]
    fn ratio_shows_counts_and_whole_percent() {
        assert_eq!(ratio(2, 3), "2/3 (66%)");
        assert_eq!(ratio(5, 5), "5/5 (100%)");
        assert_eq!(ratio(4, 0), "4");
    }
}
