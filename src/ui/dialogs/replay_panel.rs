//! Manual replay editor. Sensitive inputs live only as long as the editor.
use crate::ui::i18n::{Language, Text, domain};
use crate::ui::style::{theme::Palette, tokens::Layout};
use eframe::egui;
use http_inspector_core::interception::ProxyHandle;
use http_inspector_core::replay::{
    ReplayBodyFileError, ReplayDraft, ReplayError, ReplayField, ReplayTask,
};
use http_inspector_exchange::http_semantics::urlencoded::{self, RawPair};
use http_inspector_exchange::{Exchange, RecordId};
use std::fmt::Write as _;
use std::path::Path;

/// Bodies above this size are summarized instead of laid out in the text editor.
const EDITABLE_BODY_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
pub(crate) enum ReplayAction {
    LoadBody,
    Send,
    Cancel,
    Reset,
    Close,
}

enum ReplayPanelError {
    Replay(ReplayError),
    BodyFile(ReplayBodyFileError),
}

impl ReplayPanelError {
    fn localized(&self, lang: Language) -> String {
        match self {
            Self::Replay(err) => domain::replay_error(lang, err),
            Self::BodyFile(err) => domain::replay_body_file_error(lang, err),
        }
    }
}

pub(crate) struct ReplayPanel {
    draft: ReplayDraft,
    task: Option<ReplayTask>,
    error: Option<ReplayPanelError>,
    confirmed: bool,
    skip_confirmation: bool,
    file: String,
    /// Where the current body came from, when it was loaded from a file.
    body_source: Option<String>,
    body: BodyView,
    compare: bool,
    query: PairTable,
    cookies: Vec<PairTable>,
    form: PairTable,
}

impl ReplayPanel {
    pub(crate) fn new(original: Exchange) -> Result<Self, ReplayError> {
        let draft = ReplayDraft::new(original)?;
        Ok(Self {
            body: BodyView::new(&draft.body),
            draft,
            task: None,
            error: None,
            confirmed: false,
            skip_confirmation: false,
            file: String::new(),
            body_source: None,
            compare: false,
            query: PairTable::default(),
            cookies: Vec::new(),
            form: PairTable::default(),
        })
    }

    pub(crate) fn original_id(&self) -> RecordId {
        self.draft.original.id()
    }

