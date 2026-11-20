//! Manual replay editor. Sensitive inputs live only as long as the editor.
use eframe::egui::{self, Color32};
use http_proxy::{Exchange, ProxyHandle, ReplayDraft, ReplayField, ReplayTask};
use std::fmt::Write as _;

pub(crate) struct ReplayPanel {
    draft: ReplayDraft,
    task: Option<ReplayTask>,
    error: Option<String>,
    confirmed: bool,
    skip_confirmation: bool,
    file: String,
    compare: bool,
    query_url: String,
    query: Vec<ReplayField>,
}

impl ReplayPanel {
    pub(crate) fn new(original: Exchange) -> Result<Self, String> {
        Ok(Self {
            draft: ReplayDraft::new(original)?,
            task: None,
            error: None,
            confirmed: false,
            skip_confirmation: false,
            file: String::new(),
            compare: false,
            query_url: String::new(),
            query: Vec::new(),
        })
    }

    pub(crate) fn show(
        &mut self,
        context: &egui::Context,
        proxy: Option<&ProxyHandle>,
        records: &[Exchange],
    ) -> bool {
        let mut open = true;
        egui::Window::new("Edit and Resend")
            .open(&mut open)
            .default_width(650.0)
            .default_height(600.0)
            .vscroll(true)
            .show(context, |ui| self.contents(ui, proxy, records));
        open
    }

    fn contents(&mut self, ui: &mut egui::Ui, proxy: Option<&ProxyHandle>, records: &[Exchange]) {
        ui.label(format!("Original #{}", self.draft.original.id().value()));
        ui.label("Redacted values must be entered or disabled. Hop-by-hop headers are removed; Content-Length is calculated.");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("replay-method")
                .selected_text(&self.draft.method)
                .show_ui(ui, |ui| {
                    for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
                        ui.selectable_value(&mut self.draft.method, method.to_owned(), method);
                    }
                });
            ui.text_edit_singleline(&mut self.draft.url);
        });
        self.query_ui(ui);
        ui.checkbox(&mut self.draft.automatic_host, "Update Host from URL");
        ui.strong("Headers (Cookie: name=value; name2=value2)");
        fields(ui, &mut self.draft.headers);
        ui.collapsing("Cookies", |ui| {
            for field in self
                .draft
                .headers
                .iter_mut()
                .filter(|field| field.name.eq_ignore_ascii_case("cookie"))
            {
                let mut rows = pairs(&field.value, ';');
                fields(ui, &mut rows);
                field.value = join_pairs(&rows, "; ");
            }
            if ui.button("Add Cookie header").clicked() {
                self.draft.headers.push(ReplayField {
                    enabled: true,
                    name: "cookie".to_owned(),
                    value: String::new(),
                });
            }
        });
        self.body_ui(ui);
        self.send_ui(ui, proxy);
        self.comparison_ui(ui, records);
    }
    fn query_ui(&mut self, ui: &mut egui::Ui) {
        if self.query_url != self.draft.url {
            self.query = pairs(
                self.draft
                    .url
                    .split_once('?')
                    .map_or("", |(_, query)| query),
                '&',
            );
            self.query_url.clone_from(&self.draft.url);
        }
        ui.collapsing("Query parameters (URL-encoded)", |ui| {
            fields(ui, &mut self.query);
            let base = self
                .draft
                .url
                .split_once('?')
                .map_or(self.draft.url.as_str(), |(base, _)| base);
            let query = join_pairs(&self.query, "&");
            self.draft.url = if query.is_empty() {
                base.to_owned()
            } else {
                format!("{base}?{query}")
            };
            self.query_url.clone_from(&self.draft.url);
        });
    }

    fn body_ui(&mut self, ui: &mut egui::Ui) {
        ui.strong("Body");
        let content_type = self
            .draft
            .headers
            .iter()
            .find(|field| field.enabled && field.name.eq_ignore_ascii_case("content-type"))
            .map_or_default(|field| field.value.clone());
        if content_type.starts_with("application/json") {
            match serde_json::from_slice::<serde_json::Value>(&self.draft.body) {
                Ok(_) => {
                    ui.label("Valid JSON");
                }
                Err(error) => {
                    ui.colored_label(Color32::YELLOW, error.to_string());
                }
            }
        }
        if content_type.starts_with("application/x-www-form-urlencoded") {
            ui.collapsing("Form fields (URL-encoded)", |ui| {
                if let Ok(text) = std::str::from_utf8(&self.draft.body) {
                    let mut rows = pairs(text, '&');
                    fields(ui, &mut rows);
                    self.draft.body = join_pairs(&rows, "&").into_bytes();
                }
            });
        }
        if let Ok(mut text) = String::from_utf8(self.draft.body.clone()) {
            if ui
                .add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                self.draft.body = text.into_bytes();
            }
        } else {
            ui.label(format!(
                "Binary body: {} bytes. Load a replacement file to edit.",
                self.draft.body.len()
            ));
        }
        ui.horizontal(|ui| {
            ui.text_edit_singleline(&mut self.file);
            if ui.button("Load complete body file").clicked() {
                match std::fs::read(&self.file) {
                    Ok(body) => {
                        self.draft.body = body;
                        self.draft.body_missing = self.draft.original.request().headers().omitted();
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        });
        if self.draft.body_missing {
            ui.colored_label(Color32::YELLOW, "Capture is incomplete. Load the full body; omitted headers require another complete capture.");
        }
    }
    fn send_ui(&mut self, ui: &mut egui::Ui, proxy: Option<&ProxyHandle>) {
        if self.draft.needs_confirmation() && !self.skip_confirmation {
            ui.checkbox(
                &mut self.confirmed,
                "I confirm this request may change server state",
            );
            if self.confirmed {
                ui.checkbox(
                    &mut self.skip_confirmation,
                    "Do not ask again while this editor is open",
                );
            }
        }
        let running = self.task.as_ref().is_some_and(|task| !task.is_finished());
        ui.horizontal(|ui| {
            let approved =
                !self.draft.needs_confirmation() || self.confirmed || self.skip_confirmation;
            if ui
                .add_enabled(
                    !running && approved && proxy.is_some(),
                    egui::Button::new("Send"),
                )
                .clicked()
                && let Some(proxy) = proxy
            {
                match proxy.replay(&self.draft) {
                    Ok(task) => {
                        self.task = Some(task);
                        self.error = None;
                        self.confirmed = false;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            if running
                && ui.button("Cancel").clicked()
                && let Some(task) = &self.task
            {
                task.cancel();
            }
            if ui.button("Reset to original").clicked() {
                match ReplayDraft::new(self.draft.original.clone()) {
                    Ok(draft) => self.draft = draft,
                    Err(error) => self.error = Some(error),
                }
            }
            ui.checkbox(&mut self.compare, "Compare with original");
        });
        if let Some(error) = &self.error {
            ui.colored_label(Color32::LIGHT_RED, error);
        }
    }
    fn comparison_ui(&self, ui: &mut egui::Ui, records: &[Exchange]) {
        if self.compare {
            let original = &self.draft.original;
            if let Some(result) = records
                .iter()
                .rev()
                .find(|record| record.replay_of() == Some(original.id()))
            {
                ui.columns(2, |columns| {
                    for (column, record) in columns.iter_mut().zip([original, result]) {
                        column.strong(format!(
                            "#{} · {:.1} ms",
                            record.id().value(),
                            record.elapsed().as_secs_f64() * 1000.0
                        ));
                        let original_text = summary(if record.id() == original.id() {
                            result
                        } else {
                            original
                        });
                        let text = summary(record);
                        for (index, line) in text.lines().enumerate() {
                            let changed = original_text.lines().nth(index) != Some(line);
                            column.colored_label(
                                if changed {
                                    if column.visuals().dark_mode {
                                        Color32::LIGHT_YELLOW
                                    } else {
                                        Color32::from_rgb(140, 80, 0)
                                    }
                                } else {
                                    column.visuals().text_color()
                                },
                                line,
                            );
                        }
                    }
                });
            } else {
                ui.label("Send a request to compare its captured result.");
            }
        }
    }
}

impl Drop for ReplayPanel {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.cancel();
        }
    }
}

fn fields(ui: &mut egui::Ui, fields: &mut Vec<ReplayField>) {
    let mut remove = None;
    for (index, field) in fields.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut field.enabled, "");
                ui.text_edit_singleline(&mut field.name);
                ui.add(
                    egui::TextEdit::singleline(&mut field.value).password(matches!(
                        field.name.to_lowercase().as_str(),
                        "authorization" | "cookie" | "proxy-authorization" | "set-cookie"
                    )),
                );
                if ui.button("−").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        fields.remove(index);
    }
    if ui.button("Add header").clicked() {
        fields.push(ReplayField {
            enabled: true,
            name: String::new(),
            value: String::new(),
        });
    }
}

