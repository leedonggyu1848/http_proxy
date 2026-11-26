//! Network-panel style request list that lays out only the visible rows.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::time::Duration;

use eframe::egui::{self, Key, RichText};
use egui_extras::{Column, TableBuilder};
use http_inspector_analysis::filter::{FilterKey, TargetParts, content_type};
use http_inspector_exchange::{ByteSize, Exchange, Outcome, RecordId};

use crate::ui::AppAction;
use crate::ui::dialogs::copy_curl;
use crate::ui::i18n::{Language, Text};
use crate::ui::requests::filter_menu;
use crate::ui::requests::outcome_view::{outcome_summary, outcome_text, status_rank};
use crate::ui::requests::target_view::{host_fits, middle_ellipsis, shared_host};
use crate::ui::style::tokens::{Layout, Radius, Space, StrokeWidth, ThemeTokens, TypeScale};
use crate::ui::style::{indicator, theme::Palette};
use crate::ui::workbench::text::ShellText;
use http_inspector_core::recording::SaveFormat;

#[derive(Clone, Copy, Default)]
pub(super) struct TableMenus {
    pub(super) saving_body: bool,
    pub(super) filter_enabled: bool,
    pub(super) lang: Language,
}

/// Space before the method, kept free for the selection bar of the row.
const ROW_LEAD: f32 = Layout::SELECTION_BAR_WIDTH + Space::S;
/// Space before the status text, kept for the failure tick.
const STATUS_LEAD: f32 = Layout::TICK_WIDTH + Space::S;

/// Sortable column of the request list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Field {
    Method,
    Target,
    Status,
    Type,
    Size,
    Duration,
}

impl Field {
    const ALL: [Self; 6] = [
        Self::Method,
        Self::Target,
        Self::Status,
        Self::Type,
        Self::Size,
        Self::Duration,
    ];

    const fn title(self, lang: Language) -> &'static str {
        match self {
            Self::Method => lang.text(Text::ColMethod),
            Self::Target => lang.text(Text::ColPath),
            Self::Status => lang.text(Text::ColStatus),
            Self::Type => lang.text(Text::ColType),
            Self::Size => lang.text(Text::ColSize),
            Self::Duration => lang.pick("Duration", "시간"),
        }
    }

    /// Minimum width kept even if the user drags a column narrow.
    const fn min_width(self) -> f32 {
        let widths = Layout::REQUEST_COLUMNS;
        match self {
            Self::Method => widths.method_min,
            Self::Target => widths.path_resize_min,
            Self::Status => widths.status_min,
            Self::Type => widths.kind_min,
            Self::Size => widths.size_min,
            Self::Duration => widths.duration_min,
        }
    }

    /// Numbers line up on the right so that their digits compare.
    const fn is_numeric(self) -> bool {
        matches!(self, Self::Size | Self::Duration)
    }

    /// Text before the title, matching the lead of the column's cells.
    const fn lead(self) -> f32 {
        match self {
            Self::Method => ROW_LEAD,
            Self::Status => STATUS_LEAD,
            _ => 0.0,
        }
    }

    /// Columns truncate their text instead of clipping, because a clipped
    /// cell gets its own clip rectangle and therefore its own draw call.
    fn column(self, width: f32) -> Column {
        Column::initial(width.max(self.min_width())).at_least(self.min_width())
    }

    fn compare(self, left: &Exchange, right: &Exchange) -> Ordering {
        match self {
            Self::Method => left
                .request()
                .method()
                .as_str()
                .cmp(right.request().method().as_str()),
            Self::Target => target_text(left).cmp(target_text(right)),
            Self::Status => status_rank(left.outcome()).cmp(&status_rank(right.outcome())),
            Self::Type => content_type(left).cmp(&content_type(right)),
            Self::Size => left.received_bytes().cmp(&right.received_bytes()),
            Self::Duration => duration(left).cmp(&duration(right)),
        }
    }
}

/// Row order: arrival order unless the user picked a column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Sort {
    field: Option<Field>,
    descending: bool,
}

impl Sort {
    /// Cycles a header through ascending, descending, and arrival order.
    fn toggled(self, field: Field) -> Self {
        match self.field {
            Some(current) if current == field && !self.descending => Self {
                field: Some(field),
                descending: true,
            },
            Some(current) if current == field => Self {
                field: None,
                descending: false,
            },
            _ => Self {
                field: Some(field),
                descending: false,
            },
        }
    }

    fn arrow(self, field: Field) -> &'static str {
        match self.field {
            Some(current) if current == field && self.descending => " ▼",
            Some(current) if current == field => " ▲",
            _ => "",
        }
    }
}