    pub(crate) fn show(
        &mut self,
        context: &egui::Context,
        proxy_running: bool,
        latest: Option<&Exchange>,
        lang: Language,
    ) -> Vec<ReplayAction> {
        let mut actions = Vec::new();
        let mut open = true;
        let viewport = context.content_rect();
        let max_width = Layout::REPLAY_WIDTH.fit(viewport.width());
        let max_height = Layout::REPLAY_HEIGHT.fit(viewport.height());
        egui::Window::new(lang.text(Text::CtxEditAndResend))
            .id(egui::Id::unique("edit-and-resend"))
            .open(&mut open)
            .default_width(Layout::REPLAY_DIALOG.x.min(max_width))
            .default_height(Layout::REPLAY_DIALOG.y.min(max_height))
            .max_width(max_width)
            .max_height(max_height)
            .vscroll(true)
            .show(context, |ui| {
                self.contents(ui, proxy_running, latest, lang, &mut actions);
            });
        if !open {
            actions.push(ReplayAction::Close);
        }
        actions
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        proxy_running: bool,
        latest: Option<&Exchange>,
        lang: Language,
        actions: &mut Vec<ReplayAction>,
    ) {
        let orig_id = self.draft.original.id().value();
        let orig_label = match lang {
            Language::English => format!("Original #{orig_id}"),
            Language::Korean => format!("원본 #{orig_id}"),
        };
        ui.label(orig_label);
        ui.label(lang.pick(
            "Redacted values must be entered or disabled. Hop-by-hop headers are removed; Content-Length is calculated.",
            "가려진 값은 직접 입력하거나 비활성화해야 합니다. Hop-by-hop 헤더는 제거되며 Content-Length는 자동으로 계산됩니다.",
        ));
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("replay-method")
                .selected_text(&self.draft.method)
                .show_ui(ui, |ui| {
                    for method in ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
                        ui.selectable_value(&mut self.draft.method, method.to_owned(), method);
                    }
                });
            let url_width = Layout::REPLAY_URL_WIDTH.fit(ui.available_width());
            ui.add(egui::TextEdit::singleline(&mut self.draft.url).desired_width(url_width));
        });
        self.query_ui(ui, lang);
        ui.checkbox(
            &mut self.draft.automatic_host,
            lang.pick("Update Host from URL", "URL에 맞춰 Host 자동 갱신"),
        );
        ui.strong(lang.pick(
            "Headers (Cookie: name=value; name2=value2)",
            "헤더 (Cookie: name=value; name2=value2)",
        ));
        fields(ui, &mut self.draft.headers, lang);
        ui.collapsing(lang.pick("Cookies", "쿠키"), |ui| {
            let is_cookie = |field: &ReplayField| field.name.eq_ignore_ascii_case("cookie");
            let count = self.draft.headers.iter().filter(|f| is_cookie(f)).count();
            self.cookies.resize_with(count, PairTable::default);
            let cookies = self.draft.headers.iter_mut().filter(|f| is_cookie(f));
            for (index, (field, table)) in cookies.zip(&mut self.cookies).enumerate() {
                ui.push_id(("cookie", index), |ui| {
                    table.show(
                        ui,
                        &mut field.value,
                        PairSyntax::Cookie,
                        lang.pick("Add cookie", "쿠키 추가"),
                    );
                });
            }
            if ui
                .button(lang.pick("Add Cookie header", "Cookie 헤더 추가"))
                .clicked()
            {
                self.draft.headers.push(ReplayField {
                    enabled: true,
                    name: "cookie".to_owned(),
                    value: String::new(),
                });
            }
        });
        self.body_ui(ui, lang, actions);
        self.send_ui(ui, proxy_running, lang, actions);
        self.comparison_ui(ui, latest, lang);
    }

    fn query_ui(&mut self, ui: &mut egui::Ui, lang: Language) {
        ui.collapsing(
            lang.pick(
                "Query parameters (URL-encoded)",
                "쿼리 파라미터 (URL 인코딩)",
            ),
            |ui| {
                edit_query(&mut self.draft.url, |query| {
                    self.query.show(
                        ui,
                        query,
                        PairSyntax::UrlEncoded,
                        lang.pick("Add parameter", "파라미터 추가"),
                    )
                });
            },
        );
    }

    fn body_ui(&mut self, ui: &mut egui::Ui, lang: Language, actions: &mut Vec<ReplayAction>) {
        ui.strong(lang.pick("Body", "본문"));
        let content_type = self
            .draft
            .headers
            .iter()
            .find(|field| field.enabled && field.name.eq_ignore_ascii_case("content-type"))
            .map_or_default(|field| field.value.clone());
        if content_type.starts_with("application/json") {
            match self.body.json_error(&self.draft.body) {
                None => {
                    ui.label(lang.pick("Valid JSON", "유효한 JSON"));
                }
                Some(error) => {
                    ui.colored_label(crate::ui::style::theme::Palette::of(ui).warning, error);
                }
            }
        }
        let mut edited = false;
        if let Some(text) = &mut self.body.text {
            if content_type.starts_with("application/x-www-form-urlencoded") {
                ui.collapsing(
                    lang.pick("Form fields (URL-encoded)", "폼 필드 (URL 인코딩)"),
                    |ui| {
                        edited |= self.form.show(
                            ui,
                            text,
                            PairSyntax::UrlEncoded,
                            lang.pick("Add field", "필드 추가"),
                        );
                    },
                );
            }
            edited |= ui
                .add(
                    egui::TextEdit::multiline(text)
                        .desired_rows(6)
                        .desired_width(f32::INFINITY),
                )
                .changed();
        } else {
            ui.label(self.non_editable_body_note(lang));
        }
        if edited && let Some(text) = &self.body.text {
            self.draft.body = text.clone().into();
            self.body.json = None;
            self.body_source = None;
        }
        ui.horizontal_wrapped(|ui| {
            let file_width = Layout::REPLAY_FILE_WIDTH.fit(ui.available_width());
            ui.add(egui::TextEdit::singleline(&mut self.file).desired_width(file_width));
            if ui
                .button(lang.pick("Load complete body file", "전체 본문 파일 불러오기"))
                .clicked()
            {
                actions.push(ReplayAction::LoadBody);
            }
        });
        if self.draft.body_missing {
            ui.colored_label(
                crate::ui::style::theme::Palette::of(ui).warning,
                lang.pick(
                    "Capture is incomplete. Load the full body; omitted headers require another complete capture.",
                    "캡처가 불완전합니다. 전체 본문 파일을 불러오세요. 누락된 헤더는 완전한 캡처가 필요합니다.",
                ),
            );
        }
    }

    fn non_editable_body_note(&self, lang: Language) -> String {
        let size = self.draft.body.len();
        match (lang, self.body_source.as_deref()) {
            (Language::English, Some(file)) if size > EDITABLE_BODY_BYTES => format!(
                "{size} bytes loaded from {file}. Too large to edit here; it is sent as loaded."
            ),
            (Language::English, None) if size > EDITABLE_BODY_BYTES => {
                format!("{size} bytes. Too large to edit here; it is sent as loaded.")
            }
            (Language::Korean, Some(file)) if size > EDITABLE_BODY_BYTES => format!(
                "{file}에서 불러온 {size}바이트입니다. 여기서 편집하기엔 크기가 커서 불러온 그대로 전송됩니다."
            ),
            (Language::Korean, None) if size > EDITABLE_BODY_BYTES => format!(
                "{size}바이트입니다. 여기서 편집하기엔 크기가 커서 불러온 그대로 전송됩니다."
            ),
            (Language::English, Some(file)) => format!(
                "Binary body: {size} bytes loaded from {file}. Load a replacement file to edit."
            ),
            (Language::English, None) => {
                format!("Binary body: {size} bytes. Load a replacement file to edit.")
            }
            (Language::Korean, Some(file)) => format!(
                "바이너리 본문: {file}에서 불러온 {size}바이트. 변경하려면 대체 파일을 불러오세요."
            ),
            (Language::Korean, None) => {
                format!("바이너리 본문: {size}바이트. 변경하려면 대체 파일을 불러오세요.")
            }
        }
    }

    fn send_ui(
        &mut self,
        ui: &mut egui::Ui,
        proxy_running: bool,
        lang: Language,
        actions: &mut Vec<ReplayAction>,
    ) {
        if self.draft.needs_confirmation() && !self.skip_confirmation {
            ui.checkbox(
                &mut self.confirmed,
                lang.pick(
                    "I confirm this request may change server state",
                    "이 요청이 서버 상태를 변경할 수 있음을 확인했습니다",
                ),
            );
            if self.confirmed {
                ui.checkbox(
                    &mut self.skip_confirmation,
                    lang.pick(
                        "Do not ask again while this editor is open",
                        "이 편집기가 열려 있는 동안 다시 묻지 않기",
                    ),
                );
            }
        }
        let running = self.task.as_ref().is_some_and(|task| !task.is_finished());
        ui.horizontal_wrapped(|ui| {
            let approved =
                !self.draft.needs_confirmation() || self.confirmed || self.skip_confirmation;
            if ui
                .add_enabled(
                    !running && approved && proxy_running,
                    egui::Button::new(lang.pick("Send", "전송")),
                )
                .clicked()
            {
                actions.push(ReplayAction::Send);
            }
            if running && ui.button(lang.text(Text::Cancel)).clicked() {
                actions.push(ReplayAction::Cancel);
            }
            if ui
                .button(lang.pick("Reset to original", "원본으로 되돌리기"))
                .clicked()
            {
                actions.push(ReplayAction::Reset);
            }
            ui.checkbox(
                &mut self.compare,
                lang.pick("Compare with original", "원본과 비교"),
            );
        });
        if let Some(error) = &self.error {
            ui.colored_label(
                crate::ui::style::theme::Palette::of(ui).error,
                error.localized(lang),
            );
        }
    }

    pub(crate) fn apply(&mut self, action: ReplayAction, proxy: Option<&ProxyHandle>) {
        match action {
            ReplayAction::LoadBody => match self.draft.load_body_file(Path::new(&self.file)) {
                Ok(()) => {
                    self.body = BodyView::new(&self.draft.body);
                    self.body_source = Some(self.file.clone());
                    self.error = None;
                }
                Err(error) => self.error = Some(ReplayPanelError::BodyFile(error)),
            },
            ReplayAction::Send => {
                if let Some(proxy) = proxy {
                    match proxy.replay(&self.draft) {
                        Ok(task) => {
                            self.task = Some(task);
                            self.error = None;
                            self.confirmed = false;
                        }
                        Err(error) => self.error = Some(ReplayPanelError::Replay(error)),
                    }
                }
            }
            ReplayAction::Cancel => {
                if let Some(task) = &self.task {
                    task.cancel();
                }
            }
            ReplayAction::Reset => match ReplayDraft::new(self.draft.original.clone()) {
                Ok(draft) => {
                    self.body = BodyView::new(&draft.body);
                    self.body_source = None;
                    self.draft = draft;
                }
                Err(error) => self.error = Some(ReplayPanelError::Replay(error)),
            },
            ReplayAction::Close => {}
        }
    }

    fn comparison_ui(&self, ui: &mut egui::Ui, latest: Option<&Exchange>, lang: Language) {
        if self.compare {
            let original = &self.draft.original;
            if let Some(result) = latest {
                ui.columns(2, |columns| {
                    for (column, record) in columns.iter_mut().zip([original, result]) {
                        column.strong(format!(
                            "#{} · {:.1} ms",
                            record.id().value(),
                            record.elapsed().as_secs_f64() * 1000.0
                        ));
                        let original_text = summary(
                            if record.id() == original.id() {
                                result
                            } else {
                                original
                            },
                            lang,
                        );
                        let text = summary(record, lang);
                        for (index, line) in text.lines().enumerate() {
                            let changed = original_text.lines().nth(index) != Some(line);
                            column.colored_label(
                                if changed {
                                    crate::ui::style::theme::Palette::of(column).warning
                                } else {
                                    Palette::of(column).text
                                },
                                line,
                            );
                        }
                    }
                });
            } else {
                ui.label(lang.pick(
                    "Send a request to compare its captured result.",
                    "요청을 전송하면 캡처된 결과를 원본과 비교합니다.",
                ));
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

fn pair_input_widths(ui: &egui::Ui) -> (f32, f32) {
    let controls_width = ui
        .spacing()
        .item_spacing
        .x
        .mul_add(3.0, Layout::REPLAY_PAIR_CONTROL_WIDTH);
    let inputs = (ui.available_width() - controls_width).max(Layout::REPLAY_PAIR_MIN_WIDTH);
    let name_width = Layout::REPLAY_PAIR_LABEL_WIDTH.fit(inputs * 0.36);
    let value_width = (inputs - name_width).max(Layout::REPLAY_PAIR_MIN_VALUE);
    (name_width, value_width)
}

fn fields(ui: &mut egui::Ui, fields: &mut Vec<ReplayField>, lang: Language) {
    let (name_width, value_width) = pair_input_widths(ui);
    let mut remove = None;
    for (index, field) in fields.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut field.enabled, "");
                ui.add(egui::TextEdit::singleline(&mut field.name).desired_width(name_width));
                ui.add(
                    egui::TextEdit::singleline(&mut field.value)
                        .desired_width(value_width)
                        .password(matches!(
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
    if ui.button(lang.pick("Add header", "헤더 추가")).clicked() {
        fields.push(ReplayField {
            enabled: true,
            name: String::new(),
            value: String::new(),
        });
    }
}

fn summary(record: &Exchange, lang: Language) -> String {
    let mut text = format!(
        "{} {}\n",
        record.request().method(),
        record.request().target()
    );
    for field in record.request().headers().entries() {
        let _ = writeln!(text, "{}: {}", field.name(), field.value());
    }
    if let Some(body) = record.request().body() {
        append_body_preview(&mut text, body, lang);
        text.push('\n');
    }
    if let http_inspector_exchange::Outcome::Response(response) = record.outcome() {
        let status_label = lang.pick("Status", "상태");
        let _ = writeln!(text, "{status_label} {}", response.status());
        for field in response.headers().entries() {
            let _ = writeln!(text, "{}: {}", field.name(), field.value());
        }
        append_body_preview(&mut text, response.body(), lang);
    } else {
        let _ = write!(
            text,
            "{}",
            crate::ui::requests::outcome_view::outcome_text(record.outcome(), lang).0
        );
    }
    text
}

fn append_body_preview(
    text: &mut String,
    preview: &http_inspector_exchange::BodyPreview,
    lang: Language,
) {
    if let Some(body) = preview.snapshot() {
        text.push_str(&String::from_utf8_lossy(body.bytes()));
        if !body.is_complete() {
            text.push_str(lang.pick("\n[Partial body preview]", "\n[부분 본문 미리보기]"));
        }
    } else {
        text.push_str(lang.pick("[Preview is updating]", "[미리보기 업데이트 중]"));
    }
}

/// Cached outcome of validating a JSON request body.
enum JsonCheck {
    Valid,
    Invalid(String),
}

/// Views derived from the draft body, rebuilt only when the body changes.
struct BodyView {
    /// Editable text, present for UTF-8 bodies small enough to lay out every frame.
    text: Option<String>,
    /// Cached JSON check; `None` until a JSON content type first asks for it.
    json: Option<JsonCheck>,
}

impl BodyView {
    fn new(body: &[u8]) -> Self {
        let text = if body.len() <= EDITABLE_BODY_BYTES {
            std::str::from_utf8(body).ok().map(str::to_owned)
        } else {
            None
        };
        Self { text, json: None }
    }

    /// The JSON syntax error for `body`, parsing only on the first request after a change.
    fn json_error(&mut self, body: &[u8]) -> Option<&str> {
        let check = self.json.get_or_insert_with(|| {
            match serde_json::from_slice::<serde_json::Value>(body) {
                Ok(_) => JsonCheck::Valid,
                Err(error) => JsonCheck::Invalid(error.to_string()),
            }
        });
        match check {
            JsonCheck::Valid => None,
            JsonCheck::Invalid(message) => Some(message.as_str()),
        }
    }
}

/// Preserves the complete target until a row changes and keeps the fragment for replay validation.
fn edit_query(url: &mut String, edit: impl FnOnce(&mut String) -> bool) {
    let parts = urlencoded::split_target(url);
    let mut query = parts.query.unwrap_or_default().to_owned();
    if edit(&mut query) {
        let delimiter = if query.is_empty() { "" } else { "?" };
        *url = format!("{}{delimiter}{query}{}", parts.base, parts.fragment);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PairSyntax {
    Cookie,
    UrlEncoded,
}

impl PairSyntax {
    const fn joiner(self) -> &'static str {
        match self {
            Self::Cookie => "; ",
            Self::UrlEncoded => "&",
        }
    }

    fn rows(self, text: &str) -> Vec<PairRow> {
        match self {
            Self::Cookie => text
                .split(';')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(RawPair::parse)
                .map(PairRow::from)
                .collect(),
            Self::UrlEncoded => urlencoded::raw_pairs(text).map(PairRow::from).collect(),
        }
    }
}

/// One row of a pair table. `raw` keeps the original text until the row is edited.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PairRow {
    enabled: bool,
    name: String,
    value: String,
    raw: Option<String>,
}

/// Rows cached across frames so disabled rows and untouched text survive.
#[derive(Default)]
struct PairTable {
    source: Option<String>,
    rows: Vec<PairRow>,
}

impl PairTable {
    /// Re-parses only when `text` changed outside the table.
    fn sync(&mut self, text: &str, syntax: PairSyntax) {
        if self.source.as_deref() != Some(text) {
            self.rows = syntax.rows(text);
            self.source = Some(text.to_owned());
        }
    }

    /// Writes the rows back to `text`; call only after an edit.
    fn apply(&mut self, text: &mut String, syntax: PairSyntax) {
        *text = join_pairs(&self.rows, syntax.joiner());
        self.source = Some(text.clone());
    }

    /// Shows the table and returns whether `text` was rewritten.
    fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &mut String,
        syntax: PairSyntax,
        add: &str,
    ) -> bool {
        self.sync(text, syntax);
        let changed = pair_rows(ui, &mut self.rows, syntax == PairSyntax::Cookie, add);
        if changed {
            self.apply(text, syntax);
        }
        changed
    }
}

fn pair_rows(ui: &mut egui::Ui, rows: &mut Vec<PairRow>, mask: bool, add: &str) -> bool {
    let (name_width, value_width) = pair_input_widths(ui);
    let mut changed = false;
    let mut remove = None;
    for (index, row) in rows.iter_mut().enumerate() {
        ui.push_id(index, |ui| {
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut row.enabled, "").changed();
                let mut edited = ui
                    .add(egui::TextEdit::singleline(&mut row.name).desired_width(name_width))
                    .changed();
                edited |= ui
                    .add(
                        egui::TextEdit::singleline(&mut row.value)
                            .desired_width(value_width)
                            .password(mask),
                    )
                    .changed();
                if edited {
                    row.raw = None;
                    changed = true;
                }
                if ui.button("−").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        rows.remove(index);
        changed = true;
    }
    if ui.button(add).clicked() {
        rows.push(PairRow {
            enabled: true,
            name: String::new(),
            value: String::new(),
            raw: None,
        });
    }
    changed
}

impl From<RawPair<'_>> for PairRow {
    fn from(pair: RawPair<'_>) -> Self {
        Self {
            enabled: true,
            name: pair.name.to_owned(),
            value: pair.value.unwrap_or_default().to_owned(),
            raw: Some(pair.raw.to_owned()),
        }
    }
}

fn join_pairs(rows: &[PairRow], joiner: &str) -> String {
    rows.iter()
        .filter(|row| row.enabled)
        .filter_map(|row| match &row.raw {
            Some(raw) => Some(raw.clone()),
            None if row.name.is_empty() && row.value.is_empty() => None,
            None => Some(format!("{}={}", row.name, row.value)),
        })
        .collect::<Vec<_>>()
        .join(joiner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(table: &mut PairTable, index: usize, change: impl FnOnce(&mut PairRow)) {
        let row = table.rows.get_mut(index);
        assert!(row.is_some(), "missing row {index}");
        if let Some(row) = row {
            change(row);
        }
    }

    #[test]
    fn large_bodies_are_summarized_instead_of_edited() {
        assert_eq!(BodyView::new(b"a=1").text.as_deref(), Some("a=1"));
        assert!(
            BodyView::new(&vec![b'a'; EDITABLE_BODY_BYTES + 1])
                .text
                .is_none()
        );
        assert!(BodyView::new(&[0xff]).text.is_none());
    }

    #[test]
    fn json_check_is_cached_until_the_body_changes() {
        let mut view = BodyView::new(b"{");
        assert!(view.json_error(b"{").is_some());
        // A cached result is reused even when given other bytes; callers rebuild on change.
        assert!(view.json_error(b"{}").is_some());
        assert!(BodyView::new(b"{}").json_error(b"{}").is_none());
    }

    fn joined(text: &str, syntax: PairSyntax) -> String {
        join_pairs(&syntax.rows(text), syntax.joiner())
    }

    #[test]
    fn preserves_encoded_query_and_cookie_values() {
        assert_eq!(
            joined("a=x%26y&a=two%20words", PairSyntax::UrlEncoded),
            "a=x%26y&a=two%20words"
        );
        assert_eq!(
            joined("session=a=b; theme=dark", PairSyntax::Cookie),
            "session=a=b; theme=dark"
        );
    }

    #[test]
    fn opening_a_table_does_not_change_the_text() {
        for (text, syntax) in [
            ("flag&a=1&&b=2", PairSyntax::UrlEncoded),
            ("a=1;flag; b=2 ", PairSyntax::Cookie),
        ] {
            let mut table = PairTable::default();
            table.sync(text, syntax);
            table.sync(text, syntax);
            assert_eq!(table.source.as_deref(), Some(text));
            assert_eq!(table.rows.len(), 3);
        }
    }

    #[test]
    fn editing_query_rows_preserves_fragments_for_request_validation()
    -> Result<(), Box<dyn std::error::Error>> {
        use http_inspector_core::replay::ReplayUrlError;
        use http_inspector_exchange::{BodyLifecycle, ExchangeBuilder};

        let exchange = ExchangeBuilder::http(http::Method::GET, "http://example.test/p")?
            .request_body(Vec::new(), BodyLifecycle::Complete)
            .in_flight()?;
        let mut draft = ReplayDraft::new(exchange)?;
        for (original, actual_query) in [
            ("http://example.test/p#section?x=1", ""),
            (
                "http://example.test/p?flag&&empty=#section?x=1",
                "flag&&empty=",
            ),
        ] {
            draft.url = original.to_owned();
            edit_query(&mut draft.url, |query| {
                assert_eq!(query, actual_query);
                false
            });
            assert_eq!(draft.url, original);
            edit_query(&mut draft.url, |query| {
                "a=b".clone_into(query);
                true
            });
            assert_eq!(draft.url, "http://example.test/p?a=b#section?x=1");
            assert!(matches!(
                draft.request("127.0.0.1:8080".parse()?),
                Err(ReplayError::Url(ReplayUrlError::CredentialsOrFragment))
            ));
            edit_query(&mut draft.url, |query| {
                query.clear();
                true
            });
            assert_eq!(draft.url, "http://example.test/p#section?x=1");
        }
        Ok(())
    }

    #[test]
    fn disabled_rows_can_be_enabled_again() {
        let mut text = "a=1; b=2".to_owned();
        let mut table = PairTable::default();
        table.sync(&text, PairSyntax::Cookie);
        edit(&mut table, 0, |row| row.enabled = false);
        table.apply(&mut text, PairSyntax::Cookie);
        assert_eq!(text, "b=2");
        table.sync(&text, PairSyntax::Cookie);
        assert_eq!(table.rows.len(), 2);
        edit(&mut table, 0, |row| row.enabled = true);
        table.apply(&mut text, PairSyntax::Cookie);
        assert_eq!(text, "a=1; b=2");
    }

    #[test]
    fn editing_one_row_keeps_other_rows_verbatim() {
        let mut text = "flag&a=1".to_owned();
        let mut table = PairTable::default();
        table.sync(&text, PairSyntax::UrlEncoded);
        edit(&mut table, 1, |row| {
            row.value = "2".to_owned();
            row.raw = None;
        });
        table.apply(&mut text, PairSyntax::UrlEncoded);
        assert_eq!(text, "flag&a=2");
    }

    #[test]
    fn empty_added_rows_are_not_sent() {
        let mut text = "a=1".to_owned();
        let mut table = PairTable::default();
        table.sync(&text, PairSyntax::Cookie);
        table.rows.push(PairRow {
            enabled: true,
            name: String::new(),
            value: String::new(),
            raw: None,
        });
        table.apply(&mut text, PairSyntax::Cookie);
        assert_eq!(text, "a=1");
    }
}
