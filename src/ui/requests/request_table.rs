//! Network-panel style request list that lays out only the visible rows.

use std::cmp::Ordering;
use std::time::Duration;

use eframe::egui::{self, Key, RichText};
use egui_extras::{Column, TableBuilder};
use http_inspector_exchange::{ByteSize, Exchange, Outcome, RecordId};

use crate::ui::dialogs::copy_curl;
use crate::ui::requests::outcome_view::{outcome_summary, outcome_text, status_rank};

const ROW_HEIGHT: f32 = 20.0;
const HEADER_HEIGHT: f32 = 22.0;

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

    const fn title(self) -> &'static str {
        match self {
            Self::Method => "Method",
            Self::Target => "Host / Path",
            Self::Status => "Status",
            Self::Type => "Type",
            Self::Size => "Size",
            Self::Duration => "Duration",
        }
    }

    /// Minimum width kept even if the user drags a column narrow.
    const fn min_width(self) -> f32 {
        match self {
            Self::Method | Self::Size => 40.0,
            Self::Target => 60.0,
            Self::Status => 64.0,
            Self::Type => 32.0,
            Self::Duration => 52.0,
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

/// Sort state and scrolling preference of the request list.
pub(crate) struct RequestTable {
    pub(super) replay: Option<Exchange>,
    /// Record whose "Copy as cURL" menu item was chosen this frame.
    pub(super) copy_curl: Option<Exchange>,
    /// A body save chosen from a row's context menu.
    pub(super) save_body: Option<(Exchange, http_inspector_exchange::BodySide)>,
    /// Whether a body save is running, which disables the save menu items.
    pub(super) saving_body: bool,
    sort: Sort,
    follow_newest: bool,
    keyboard_row: Option<usize>,
    page_rows: usize,
    last_viewport_width: f32,
}

impl RequestTable {
    pub(crate) const fn new() -> Self {
        Self {
            sort: Sort {
                field: None,
                descending: false,
            },
            follow_newest: true,
            replay: None,
            copy_curl: None,
            save_body: None,
            saving_body: false,
            keyboard_row: None,
            page_rows: 1,
            last_viewport_width: 0.0,
        }
    }

    /// Shows the list options inline in the filter row.
    pub(super) fn controls(&mut self, ui: &mut egui::Ui) {
        ui.checkbox(&mut self.follow_newest, "Follow newest")
            .on_hover_text("Keep the newest request in view while sorted by arrival.");
        if self.follow_newest {
            self.keyboard_row = None;
        }
        if self.sort.field.is_some() && ui.button("Arrival order").clicked() {
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
    ) -> Option<RecordId> {
        self.sort_rows(&mut rows);
        let mut changed = None;
        let mut scroll_to = None;
        if shortcuts_allowed || self.keyboard_row.is_some() {
            let current = rows.iter().position(|record| Some(record.id()) == selected);
            if shortcuts_allowed
                && let Some(index) = keyboard_selection(ui, current, rows.len(), self.page_rows)
                && let Some(record) = rows.get(index)
            {
                self.follow_newest = false;
                self.keyboard_row = Some(index);
                changed = Some(record.id());
                selected = changed;
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
        let row_stride = ROW_HEIGHT + ui.spacing().item_spacing.y;

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
                    &mut changed,
                )
            })
            .inner;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "The viewport height is nonnegative and UI-sized, and a page counts only full rows"
        )]
        let page_rows = (inner_height / row_stride) as usize;
        self.page_rows = page_rows.max(1);
        changed
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
        changed: &mut Option<RecordId>,
    ) -> f32 {
        let mut table = TableBuilder::new(ui)
            .id_salt("requests")
            .striped(true)
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
        let body = table
            .header(HEADER_HEIGHT, |mut header| {
                for field in Field::ALL {
                    header.col(|ui| {
                        let title = format!("{}{}", field.title(), self.sort.arrow(field));
                        if ui
                            .add(
                                egui::Button::new(RichText::new(title).strong())
                                    .frame(false)
                                    .truncate(),
                            )
                            .clicked()
                        {
                            self.sort = self.sort.toggled(field);
                        }
                    });
                }
            })
            .body(|mut body| {
                // Let the table cells handle row clicks while labels still show tooltips.
                body.ui_mut().style_mut().interaction.selectable_labels = false;
                body.rows(ROW_HEIGHT, rows.len(), |mut row| {
                    let Some(record) = rows.get(row.index()) else {
                        return;
                    };
                    row.set_selected(selected == Some(record.id()));
                    let selected_row = selected == Some(record.id());
                    let focus_row = scroll_to == Some(row.index());
                    if show_cells(&mut row, record, selected_row, focus_row) {
                        self.follow_newest = false;
                        self.keyboard_row = None;
                        *changed = Some(record.id());
                    }
                    self.row_menu(&row.response(), record);
                    if row.response().clicked() {
                        self.keyboard_row = None;
                        *changed = Some(record.id());
                    }
                });
            });
        body.inner_rect.height()
    }
}

