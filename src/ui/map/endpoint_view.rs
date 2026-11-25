//! Endpoint section of the Map detail pane: per-method request and response
//! structure for the selected path node.
//!
//! Privacy invariant: this view renders only names, types, formats and
//! counts from [`EndpointSchema`]. The schema holds no query, header,
//! cookie, credential or body value, so none can appear here.

use crate::ui::style::tokens::Space;
use std::fmt::Write as _;

use eframe::egui::{self, RichText};
use http::Method;
use http_inspector_analysis::endpoint_schema::{
    BodySchema, EndpointSchema, EndpointSchemas, FieldShape, ResponseKey, TypeCounts, ValueKind,
};
use http_inspector_analysis::url::{PathSegmentPattern, percent_decode_display};
use http_inspector_analysis::url_map::{NodeId, RequestScope, ViewNode, ViewNodeKey};

use super::MapAction;
use crate::ui::style::theme::Palette;

/// Below this many described bodies the structure is flagged as tentative.
const FEW_SAMPLES: u64 = 3;

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

/// Renders the endpoint section and returns a `Show in Requests` action.
pub(super) fn show(
    ui: &mut egui::Ui,
    node: &ViewNode,
    endpoints: &[(String, EndpointSchema)],
    skipped: u64,
) -> Option<MapAction> {
    let palette = Palette::of(ui);
    ui.label(RichText::new("Endpoints").strong());
    ui.add(
        egui::Label::new(
            RichText::new(
                "Structure inferred from captured requests on this exact path: names, types and how often they appeared. Values are never shown.",
            )
            .color(palette.muted)
            .small(),
        )
        .wrap(),
    );
    if skipped > 0 {
        ui.label(
            RichText::new(format!(
                "Endpoint analysis skipped {skipped} request(s) this session."
            ))
            .color(palette.warning)
            .small(),
        );
    }
    if endpoints.is_empty() {
        ui.weak("No finished requests on this exact path yet.");
        return None;
    }
    let mut action = None;
    for (index, (method, schema)) in endpoints.iter().enumerate() {
        let salt = format!("{}#{method}", node.key.display_url());
        let title = format!("{method} · {} request(s)", schema.requests());
        egui::CollapsingHeader::new(RichText::new(title).monospace().strong())
            .id_salt(&salt)
            .default_open(index == 0)
            .show(ui, |ui| {
                if let Some(clicked) = render_endpoint(ui, node, method, schema, &salt) {
                    action = Some(clicked);
                }
            });
    }
    action
}

fn render_endpoint(
    ui: &mut egui::Ui,
    node: &ViewNode,
    method: &str,
    schema: &EndpointSchema,
    salt: &str,
) -> Option<MapAction> {
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if let Ok(parsed) = Method::from_bytes(method.as_bytes())
            && ui
                .small_button(format!("Show {method} in Requests"))
                .on_hover_text("Switch to Requests filtered to this method and exact path.")
                .clicked()
        {
            action = Some(MapAction::ShowInRequests(RequestScope::endpoint(
                node.key.clone(),
                parsed,
            )));
        }
    });
    render_evidence(ui, schema);
    render_path_params(ui, &node.key, schema);
    render_query(ui, schema, salt);
    render_headers(ui, schema, salt);
    render_auth(ui, schema);
    render_request_bodies(ui, schema, salt);
    render_responses(ui, schema, salt);
    action
}

fn render_evidence(ui: &mut egui::Ui, schema: &EndpointSchema) {
    let palette = Palette::of(ui);
    let mut parts = vec![
        format!("{} request(s)", schema.requests()),
        format!("{} body sample(s) described", schema.structured_bodies()),
    ];
    let excluded = schema.excluded_bodies();
    if excluded > 0 {
        parts.push(format!("{excluded} excluded as truncated or unreadable"));
    }
    if schema.missing_responses() > 0 {
        parts.push(format!("{} without response", schema.missing_responses()));
    }
    ui.label(RichText::new(format!("Evidence: {}", parts.join(" · "))).small());
    if schema.requests() < FEW_SAMPLES {
        ui.label(
            RichText::new("Few samples: fields seen once may be optional or vary.")
                .color(palette.warning)
                .small(),
        );
    }
}

