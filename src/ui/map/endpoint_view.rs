//! Endpoint section of the Map detail pane: per-method request and response
//! structure for the selected path node.
//!
//! Privacy invariant: this view renders only names, types, formats and
//! counts from [`EndpointSchema`]. The schema holds no query, header,
//! cookie, credential or body value, so none can appear here.

use crate::ui::style::tokens::{Layout, Space};
use crate::ui::workbench::text::{self, ShellText};
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
use crate::ui::i18n::{Language, domain};
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
    let palette = Palette::of(ui);
    let mut action = None;
    tree::group(
        ui,
        "map-endpoints",
        ShellText::Endpoints.text(lang),
        Some(endpoints.len()),
        |ui| {
            egui::Frame::new()
                .inner_margin(Layout::GROUP_BODY_MARGIN)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = Space::XS;
                    if skipped > 0 {
                        ui.label(
                            RichText::new(text::analysis_skipped(lang, skipped))
                                .color(palette.warning),
                        );
                    }
                    for (index, (method, schema)) in endpoints.iter().enumerate() {
                        let salt = format!("{}#{method}", node.key.display_url());
                        let title = match lang {
                            Language::English => {
                                format!("{method} · {} request(s)", schema.requests())
                            }
                            Language::Korean => format!("{method} · 요청 {}건", schema.requests()),
                        };
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
    .on_hover_text(ShellText::EndpointsHint.text(lang));
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
            let label = match lang {
                Language::English => format!("Show {method} in Requests"),
                Language::Korean => format!("요청 목록에서 {method} 보기"),
            };
            if ui
                .small_button(label)
                .on_hover_text(lang.pick(
                    "Switch to Requests filtered to this method and exact path.",
                    "이 메서드와 정확한 경로로 필터링된 요청 목록으로 전환합니다.",
                ))
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
    let mut parts = match lang {
        Language::English => vec![
            format!("{} request(s)", schema.requests()),
            format!("{} body sample(s) described", schema.structured_bodies()),
        ],
        Language::Korean => vec![
            format!("요청 {}건", schema.requests()),
            format!("본문 샘플 {}건 분석", schema.structured_bodies()),
        ],
    };
    let excluded = schema.excluded_bodies();
    if excluded > 0 {
        parts.push(match lang {
            Language::English => format!("{excluded} excluded as truncated or unreadable"),
            Language::Korean => format!("{excluded}건 제외(잘림 또는 읽기 불가)"),
        });
    }
    if schema.missing_responses() > 0 {
        parts.push(match lang {
            Language::English => format!("{} without response", schema.missing_responses()),
            Language::Korean => format!("응답 없음 {}건", schema.missing_responses()),
        });
    }
    let summary = match lang {
        Language::English => format!("Evidence: {}", parts.join(" · ")),
        Language::Korean => format!("근거: {}", parts.join(" · ")),
    };
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
        let text = match lang {
            Language::English => {
                format!("Path parameter {{id}} (segment {}): {kinds}", position + 1)
            }
            Language::Korean => {
                format!("경로 매개변수 {{id}} (세그먼트 {}): {kinds}", position + 1)
            }
        };
        ui.label(text);
    }
}

fn render_query(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    section_title(ui, lang.pick("Query parameters", "쿼리 매개변수"));
    if schema.query().is_empty() {
        ui.weak(lang.pick("None observed.", "관측되지 않음."));
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
        lang.pick("query parameter", "쿼리 매개변수"),
        lang,
    );
}

fn render_headers(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    section_title(
        ui,
        lang.pick(
            "Custom request headers (names only)",
            "사용자 지정 요청 헤더 (이름만)",
        ),
    );
    if schema.headers().is_empty() {
        ui.weak(lang.pick("None observed.", "관측되지 않음."));
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
        lang.pick("header", "헤더"),
        lang,
    );
}

fn render_auth(ui: &mut egui::Ui, schema: &EndpointSchema, lang: Language) {
    section_title(
        ui,
        lang.pick("Authentication (scheme names only)", "인증 (방식 이름만)"),
    );
    if schema.auth().is_empty() {
        ui.weak(lang.pick("No credentials observed.", "관측된 인증 정보 없음."));
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (scheme, count) in schema.auth() {
            ui.label(format!("{scheme} {}", ratio(*count, schema.requests())));
        }
    });
}