/// Computes responsive initial column widths for `viewport_width`.
/// When all six columns fit, `Host / Path` takes the spare width. When the
/// table is narrower, `Host / Path` fills the space beside the leading columns
/// that fit cleanly without slicing the next column at the pane border, while
/// the remaining columns stay reachable via horizontal scrolling.
fn column_widths(viewport_width: f32, spacing_x: f32, scroll_gutter: f32) -> [f32; 6] {
    const METHOD: f32 = 78.0;
    const STATUS_WIDE: f32 = 112.0;
    const STATUS_COMPACT: f32 = 104.0;
    const TYPE: f32 = 48.0;
    const SIZE: f32 = 68.0;
    const DURATION: f32 = 68.0;
    const MIN_TARGET: f32 = 140.0;
    const EDGE_PAD: f32 = 4.0;

    let usable = (viewport_width - scroll_gutter).max(0.0);
    let fixed_wide = METHOD + STATUS_WIDE + TYPE + SIZE + DURATION;
    let all_spacing = spacing_x * 5.0;
    if usable >= fixed_wide + all_spacing + MIN_TARGET {
        let target = (usable - fixed_wide - all_spacing).floor();
        return [METHOD, target, STATUS_WIDE, TYPE, SIZE, DURATION];
    }

    let visible = viewport_width.max(0.0);
    let fixed_five = spacing_x.mul_add(4.0, METHOD + STATUS_COMPACT + TYPE + SIZE) + EDGE_PAD;
    if visible >= fixed_five + MIN_TARGET {
        let target = (visible - fixed_five).floor();
        return [METHOD, target, STATUS_COMPACT, TYPE, SIZE, DURATION];
    }

    let fixed_four = spacing_x.mul_add(3.0, METHOD + STATUS_COMPACT + TYPE) + EDGE_PAD;
    if visible >= fixed_four + MIN_TARGET {
        let target = (visible - fixed_four).floor();
        return [METHOD, target, STATUS_COMPACT, TYPE, SIZE, DURATION];
    }

    let method = METHOD.min((visible * 0.22).floor().max(Field::Method.min_width()));
    let status = STATUS_COMPACT.min((visible * 0.30).floor().max(Field::Status.min_width()));
    let target = spacing_x
        .mul_add(-2.0, visible - method - status - EDGE_PAD)
        .floor()
        .clamp(84.0, 220.0);
    [method, target, status, TYPE, SIZE, DURATION]
}

impl RequestTable {
    /// Offers resending, copying, or saving a body of the row's request on right click.
    fn row_menu(&mut self, response: &egui::Response, record: &Exchange) {
        response.context_menu(|ui| {
            if ui
                .add_enabled(
                    record.source() != http_inspector_exchange::CaptureSource::Connect,
                    egui::Button::new("Edit and Resend"),
                )
                .clicked()
            {
                self.replay = Some(record.clone());
                ui.close();
            }
            if copy_curl::copy_button(ui, record).clicked() {
                self.copy_curl = Some(record.clone());
                ui.close();
            }
            ui.separator();
            if let Some(side) = save_body_items(ui, record, !self.saving_body) {
                self.save_body = Some((record.clone(), side));
                ui.close();
            }
        });
    }
}

/// The body save items of the row menu; returns the side chosen.
/// They are disabled while `idle` is false, when another save is running.
fn save_body_items(
    ui: &mut egui::Ui,
    record: &Exchange,
    idle: bool,
) -> Option<http_inspector_exchange::BodySide> {
    let has_response = matches!(
        record.outcome(),
        http_inspector_exchange::Outcome::Response(_)
    );
    let mut chosen = None;
    for (side, label, enabled) in [
        (
            http_inspector_exchange::BodySide::Response,
            "Save response body",
            has_response,
        ),
        (
            http_inspector_exchange::BodySide::Request,
            "Save request body",
            record.request().body().is_some(),
        ),
    ] {
        if ui
            .add_enabled(enabled && idle, egui::Button::new(label))
            .on_hover_text("Save the decoded body to the Save directory.")
            .on_disabled_hover_text(
                "Unavailable while another body is being saved, or when the body was not observed.",
            )
            .clicked()
        {
            chosen = Some(side);
        }
    }
    chosen
}

