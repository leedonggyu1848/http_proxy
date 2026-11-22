//! Network-panel style request list that lays out only the visible rows.

use std::cmp::Ordering;
use std::time::Duration;

use eframe::egui::{self, Key, RichText};
use egui_extras::{Column, TableBuilder};
use http_proxy::{Exchange, Outcome, RecordId};

use crate::ui::copy_curl;
use crate::ui::outcome_view::{outcome_summary, outcome_text, status_rank};

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

    /// Columns truncate their text instead of clipping, because a clipped
    /// cell gets its own clip rectangle and therefore its own draw call.
    fn column(self) -> Column {
        match self {
            Self::Method => Column::initial(56.0).at_least(40.0),
            Self::Target => Column::initial(170.0).at_least(60.0),
            Self::Status => Column::initial(130.0).at_least(64.0),
            Self::Type => Column::initial(44.0).at_least(32.0),
            Self::Size => Column::initial(68.0).at_least(40.0),
            Self::Duration => Column::remainder().at_least(56.0),
        }
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
pub(in crate::ui) struct RequestTable {
    pub(in crate::ui) replay: Option<Exchange>,
    /// Record whose "Copy as cURL" menu item was chosen this frame.
    pub(in crate::ui) copy_curl: Option<Exchange>,
    sort: Sort,
    follow_newest: bool,
    keyboard_row: Option<usize>,
    page_rows: usize,
}

impl RequestTable {
    pub(in crate::ui) const fn new() -> Self {
        Self {
            sort: Sort {
                field: None,
                descending: false,
            },
            follow_newest: true,
            replay: None,
            copy_curl: None,
            keyboard_row: None,
            page_rows: 1,
        }
    }

    /// Shows the list options inline in the filter row.
    pub(in crate::ui) fn controls(&mut self, ui: &mut egui::Ui) {
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
    pub(in crate::ui) fn show(
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

        let mut table = TableBuilder::new(ui)
            .id_salt("requests")
            .striped(true)
            .resizable(true)
            .sense(egui::Sense::click())
            .auto_shrink([false, false])
            .stick_to_bottom(self.follow_newest && self.sort.field.is_none())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for field in Field::ALL {
            table = table.column(field.column());
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
                            .add(egui::Button::new(RichText::new(title).strong()).frame(false))
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
                        changed = Some(record.id());
                    }
                    self.row_menu(&row.response(), record);
                    if row.response().clicked() {
                        self.keyboard_row = None;
                        changed = Some(record.id());
                    }
                });
            });
        let row_stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "The viewport height is nonnegative and UI-sized, and a page counts only full rows"
        )]
        let page_rows = (body.inner_rect.height() / row_stride) as usize;
        self.page_rows = page_rows.max(1);
        changed
    }
}

impl RequestTable {
    /// Offers resending or copying the row's request on right click.
    fn row_menu(&mut self, response: &egui::Response, record: &Exchange) {
        response.context_menu(|ui| {
            if ui
                .add_enabled(
                    record.source() != http_proxy::CaptureSource::Connect,
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
        });
    }
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
            let response = ui.selectable_label(selected, record.request().method().as_str());
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
        let (text, color) = outcome_text(record.outcome(), &crate::ui::theme::Palette::of(ui));
        let status = ui.add(egui::Label::new(RichText::new(text).color(color)).truncate());
        if matches!(record.outcome(), Outcome::Tunnel(_)) {
            status.on_hover_text(super::scope_help::TUNNEL_NOTE);
        } else {
            status.on_hover_text(outcome_summary(record.outcome()));
        }
    });
    row.col(|ui| {
        ui.add(egui::Label::new(content_type(record).unwrap_or("—")).truncate());
    });
    row.col(|ui| {
        ui.monospace(
            record
                .received_bytes()
                .map_or_else(|| "—".to_owned(), format_bytes),
        );
    });
    row.col(|ui| {
        ui.monospace(duration_text(record));
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

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    let scaled = |unit: u64| {
        let whole = bytes / unit;
        let tenths = (bytes % unit) * 10 / unit;
        format!("{whole}.{tenths}")
    };
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{} KiB", scaled(KIB))
    } else {
        format!("{} MiB", scaled(MIB))
    }
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
        let mut proxy = http_proxy::ProxyHandle::start(http_proxy::ProxyConfig::loopback(0))?;
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
                    .filter(|record| {
                        crate::ui::filter::record_matches(
                            record,
                            crate::ui::filter::RecordFilter::All,
                            &query,
                        )
                    })
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
    fn formats_sizes_with_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(4 * 1024 * 1024), "4.0 MiB");
    }

    #[test]
    fn ranks_responses_by_status_before_other_outcomes() {
        assert!(status_rank(&Outcome::InFlight) > (0, 599));
        assert!(status_rank(&Outcome::Cancelled) > status_rank(&Outcome::InFlight));
    }
}