fn move_selection(key: Key, current: Option<usize>, len: usize, page: usize) -> Option<usize> {
    let last = len.checked_sub(1)?;
    let index = match key {
        Key::ArrowUp => current.map_or(last, |index| index.saturating_sub(1)),
        Key::ArrowDown => current.map_or(0, |index| index.saturating_add(1)),
        Key::Home => 0,
        Key::End => last,
        Key::PageUp => current.map_or(last, |index| index.saturating_sub(page)),
        Key::PageDown => current.map_or(0, |index| index.saturating_add(page)),
        _ => return None,
    };
    Some(index.min(last))
}

fn keyboard_selection(
    ui: &egui::Ui,
    current: Option<usize>,
    len: usize,
    page: usize,
) -> Option<usize> {
    let mut selected = current;
    let mut moved_with_arrow = false;
    ui.input_mut(|input| {
        input.events.retain(|event| {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
                ..
            } = *event
                && let Some(index) = move_selection(key, selected, len, page)
            {
                selected = Some(index);
                moved_with_arrow |= matches!(key, Key::ArrowUp | Key::ArrowDown);
                return false;
            }
            true
        });
    });
    if moved_with_arrow {
        ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
    }
    selected.filter(|&index| Some(index) != current)
}

pub(super) const fn follow_newest_label(lang: Language) -> &'static str {
    lang.pick("Follow newest", "최신 항목 따라가기")
}

pub(super) const fn arrival_order_label(lang: Language) -> &'static str {
    lang.pick("Arrival order", "도착순")
}

/// Sort state and scrolling preference of the request list.
pub(crate) struct RequestTable {
    sort: Sort,
    follow_newest: bool,
    keyboard_row: Option<usize>,
    page_rows: usize,
    last_viewport_width: f32,
}

#[derive(Default)]
pub(super) struct TableResult {
    pub(super) selected: Option<RecordId>,
    pub(super) action: Option<AppAction>,
}

impl RequestTable {
    pub(crate) const fn new() -> Self {
        Self {
            sort: Sort {
                field: None,
                descending: false,
            },
            follow_newest: true,
            keyboard_row: None,
            page_rows: 1,
            last_viewport_width: 0.0,
        }
    }

    /// Reports whether the list is sorted by a column instead of arrival.
    pub(super) const fn is_sorted(&self) -> bool {
        self.sort.field.is_some()
    }

    /// Shows the list options in the request pane header.
    pub(super) fn controls(&mut self, ui: &mut egui::Ui, lang: Language) {
        ui.checkbox(&mut self.follow_newest, follow_newest_label(lang))
            .on_hover_text(lang.pick(
                "Keep the newest request in view while sorted by arrival.",
                "도착순으로 정렬된 동안 가장 최근 요청이 보이도록 유지합니다.",
            ));
        if self.follow_newest {
            self.keyboard_row = None;
        }
        if self.sort.field.is_some()
            && ui
                .add(crate::ui::workbench::controls::ghost(
                    arrival_order_label(lang),
                    Layout::PANE_BUTTON_HEIGHT,
                ))
                .clicked()
        {
            self.sort = Self::new().sort;
        }
    }

    fn sort_rows(&self, rows: &mut [&Exchange]) {
        if let Some(field) = self.sort.field {
            rows.sort_by(|left, right| {
                let order = field.compare(left, right);
                if self.sort.descending {
                    order.reverse()
                } else {
                    order
                }
            });
        }
    }