fn render_path_params(ui: &mut egui::Ui, key: &ViewNodeKey, schema: &EndpointSchema) {
    for (position, pattern) in key.path.iter().enumerate() {
        if *pattern != PathSegmentPattern::FoldedId {
            continue;
        }
        let kinds = schema.path_ids().get(&position).map_or_else(
            || "—".to_owned(),
            |kinds| {
                kinds
                    .iter()
                    .map(|(kind, count)| format!("{} ×{count}", kind.label()))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        ui.label(format!(
            "Path parameter {{id}} (segment {}): {kinds}",
            position + 1
        ));
    }
}

fn render_query(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str) {
    section_title(ui, "Query parameters");
    if schema.query().is_empty() {
        ui.weak("None observed.");
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
    omitted_note(ui, schema.omitted_query(), "query parameter");
}

fn render_headers(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str) {
    section_title(ui, "Custom request headers (names only)");
    if schema.headers().is_empty() {
        ui.weak("None observed.");
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
    omitted_note(ui, schema.omitted_headers(), "header");
}

fn render_auth(ui: &mut egui::Ui, schema: &EndpointSchema) {
    section_title(ui, "Authentication (scheme names only)");
    if schema.auth().is_empty() {
        ui.weak("No credentials observed.");
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (scheme, count) in schema.auth() {
            ui.label(format!("{scheme} {}", ratio(*count, schema.requests())));
        }
    });
}

fn render_request_bodies(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str) {
    section_title(ui, "Request body");
    if schema.request_bodies().is_empty() {
        ui.weak("No request bodies observed.");
    }
    for (content_type, group) in schema.request_bodies() {
        let title = format!("{content_type} · {} request(s)", group.bodies());
        render_body_group(
            ui,
            RichText::new(title),
            &format!("{salt}/req/{content_type}"),
            group,
        );
    }
    if schema.requests_without_body() > 0 && !schema.request_bodies().is_empty() {
        ui.weak(format!(
            "{} request(s) without a body",
            schema.requests_without_body()
        ));
    }
    omitted_note(ui, schema.omitted_groups(), "body group");
}

fn render_responses(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str) {
    section_title(ui, "Responses");
    if schema.responses().is_empty() {
        ui.weak("No responses observed.");
        return;
    }
    let palette = Palette::of(ui);
    for (key, group) in schema.responses() {
        let title = RichText::new(format!(
            "{} · {} · {} response(s)",
            key.status,
            key.content_type,
            group.bodies()
        ))
        .color(status_color(key, &palette));
        render_body_group(
            ui,
            title,
            &format!("{salt}/res/{}/{}", key.status, key.content_type),
            group,
        );
    }
}

fn render_body_group(ui: &mut egui::Ui, title: RichText, salt: &str, group: &BodySchema) {
    egui::CollapsingHeader::new(title)
        .id_salt(salt)
        .default_open(true)
        .show(ui, |ui| {
            render_body_evidence(ui, group);
            let root = group.fields();
            if root.seen() > 0 {
                if root
                    .types()
                    .kinds()
                    .keys()
                    .any(|kind| *kind != ValueKind::Object)
                {
                    ui.label(format!("Body type: {}", root.types().summary()));
                }
                render_children(ui, root, salt);
            }
            render_parts(ui, group);
        });
}

fn render_body_evidence(ui: &mut egui::Ui, group: &BodySchema) {
    let palette = Palette::of(ui);
    let described = group.structured_samples();
    let mut parts = vec![format!("{described} described")];
    let counts = [
        (group.empty(), "empty"),
        (group.unstructured(), "without field structure"),
        (group.invalid(), "unparseable"),
    ];
    for (count, label) in counts {
        if count > 0 {
            parts.push(format!("{count} {label}"));
        }
    }
    let excluded = group.excluded_total();
    if excluded > 0 {
        let reasons: Vec<String> = group
            .excluded()
            .iter()
            .map(|(reason, count)| format!("{reason} {count}"))
            .collect();
        parts.push(format!("{excluded} excluded ({})", reasons.join(", ")));
    }
    let color = if excluded > 0 || (described > 0 && described < FEW_SAMPLES) {
        palette.warning
    } else {
        palette.muted
    };
    ui.label(RichText::new(parts.join(" · ")).color(color).small());
}

fn render_parts(ui: &mut egui::Ui, group: &BodySchema) {
    if group.parts().is_empty() {
        return;
    }
    for (name, part) in group.parts() {
        let types: Vec<&str> = part.content_types().keys().map(String::as_str).collect();
        let kind = if part.files() > 0 { "file" } else { "field" };
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
    omitted_note(ui, group.omitted_parts(), "part");
}

/// Renders an object's fields and an array's item structure.
fn render_children(ui: &mut egui::Ui, shape: &FieldShape, salt: &str) {
    let objects = shape.object_count();
    for (name, field) in shape.fields() {
        render_field(
            ui,
            name,
            field,
            Presence::Ratio(objects),
            &format!("{salt}/{name}"),
        );
    }
    if let Some(items) = shape.items() {
        render_field(ui, "[items]", items, Presence::Items, &format!("{salt}/[]"));
    }
    omitted_note(ui, shape.omitted_fields(), "field");
    if shape.depth_limited() {
        ui.weak("Deeper nesting is not described.");
    }
}

/// How a field's frequency is shown: against its parent objects, or as an
/// element count for array items.
#[derive(Clone, Copy)]
enum Presence {
    Ratio(u64),
    Items,
}

fn render_field(ui: &mut egui::Ui, name: &str, field: &FieldShape, presence: Presence, salt: &str) {
    let palette = Palette::of(ui);
    let label = field_label(name, field, presence, &palette);
    if field.fields().is_empty() && field.items().is_none() {
        ui.label(label);
        return;
    }
    egui::CollapsingHeader::new(label)
        .id_salt(salt)
        .default_open(true)
        .show(ui, |ui| render_children(ui, field, salt));
}

fn field_label(name: &str, field: &FieldShape, presence: Presence, palette: &Palette) -> RichText {
    let types = field.types();
    let mut text = format!("{name}: {}", types.summary());
    match presence {
        Presence::Ratio(total) => {
            let _ = write!(text, "  {}", ratio(field.seen(), total));
            if field.seen() < total {
                text.push_str(" · optional");
            }
        }
        Presence::Items => {
            let _ = write!(text, "  {} item(s)", field.seen());
        }
    }
    if types.count(ValueKind::Null) > 0 {
        text.push_str(" · nullable");
    }
    if types.is_mixed() {
        text.push_str(" · mixed types");
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

fn omitted_note(ui: &mut egui::Ui, omitted: u64, what: &str) {
    if omitted > 0 {
        ui.weak(format!(
            "{omitted} more {what}(s) not kept (limit reached)."
        ));
    }
}

const fn status_color(key: &ResponseKey, palette: &Palette) -> egui::Color32 {
    match key.status {
        200..=299 => palette.success,
        300..=399 => palette.warning,
        400..=599 => palette.error,
        _ => palette.muted,
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