fn summary(record: &Exchange) -> String {
    let mut text = format!(
        "{} {}\n",
        record.request().method(),
        record.request().target()
    );
    for field in record.request().headers().entries() {
        let _ = writeln!(text, "{}: {}", field.name(), field.value());
    }
    if let Some(body) = record
        .request()
        .body()
        .and_then(http_proxy::BodyPreview::snapshot)
    {
        text.push_str(&String::from_utf8_lossy(body.bytes()));
        text.push('\n');
    }
    if let http_proxy::Outcome::Response(response) = record.outcome() {
        let _ = writeln!(text, "Status {}", response.status());
        for field in response.headers().entries() {
            let _ = writeln!(text, "{}: {}", field.name(), field.value());
        }
        if let Some(body) = response.body().snapshot() {
            text.push_str(&String::from_utf8_lossy(body.bytes()));
            if body.observed_bytes() != body.bytes().len() as u64 {
                text.push_str("\n[Partial body preview]");
            }
        }
    } else {
        let _ = write!(text, "{:?}", record.outcome());
    }
    text
}

fn pairs(text: &str, separator: char) -> Vec<ReplayField> {
    text.split(separator)
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (name, value) = part
                .trim()
                .split_once('=')
                .unwrap_or_else(|| (part.trim(), ""));
            ReplayField {
                enabled: true,
                name: name.to_owned(),
                value: value.to_owned(),
            }
        })
        .collect()
}

fn join_pairs(rows: &[ReplayField], separator: &str) -> String {
    rows.iter()
        .filter(|row| row.enabled)
        .map(|row| format!("{}={}", row.name, row.value))
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_encoded_query_and_cookie_values() {
        assert_eq!(
            join_pairs(&pairs("a=x%26y&a=two%20words", '&'), "&"),
            "a=x%26y&a=two%20words"
        );
        assert_eq!(
            join_pairs(&pairs("session=a=b; theme=dark", ';'), "; "),
            "session=a=b; theme=dark"
        );
    }
}