fn show_cells(
    row: &mut egui_extras::TableRow<'_, '_>,
    record: &Exchange,
    selected: bool,
    focus: bool,
) -> bool {
    let mut clicked = false;
    row.col(|ui| {
        ui.push_id(record.id(), |ui| {
            let method = record.request().method().as_str();
            let response = ui
                .add(egui::Button::selectable(selected, method).truncate())
                .on_hover_text(method);
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::ListBoxOption,
                    ui.is_enabled(),
                    selected,
                    format!(
                        "{} {} · {}",
                        record.request().method(),
                        record.request().target(),
                        outcome_summary(record.outcome()),
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
            clicked = response.clicked();
        });
    });
    row.col(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(record.replay_of().map_or_else(
                    || target_text(record).to_owned(),
                    |original| format!("Replay of #{} · {}", original.value(), target_text(record)),
                ))
                .monospace(),
            )
            .truncate(),
        )
        .on_hover_text(record.request().target());
    });
    row.col(|ui| {
        let (text, color) =
            outcome_text(record.outcome(), &crate::ui::style::theme::Palette::of(ui));
        let status = ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
        if matches!(record.outcome(), Outcome::Tunnel(_)) {
            status.on_hover_text(crate::ui::dialogs::scope_help::TUNNEL_NOTE);
        } else {
            status.on_hover_text(outcome_summary(record.outcome()));
        }
    });
    row.col(|ui| {
        ui.add(egui::Label::new(content_type(record).unwrap_or("—")).truncate());
    });
    row.col(|ui| {
        ui.add(
            egui::Label::new(
                RichText::new(
                    record
                        .received_bytes()
                        .map_or_else(|| "—".to_owned(), |bytes| ByteSize::new(bytes).to_string()),
                )
                .monospace(),
            )
            .truncate(),
        );
    });
    row.col(|ui| {
        ui.add(egui::Label::new(RichText::new(duration_text(record)).monospace()).truncate());
    });
    clicked
}

/// Target without the `http://` or `https://` prefix.
fn target_text(record: &Exchange) -> &str {
    let target = record.request().target();
    target
        .strip_prefix("http://")
        .or_else(|| target.strip_prefix("https://"))
        .unwrap_or(target)
}

/// Short media type such as `json` or `html`, from the response `Content-Type`.
fn content_type(record: &Exchange) -> Option<&str> {
    let Outcome::Response(response) = record.outcome() else {
        return None;
    };
    let media_type = response.media_type()?;
    let subtype = media_type.subtype();
    let subtype = subtype.rsplit('+').next().unwrap_or(subtype);
    Some(subtype.strip_prefix("x-").unwrap_or(subtype))
}

/// Total time once nothing more will be observed, for sorting.
fn duration(record: &Exchange) -> Option<Duration> {
    record.timing().complete()
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
                if let Some(id) = table.show(ui, rows, selected, shortcuts_allowed) {
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
                        let _ = table.show(ui, Vec::new(), None, false);
                    });
            });
            output.textures_delta.clear();
            output.shapes
        };

        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = run(Vec::new());
        }
        let before_target =
            header_left(&shapes, "Host / Path").ok_or("missing Host / Path header")?;
        let before_size = header_left(&shapes, "Size").ok_or("missing Size header")?;

        // Column 0 resize handle sits halfway across the spacing between Method and Host / Path.
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
        let after_target =
            header_left(&settled, "Host / Path").ok_or("missing Host / Path header after drag")?;
        let after_size = header_left(&settled, "Size").ok_or("missing Size header after drag")?;
        assert!(
            after_target > before_target + 60.0,
            "expected Method column expansion to shift Host / Path right: before={before_target}, after={after_target}"
        );
        assert!(
            after_size > before_size + 60.0,
            "expected later columns to shift right instead of being crushed: before={before_size}, after={after_size}"
        );

        // Width stays preserved on subsequent frames while viewport width is unchanged.
        let next = run(Vec::new());
        let next_target = header_left(&next, "Host / Path")
            .ok_or("missing Host / Path header on subsequent frame")?;
        assert!((next_target - after_target).abs() < 1.0);
        Ok(())
    }
}