    /// Shows `rows` (oldest first) and returns a newly selected record.
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        mut rows: Vec<&Exchange>,
        mut selected: Option<RecordId>,
        shortcuts_allowed: bool,
        menus: TableMenus,
    ) -> TableResult {
        self.sort_rows(&mut rows);
        let mut result = TableResult::default();
        let mut scroll_to = None;
        if shortcuts_allowed || self.keyboard_row.is_some() {
            let current = rows.iter().position(|record| Some(record.id()) == selected);
            if shortcuts_allowed
                && let Some(index) = keyboard_selection(ui, current, rows.len(), self.page_rows)
                && let Some(record) = rows.get(index)
            {
                self.follow_newest = false;
                self.keyboard_row = Some(index);
                result.selected = Some(record.id());
                selected = result.selected;
                scroll_to = Some(index);
                ui.ctx().request_repaint();
            } else if self.keyboard_row.is_some() && self.keyboard_row != current {
                self.keyboard_row = current;
                scroll_to = current;
            }
        }

        let viewport_width = ui.available_width();
        let spacing_x = ui.spacing().item_spacing.x;
        let scroll_gutter = ui.spacing().scroll.allocated_width();
        let widths = column_widths(viewport_width, spacing_x, scroll_gutter);
        let total_width = spacing_x.mul_add(5.0, widths.iter().sum::<f32>()) + scroll_gutter;
        let width_changed = (viewport_width - self.last_viewport_width).abs() > 1.0;
        self.last_viewport_width = viewport_width;

        let inner_height = egui::ScrollArea::horizontal()
            .id_salt("requests-hscroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(total_width);
                self.show_table(
                    ui,
                    &rows,
                    selected,
                    scroll_to,
                    widths,
                    width_changed,
                    &mut result,
                    menus,
                )
            })
            .inner;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "The viewport height is nonnegative and UI-sized, and a page counts only full rows"
        )]
        let page_rows = (inner_height / Layout::TABLE_ROW_HEIGHT) as usize;
        self.page_rows = page_rows.max(1);
        result
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Table rendering passes frame-local selection and layout metrics directly without extra state"
    )]
    fn show_table(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[&Exchange],
        selected: Option<RecordId>,
        scroll_to: Option<usize>,
        widths: [f32; 6],
        width_changed: bool,
        result: &mut TableResult,
        menus: TableMenus,
    ) -> f32 {
        // Rows touch each other; the row line separates them.
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut table = TableBuilder::new(ui)
            .id_salt("requests")
            .striped(false)
            .resizable(true)
            .sense(egui::Sense::click())
            .auto_shrink([false, false])
            .stick_to_bottom(self.follow_newest && self.sort.field.is_none())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for (field, width) in Field::ALL.into_iter().zip(widths) {
            table = table.column(field.column(width));
        }
        if width_changed {
            table.reset();
        }
        if let Some(index) = scroll_to {
            table = table.scroll_to_row(index, None).animate_scrolling(false);
        }
        // One shared host moves from every row to the column header.
        let shared_host = shared_host(rows.iter().map(|record| record.request().target()));
        let body = table
            .header(Layout::TABLE_HEADER_HEIGHT, |mut header| {
                for field in Field::ALL {
                    header.col(|ui| {
                        paint_header_cell(ui, field == Field::Duration);
                        let host = shared_host.filter(|_| field == Field::Target);
                        if self.header_cell(ui, field, host, menus, result) {
                            self.sort = self.sort.toggled(field);
                        }
                    });
                }
            })
            .body(|mut body| {
                // Let the table cells handle row clicks while labels still show tooltips.
                body.ui_mut().style_mut().interaction.selectable_labels = false;
                body.rows(Layout::TABLE_ROW_HEIGHT, rows.len(), |mut row| {
                    let Some(record) = rows.get(row.index()) else {
                        return;
                    };
                    row.set_selected(selected == Some(record.id()));
                    let selected_row = selected == Some(record.id());
                    let focus_row = scroll_to == Some(row.index());
                    let show_host = shared_host.is_none();
                    let (method_clicked, action) =
                        show_cells(&mut row, record, selected_row, focus_row, show_host, menus);
                    if method_clicked {
                        self.follow_newest = false;
                        self.keyboard_row = None;
                        result.selected = Some(record.id());
                    }
                    if let Some(action) = action {
                        result.action = Some(action);
                    }
                    if row.response().clicked() {
                        self.keyboard_row = None;
                        result.selected = Some(record.id());
                    }
                });
            });
        body.inner_rect.height()
    }

    /// The column title, which sorts by the column when clicked, and the
    /// host shared by every row. Returns whether the title was clicked.
    fn header_cell(
        &self,
        ui: &mut egui::Ui,
        field: Field,
        shared_host: Option<&str>,
        menus: TableMenus,
        result: &mut TableResult,
    ) -> bool {
        let lang = menus.lang;
        let secondary = ThemeTokens::for_theme(ui.ctx().theme()).text.secondary;
        let title = RichText::new(format!("{}{}", field.title(lang), self.sort.arrow(field)))
            .size(TypeScale::COLUMN_HEADER)
            .color(secondary);
        let button = egui::Button::new(title).frame(false).truncate();
        if field.is_numeric() {
            return ui
                .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(button).clicked()
                })
                .inner;
        }
        ui.add_space(field.lead());
        let clicked = ui.add(button).clicked();
        if let Some(host) = shared_host {
            let label = egui::Label::new(RichText::new(host).monospace().weak())
                .truncate()
                .sense(egui::Sense::click());
            ui.add(label)
                .on_hover_text(ShellText::SharedHost.text(lang))
                .context_menu(|ui| {
                    if let Some(action) =
                        filter_menu::items(ui, FilterKey::Host, host, menus.filter_enabled, lang)
                    {
                        result.action = Some(action);
                    }
                });
        }
        clicked
    }
}