fn render_request_bodies(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    section_title(ui, lang.pick("Request body", "요청 본문"));
    if schema.request_bodies().is_empty() {
        ui.weak(lang.pick(
            "No request bodies observed.",
            "관측된 요청 본문이 없습니다.",
        ));
    }
    for (content_type, group) in schema.request_bodies() {
        let title = match lang {
            Language::English => format!("{content_type} · {} request(s)", group.bodies()),
            Language::Korean => format!("{content_type} · 요청 {}건", group.bodies()),
        };
        render_body_group(
            ui,
            RichText::new(title),
            &format!("{salt}/req/{content_type}"),
            group,
            lang,
        );
    }
    if schema.requests_without_body() > 0 && !schema.request_bodies().is_empty() {
        let note = match lang {
            Language::English => format!(
                "{} request(s) without a body",
                schema.requests_without_body()
            ),
            Language::Korean => format!("본문 없는 요청 {}건", schema.requests_without_body()),
        };
        ui.weak(note);
    }
    omitted_note(
        ui,
        schema.omitted_groups(),
        lang.pick("body group", "본문 그룹"),
        lang,
    );
}

fn render_responses(ui: &mut egui::Ui, schema: &EndpointSchema, salt: &str, lang: Language) {
    section_title(ui, lang.pick("Responses", "응답"));
    if schema.responses().is_empty() {
        ui.weak(lang.pick("No responses observed.", "관측된 응답이 없습니다."));
        return;
    }
    let palette = Palette::of(ui);
    for (key, group) in schema.responses() {
        let title_text = match lang {
            Language::English => format!(
                "{} · {} · {} response(s)",
                key.status,
                key.content_type,
                group.bodies()
            ),
            Language::Korean => format!(
                "{} · {} · 응답 {}건",
                key.status,
                key.content_type,
                group.bodies()
            ),
        };
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
                    let label = match lang {
                        Language::English => format!("Body type: {}", root.types().summary()),
                        Language::Korean => format!("본문 형식: {}", root.types().summary()),
                    };
                    ui.label(label);
                }
                render_children(ui, root, salt, lang);
            }
            render_parts(ui, group, lang);
        });
}

fn render_body_evidence(ui: &mut egui::Ui, group: &BodySchema, lang: Language) {
    let palette = Palette::of(ui);
    let described = group.structured_samples();
    let mut parts = vec![match lang {
        Language::English => format!("{described} described"),
        Language::Korean => format!("분석 {described}건"),
    }];
    let counts = [
        (group.empty(), lang.pick("empty", "비어 있음")),
        (
            group.unstructured(),
            lang.pick("without field structure", "필드 구조 없음"),
        ),
        (group.invalid(), lang.pick("unparseable", "파싱 불가")),
    ];
    for (count, label) in counts {
        if count > 0 {
            parts.push(match lang {
                Language::English => format!("{count} {label}"),
                Language::Korean => format!("{label} {count}건"),
            });
        }
    }
    let excluded = group.excluded_total();
    if excluded > 0 {
        let reasons: Vec<String> = group
            .excluded()
            .iter()
            .map(|(reason, count)| format!("{} {count}", domain::exclusion_reason(lang, *reason)))
            .collect();
        parts.push(match lang {
            Language::English => format!("{excluded} excluded ({})", reasons.join(", ")),
            Language::Korean => format!("제외 {excluded}건 ({})", reasons.join(", ")),
        });
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
    for (name, part) in group.parts() {
        let types: Vec<&str> = part.content_types().keys().map(String::as_str).collect();
        let kind = if part.files() > 0 {
            lang.pick("file", "파일")
        } else {
            lang.pick("field", "필드")
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
    omitted_note(ui, group.omitted_parts(), lang.pick("part", "파트"), lang);
}

/// Renders an object's fields and an array's item structure.
fn render_children(ui: &mut egui::Ui, shape: &FieldShape, salt: &str, lang: Language) {
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
    omitted_note(ui, shape.omitted_fields(), lang.pick("field", "필드"), lang);
    if shape.depth_limited() {
        ui.weak(lang.pick(
            "Deeper nesting is not described.",
            "더 깊은 중첩 구조는 표시하지 않습니다.",
        ));
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
    let types = field.types();
    let mut text = format!("{name}: {}", types.summary());
    match presence {
        Presence::Ratio(total) => {
            let _ = write!(text, "  {}", ratio(field.seen(), total));
            if field.seen() < total {
                text.push_str(lang.pick(" · optional", " · 선택"));
            }
        }
        Presence::Items => {
            let _ = match lang {
                Language::English => write!(text, "  {} item(s)", field.seen()),
                Language::Korean => write!(text, "  항목 {}개", field.seen()),
            };
        }
    }
    if types.count(ValueKind::Null) > 0 {
        text.push_str(lang.pick(" · nullable", " · null 허용"));
    }
    if types.is_mixed() {
        text.push_str(lang.pick(" · mixed types", " · 혼합 형식"));
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
        let note = match lang {
            Language::English => format!("{omitted} more {what}(s) not kept (limit reached)."),
            Language::Korean => {
                format!("한도에 도달해 {what} {omitted}개가 더 보관되지 않았습니다.")
            }
        };
        ui.weak(note);
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