/// Horizontal extent of a cell including half the column gap on each side,
/// so that neighboring bands join. The last band reaches the right edge.
fn cell_band(ui: &egui::Ui, last: bool) -> egui::Rect {
    let mut band = ui
        .max_rect()
        .expand2(egui::vec2(0.5 * ui.spacing().item_spacing.x, 0.0));
    if last {
        band.max.x = band.max.x.max(ui.clip_rect().right());
    }
    band
}

/// Header background on the chrome surface with the structural line below.
fn paint_header_cell(ui: &egui::Ui, last: bool) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let band = cell_band(ui, last);
    let painter = ui.painter();
    painter.rect_filled(band, Radius::PANE, tokens.surfaces.chrome);
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.structural),
    );
}

/// The subtle line under a row and, in the first cell of the selected row,
/// the selection bar at its left edge.
fn paint_row_cell(ui: &egui::Ui, selection_bar: bool, last: bool) {
    let tokens = ThemeTokens::for_theme(ui.ctx().theme());
    let band = cell_band(ui, last);
    let painter = ui.painter();
    painter.hline(
        band.x_range(),
        band.bottom() - 0.5,
        egui::Stroke::new(StrokeWidth::STRUCTURAL, tokens.borders.subtle),
    );
    if selection_bar {
        let cell = ui.max_rect();
        let bar = egui::Rect::from_min_size(
            cell.left_top(),
            egui::vec2(Layout::SELECTION_BAR_WIDTH, cell.height()),
        );
        painter.rect_filled(bar, Radius::PANE, tokens.accent.primary);
    }
}

/// Computes responsive initial column widths for `viewport_width`.
/// When all six columns fit, `Path` takes the spare width. When the
/// table is narrower, `Path` fills the space beside the leading columns
/// that fit cleanly without slicing the next column at the pane border, while
/// the remaining columns stay reachable via horizontal scrolling.
fn column_widths(viewport_width: f32, spacing_x: f32, scroll_gutter: f32) -> [f32; 6] {
    let widths = Layout::REQUEST_COLUMNS;

    let usable = (viewport_width - scroll_gutter).max(0.0);
    let fixed_wide =
        widths.method + widths.status_wide + widths.kind + widths.size + widths.duration;
    let all_spacing = spacing_x * 5.0;
    if usable >= fixed_wide + all_spacing + widths.path_min {
        let target = (usable - fixed_wide - all_spacing).floor();
        return [
            widths.method,
            target,
            widths.status_wide,
            widths.kind,
            widths.size,
            widths.duration,
        ];
    }

    let visible = viewport_width.max(0.0);
    let fixed_five = spacing_x.mul_add(
        4.0,
        widths.method + widths.status_compact + widths.kind + widths.size,
    ) + Space::XS;
    if visible >= fixed_five + widths.path_min {
        let target = (visible - fixed_five).floor();
        return [
            widths.method,
            target,
            widths.status_compact,
            widths.kind,
            widths.size,
            widths.duration,
        ];
    }

    let fixed_four =
        spacing_x.mul_add(3.0, widths.method + widths.status_compact + widths.kind) + Space::XS;
    if visible >= fixed_four + widths.path_min {
        let target = (visible - fixed_four).floor();
        return [
            widths.method,
            target,
            widths.status_compact,
            widths.kind,
            widths.size,
            widths.duration,
        ];
    }

    let method = widths
        .method
        .min((visible * 0.22).floor().max(Field::Method.min_width()));
    let status = widths
        .status_compact
        .min((visible * 0.30).floor().max(Field::Status.min_width()));
    let target = spacing_x
        .mul_add(-2.0, visible - method - status - Space::XS)
        .floor();
    let target = widths.path_compact.fit(target);
    [
        method,
        target,
        status,
        widths.kind,
        widths.size,
        widths.duration,
    ]
}

/// Offers resending, copying, or saving a body of the row's request on right click.
fn row_menu(
    response: &egui::Response,
    record: &Exchange,
    menus: TableMenus,
    field: Field,
) -> Option<AppAction> {
    let lang = menus.lang;
    let mut action = None;
    response.context_menu(|ui| {
        if let Some((key, value)) = field_filter(field, record) {
            action = filter_menu::items(ui, key, &value, menus.filter_enabled, lang);
            ui.separator();
        }
        if ui
            .add_enabled(
                record.source() != http_inspector_exchange::CaptureSource::Connect,
                egui::Button::new(ShellText::EditAndResend.text(lang)),
            )
            .clicked()
        {
            action = Some(AppAction::OpenReplay(record.id()));
            ui.close();
        }
        if copy_curl::copy_button(ui, record, lang).clicked() {
            action = Some(AppAction::CopyCurl(record.id()));
            ui.close();
        }
        ui.separator();
        if let Some(side) = save_body_items(ui, record, !menus.saving_body, lang) {
            action = Some(AppAction::SaveBody(record.id(), side, SaveFormat::Decoded));
            ui.close();
        }
    });
    action
}

fn save_body_items(
    ui: &mut egui::Ui,
    record: &Exchange,
    idle: bool,
    lang: Language,
) -> Option<http_inspector_exchange::BodySide> {
    let has_response = matches!(
        record.outcome(),
        http_inspector_exchange::Outcome::Response(_)
    );
    let mut chosen = None;
    for (side, label, enabled) in [
        (
            http_inspector_exchange::BodySide::Response,
            lang.pick("Save response body", "응답 본문 저장"),
            has_response,
        ),
        (
            http_inspector_exchange::BodySide::Request,
            lang.pick("Save request body", "요청 본문 저장"),
            record.request().body().is_some(),
        ),
    ] {
        if ui
            .add_enabled(enabled && idle, egui::Button::new(label))
            .on_hover_text(lang.pick(
                "Save the decoded body to the Save directory.",
                "디코딩된 본문을 저장 폴더에 저장합니다.",
            ))
            .on_disabled_hover_text(lang.pick(
                "Unavailable while another body is being saved, or when the body was not observed.",
                "다른 본문을 저장 중이거나 관찰된 본문이 없을 때는 사용할 수 없습니다.",
            ))
            .clicked()
        {
            chosen = Some(side);
        }
    }
    chosen
}

/// Fills a row and reports selection and context actions.
fn show_cells(
    row: &mut egui_extras::TableRow<'_, '_>,
    record: &Exchange,
    selected: bool,
    focus: bool,
    show_host: bool,
    menus: TableMenus,
) -> (bool, Option<AppAction>) {
    let lang = menus.lang;
    let mut clicked = false;
    let mut action = None;
    for field in Field::ALL {
        let mut method_response = None;
        let (_, cell) = row.col(|ui| {
            paint_row_cell(
                ui,
                selected && field == Field::Method,
                field == Field::Duration,
            );
            match field {
                Field::Method => {
                    method_response = Some(method_cell(ui, record, selected, focus, lang));
                }
                Field::Target => target_cell(ui, record, show_host, lang),
                Field::Status => status_cell(ui, record, lang),
                Field::Type => data_cell(ui, content_type(record).unwrap_or("—")),
                Field::Size => numeric_cell(ui, &size_text(record)),
                Field::Duration => numeric_cell(ui, &duration_text(record)),
            }
        });
        let cell = if let Some(method) = method_response {
            clicked = method.clicked();
            cell.union(method)
        } else {
            cell
        };
        if let Some(selected_action) = row_menu(&cell, record, menus, field) {
            action = Some(selected_action);
        }
    }
    (clicked, action)
}

fn field_filter(field: Field, record: &Exchange) -> Option<(FilterKey, Cow<'_, str>)> {
    match field {
        Field::Method => Some((
            FilterKey::Method,
            Cow::Borrowed(record.request().method().as_str()),
        )),
        Field::Target => {
            let host = TargetParts::of(record.request().target()).host;
            (!host.is_empty()).then_some((FilterKey::Host, Cow::Borrowed(host)))
        }
        Field::Status => match record.outcome() {
            Outcome::Response(response) => Some((
                FilterKey::Status,
                Cow::Owned(response.status().as_u16().to_string()),
            )),
            _ => None,
        },
        _ => None,
    }
}

/// Monospace data that truncates to the cell.
fn data_cell(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).monospace()).truncate());
}

/// A number aligned to the right edge of the cell.
fn numeric_cell(ui: &mut egui::Ui, text: &str) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        data_cell(ui, text);
    });
}

/// The method, which also carries the row's keyboard focus and its screen
/// reader name.
fn method_cell(
    ui: &mut egui::Ui,
    record: &Exchange,
    selected: bool,
    focus: bool,
    lang: Language,
) -> egui::Response {
    ui.add_space(ROW_LEAD);
    ui.push_id(record.id(), |ui| {
        let response = method_text(ui, record.request().method().as_str());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::ListBoxOption,
                ui.is_enabled(),
                selected,
                format!(
                    "{} {} · {}",
                    record.request().method(),
                    record.request().target(),
                    outcome_summary(record.outcome(), lang),
                ),
            )
        });
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.clear_toggled();
            node.set_selected(selected);
        });
        if focus && !response.has_focus() {
            response.request_focus();
        }
        if response.has_focus() {
            let ring = ThemeTokens::for_theme(ui.ctx().theme()).accent.secondary;
            ui.painter().rect_stroke(
                response.rect.expand(Space::XXS),
                Radius::CONTROL,
                egui::Stroke::new(StrokeWidth::FOCUS_RING, ring),
                egui::StrokeKind::Outside,
            );
        }
        response
    })
    .inner
}

/// The method as plain clickable text. An `egui::Label` would underline
/// itself when focused, which repeats the focus ring.
fn method_text(ui: &mut egui::Ui, method: &str) -> egui::Response {
    let galley = egui::WidgetText::from(RichText::new(method).monospace()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        ui.available_width(),
        egui::TextStyle::Monospace,
    );
    let (rect, response) = ui.allocate_exact_size(galley.size(), egui::Sense::click());
    let elided = galley.elided;
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.min, galley, Palette::of(ui).text);
    }
    if elided {
        response.on_hover_text(method)
    } else {
        response
    }
}

/// The path, elided in the middle when it does not fit, then the host in
/// secondary text when the list shows several hosts.
fn target_cell(ui: &mut egui::Ui, record: &Exchange, show_host: bool, lang: Language) {
    let target = record.request().target();
    if let Some(original) = record.replay_of() {
        let label = match lang {
            Language::English => format!("Replay of #{} ·", original.value()),
            Language::Korean => format!("재전송 #{} ·", original.value()),
        };
        ui.add(egui::Label::new(RichText::new(label).weak()))
            .on_hover_text(target);
    }
    let parts = TargetParts::of(target);
    let font = egui::TextStyle::Monospace.resolve(ui.style());
    let char_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'M'));
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Widths are nonnegative and UI-sized; the cell holds only whole characters and the gap rounds up"
    )]
    let (max_chars, gap_chars) = {
        let char_width = char_width.max(1.0);
        (
            (ui.available_width() / char_width) as usize,
            (ui.spacing().item_spacing.x / char_width).ceil() as usize,
        )
    };
    let primary = middle_ellipsis(parts.primary(), max_chars);
    let host = parts
        .secondary()
        .filter(|&host| show_host && host_fits(max_chars, &primary, gap_chars, host));
    ui.add(egui::Label::new(RichText::new(primary).monospace()).truncate())
        .on_hover_text(target);
    if let Some(host) = host {
        ui.add(egui::Label::new(RichText::new(host).monospace().weak()).truncate())
            .on_hover_text(target);
    }
}

/// The status text after the failure tick, with the status class or tunnel
/// note on hover. The tick alone marks exceptions; the text keeps the
/// default color.
fn status_cell(ui: &mut egui::Ui, record: &Exchange, lang: Language) {
    let (text, tone) = outcome_text(record.outcome(), lang);
    ui.spacing_mut().item_spacing.x = Space::S;
    indicator::tick(ui, tone);
    let status = ui.add(egui::Label::new(RichText::new(text).monospace()).truncate());
    if matches!(record.outcome(), Outcome::Tunnel(_)) {
        status.on_hover_text(ShellText::TunnelHint.text(lang));
    } else {
        status.on_hover_text(outcome_summary(record.outcome(), lang));
    }
}

/// Target without the `http://` or `https://` prefix.
fn target_text(record: &Exchange) -> &str {
    let target = record.request().target();
    target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
        .unwrap_or(target)
}

/// Total time once nothing more will be observed, for sorting.
fn duration(record: &Exchange) -> Option<Duration> {
    record.timing().complete()
}

fn size_text(record: &Exchange) -> String {
    record
        .received_bytes()
        .map_or_else(|| "—".to_owned(), |bytes| ByteSize::new(bytes).to_string())
}

fn duration_text(record: &Exchange) -> String {
    use super::outcome_view::elapsed_millis as millis;
    record.timing().complete().map_or_else(
        || format!("{:.0} ms…", millis(record.elapsed())),
        |total| format!("{:.1} ms", millis(total)),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn status_context_filter_uses_numeric_code() -> Result<(), Box<dyn std::error::Error>> {
        use http::{Method, StatusCode};
        use http_inspector_analysis::filter::FilterQuery;
        use http_inspector_exchange::{ExchangeBuilder, ResponseFixture};

        let record = ExchangeBuilder::http(Method::GET, "http://fixture.test/")?
            .respond(ResponseFixture::new(StatusCode::NOT_FOUND))?;
        let (key, value) =
            super::field_filter(super::Field::Status, &record).ok_or("missing filter")?;
        let term = super::filter_menu::field_term(key, &value, false);
        assert_eq!(term, "status:404");
        let parsed = FilterQuery::parse(&term);
        assert!(parsed.diagnostics.is_empty());
        assert!(parsed.matches(&record));
        Ok(())
    }

    use super::*;

    #[test]
    fn moves_within_the_visible_row_order() {
        assert_eq!(move_selection(Key::ArrowDown, None, 0, 3), None);
        assert_eq!(move_selection(Key::End, None, 0, 3), None);
        assert_eq!(move_selection(Key::ArrowDown, None, 8, 3), Some(0));
        assert_eq!(move_selection(Key::ArrowUp, None, 8, 3), Some(7));
        assert_eq!(move_selection(Key::ArrowUp, Some(0), 8, 3), Some(0));
        assert_eq!(move_selection(Key::ArrowDown, Some(7), 8, 3), Some(7));
        assert_eq!(move_selection(Key::Home, Some(5), 8, 3), Some(0));
        assert_eq!(move_selection(Key::End, Some(0), 8, 3), Some(7));
        assert_eq!(move_selection(Key::PageUp, Some(5), 8, 3), Some(2));
        assert_eq!(move_selection(Key::PageUp, Some(2), 8, 3), Some(0));
        assert_eq!(move_selection(Key::PageDown, Some(3), 8, 3), Some(6));
        assert_eq!(move_selection(Key::PageDown, Some(6), 8, 3), Some(7));
    }

    #[tokio::test]
    async fn keyboard_selection_uses_sorted_filtered_records()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::AsyncWriteExt;
        use tokio::net::{TcpListener, TcpStream};

        let upstream = TcpListener::bind("127.0.0.1:0").await?;
        let upstream_addr = upstream.local_addr()?;
        let mut proxy = http_inspector_core::interception::ProxyHandle::start(
            http_inspector_core::interception::ProxyConfig::loopback(0),
        )?;
        let mut records = Vec::new();
        for path in ["visible-z", "hidden", "visible-a", "visible-m"] {
            let mut client = TcpStream::connect(proxy.local_addr()).await?;
            client
                .write_all(
                    format!("GET http://{upstream_addr}/{path} HTTP/1.1\r\nHost: {upstream_addr}\r\n\r\n")
                        .as_bytes(),
                )
                .await?;
            let record = tokio::time::timeout(Duration::from_secs(5), proxy.receive())
                .await?
                .ok_or("capture channel closed")?;
            records.push(record);
        }

        let id_for = |path: &str| {
            records
                .iter()
                .find(|record| record.request().target().ends_with(path))
                .map(Exchange::id)
        };
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut table = RequestTable::new();
        table.sort = table.sort.toggled(Field::Target);
        let mut selected = id_for("/hidden");
        assert!(selected.is_some());
        for (key, editing, expected) in [
            (Key::ArrowDown, false, "/visible-a"),
            (Key::ArrowDown, false, "/visible-m"),
            (Key::End, true, "/visible-m"),
            (Key::End, false, "/visible-z"),
        ] {
            let expected = id_for(expected).ok_or("expected record was not captured")?;
            let input = egui::RawInput {
                events: vec![egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                let mut query = "/visible".to_owned();
                let filter = ui.text_edit_singleline(&mut query);
                if editing {
                    filter.request_focus();
                } else {
                    filter.surrender_focus();
                }
                let rows = records
                    .iter()
                    .filter(|record| record.request().target().contains(query.as_str()))
                    .collect();
                let shortcuts_allowed = crate::ui::request_shortcuts_allowed(ui.ctx());
                if let Some(id) = table
                    .show(ui, rows, selected, shortcuts_allowed, TableMenus::default())
                    .selected
                {
                    selected = Some(id);
                }
            });
            output.textures_delta.clear();
            assert_eq!(selected, Some(expected));
            assert!(!table.follow_newest);
            assert_accessible_requests(&output)?;
        }
        proxy.stop()?;
        Ok(())
    }

    fn assert_accessible_requests(
        output: &egui::FullOutput,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let tree = output
            .platform_output
            .accesskit_update
            .as_ref()
            .ok_or("missing accessibility tree")?;
        assert!(tree.nodes.iter().any(|(_, node)| {
            node.role() == egui::Role::ListBoxOption
                && node
                    .label()
                    .is_some_and(|label| label.contains("GET http://"))
        }));
        assert!(tree.nodes.iter().any(|(_, node)| {
            node.role() == egui::Role::ListBoxOption && node.is_selected() == Some(true)
        }));
        Ok(())
    }

    #[test]
    fn cycles_sort_through_ascending_descending_and_arrival() {
        let arrival = RequestTable::new().sort;
        let ascending = arrival.toggled(Field::Size);
        assert_eq!(ascending.field, Some(Field::Size));
        assert!(!ascending.descending);
        let descending = ascending.toggled(Field::Size);
        assert!(descending.descending);
        assert_eq!(descending.toggled(Field::Size), arrival);
        assert_eq!(descending.toggled(Field::Method).field, Some(Field::Method));
        assert_eq!(ascending.arrow(Field::Size), " ▲");
        assert_eq!(descending.arrow(Field::Size), " ▼");
        assert_eq!(descending.arrow(Field::Method), "");
    }

    #[test]
    fn ranks_responses_by_status_before_other_outcomes() {
        assert!(status_rank(&Outcome::InFlight) > (0, 599));
        assert!(status_rank(&Outcome::Cancelled) > status_rank(&Outcome::InFlight));
    }

    #[test]
    fn column_widths_adapt_between_narrow_and_wide_viewports() {
        let spacing = 8.0;
        let gutter = 14.0;

        // Minimum-width table pane (~360px): Method, Host/Path, Status fit cleanly inside.
        let narrow = column_widths(360.0, spacing, gutter);
        assert!(narrow[0] >= Field::Method.min_width());
        assert!(narrow[1] >= 84.0);
        assert!(narrow[2] >= Field::Status.min_width());
        assert!(spacing.mul_add(2.0, narrow[0] + narrow[1] + narrow[2]) <= 360.0);

        // Medium table pane (~415px): 4 leading columns fit cleanly.
        let medium = column_widths(415.0, spacing, gutter);
        assert!(spacing.mul_add(3.0, medium[0] + medium[1] + medium[2] + medium[3]) <= 415.0);

        // Wide table pane (~740px): all 6 columns fit and Host/Path takes spare space.
        let wide = column_widths(740.0, spacing, gutter);
        let wide_total = spacing.mul_add(5.0, wide.iter().sum::<f32>()) + gutter;
        assert!(wide_total <= 740.0);
        assert!(wide[1] > narrow[1]);
        assert!(wide[1] >= 250.0);
    }

    fn header_left(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<f32> {
        fn find_in_shape(shape: &egui::Shape, label: &str) -> Option<f32> {
            match shape {
                egui::Shape::Text(text) if text.galley.text().trim() == label => {
                    Some(text.visual_bounding_rect().left())
                }
                egui::Shape::Vec(items) => items.iter().find_map(|item| find_in_shape(item, label)),
                _ => None,
            }
        }
        shapes
            .iter()
            .find_map(|clipped| find_in_shape(&clipped.shape, label))
    }

    #[test]
    fn manual_column_resize_expands_column_and_persists() -> Result<(), Box<dyn std::error::Error>>
    {
        let ctx = egui::Context::default();
        let mut table = RequestTable::new();
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(740.0, 300.0));
        let mut run = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(viewport),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        let _ = table.show(ui, Vec::new(), None, false, TableMenus::default());
                    });
            });
            output.textures_delta.clear();
            output.shapes
        };

        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = run(Vec::new());
        }
        let before_target = header_left(&shapes, "Path").ok_or("missing Path header")?;
        let before_size = header_left(&shapes, "Size").ok_or("missing Size header")?;

        // Column 0 resize handle sits halfway across the spacing between Method and Path.
        let handle_x = before_target - 4.0;
        let drag_y = 12.0;
        let _ = run(vec![egui::Event::PointerMoved(egui::pos2(
            handle_x, drag_y,
        ))]);
        let _ = run(vec![egui::Event::PointerButton {
            pos: egui::pos2(handle_x, drag_y),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }]);
        let _ = run(vec![egui::Event::PointerMoved(egui::pos2(
            handle_x + 80.0,
            drag_y,
        ))]);
        let _ = run(vec![egui::Event::PointerButton {
            pos: egui::pos2(handle_x + 80.0, drag_y),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);

        let settled = run(Vec::new());
        let after_target = header_left(&settled, "Path").ok_or("missing Path header after drag")?;
        let after_size = header_left(&settled, "Size").ok_or("missing Size header after drag")?;
        assert!(
            after_target > before_target + 60.0,
            "expected Method column expansion to shift Path right: before={before_target}, after={after_target}"
        );
        assert!(
            after_size > before_size + 60.0,
            "expected later columns to shift right instead of being crushed: before={before_size}, after={after_size}"
        );

        // Width stays preserved on subsequent frames while viewport width is unchanged.
        let next = run(Vec::new());
        let next_target =
            header_left(&next, "Path").ok_or("missing Path header on subsequent frame")?;
        assert!((next_target - after_target).abs() < 1.0);
        Ok(())
    }
}
