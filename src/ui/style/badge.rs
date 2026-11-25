//! Compact tone-colored labels: method badges and status pills.
//!
//! A badge always shows its meaning as text; the tone only reinforces it.

use eframe::egui::{self, FontFamily, FontId, Sense, TextWrapMode, Widget};

use super::tokens::{Radius, Space, Tone, TypeScale};

/// Height of every badge, small enough to sit inside a list row.
pub(crate) const BADGE_HEIGHT: f32 = 18.0;

/// Badge outline: a tag with small corners or a fully rounded pill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Shape {
    Tag,
    Pill,
}

/// A label on a tinted fill. Text that does not fit is truncated with `…`.
#[must_use = "Add the badge with `ui.add(badge)`"]
pub(crate) struct Badge {
    text: String,
    tone: Tone,
    shape: Shape,
    family: FontFamily,
    min_width: f32,
    sense: Sense,
}

impl Badge {
    /// An HTTP method badge such as `GET`, colored by [`method_tone`].
    pub(crate) fn method(method: &str) -> Self {
        Self {
            text: method.to_owned(),
            tone: method_tone(method),
            shape: Shape::Tag,
            family: FontFamily::Monospace,
            // Wide enough for `DELETE`, so common methods line up.
            min_width: 52.0,
            sense: Sense::hover(),
        }
    }

    /// A status pill such as `404 Not Found`.
    pub(crate) fn status(text: impl Into<String>, tone: Tone) -> Self {
        Self {
            text: text.into(),
            tone,
            shape: Shape::Pill,
            family: FontFamily::Proportional,
            min_width: 0.0,
            sense: Sense::hover(),
        }
    }

    /// Makes the badge respond to `sense`, for example clicks and focus.
    pub(crate) const fn sense(mut self, sense: Sense) -> Self {
        self.sense = sense;
        self
    }

    fn padding(&self) -> f32 {
        match self.shape {
            Shape::Tag => Space::XS + Space::XXS,
            Shape::Pill => Space::SM,
        }
    }
}

impl Widget for Badge {
    #[expect(clippy::disallowed_methods, reason = "badge font uses the type scale")]
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let colors = self.tone.colors(ui.ctx().theme());
        let padding = self.padding();
        let available = ui.available_width().max(0.0);
        let galley = egui::WidgetText::from(
            egui::RichText::new(&self.text)
                .font(FontId::new(TypeScale::SMALL, self.family.clone()))
                .color(colors.fg),
        )
        .into_galley(
            ui,
            Some(TextWrapMode::Truncate),
            2.0f32.mul_add(-padding, available).max(0.0),
            egui::TextStyle::Small,
        );
        let width = 2.0f32
            .mul_add(padding, galley.size().x)
            .max(self.min_width)
            .min(available);
        let (rect, response) = ui.allocate_exact_size(egui::vec2(width, BADGE_HEIGHT), self.sense);
        let role = if self.sense.senses_click() {
            egui::Role::Button
        } else {
            egui::Role::Label
        };
        response.widget_info(|| egui::WidgetInfo::labeled(role, ui.is_enabled(), &self.text));
        if ui.is_rect_visible(rect) {
            let radius = match self.shape {
                Shape::Tag => egui::CornerRadius::same(Radius::BADGE),
                Shape::Pill => egui::CornerRadius::same(u8::MAX),
            };
            let painter = ui.painter();
            painter.rect_filled(rect, radius, colors.fill);
            if response.has_focus() {
                painter.rect_stroke(
                    rect.expand(1.0),
                    radius,
                    ui.visuals().selection.stroke,
                    egui::StrokeKind::Outside,
                );
            }
            let text_pos = egui::pos2(
                rect.left() + padding,
                rect.center().y - galley.size().y / 2.0,
            );
            painter.galley(text_pos, galley, colors.fg);
        }
        response
    }
}

/// Tone of an HTTP method: reads in blue, writes in green or orange, and
/// deletes in red. Methods are case-sensitive, so only the standard
/// uppercase names get a color.
pub(crate) fn method_tone(method: &str) -> Tone {
    match method {
        "GET" => Tone::Info,
        "POST" => Tone::Success,
        "PUT" | "PATCH" => Tone::Warning,
        "DELETE" => Tone::Danger,
        _ => Tone::Neutral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_get_their_tone() {
        assert_eq!(method_tone("GET"), Tone::Info);
        assert_eq!(method_tone("POST"), Tone::Success);
        assert_eq!(method_tone("PUT"), Tone::Warning);
        assert_eq!(method_tone("PATCH"), Tone::Warning);
        assert_eq!(method_tone("DELETE"), Tone::Danger);
        assert_eq!(method_tone("OPTIONS"), Tone::Neutral);
        assert_eq!(method_tone("get"), Tone::Neutral);
    }

    #[test]
    fn badge_names_itself_and_truncates_to_the_space() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut width = 0.0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.allocate_ui(egui::vec2(40.0, 30.0), |ui| {
                width = ui
                    .add(Badge::status("500 Internal Server Error", Tone::Danger))
                    .rect
                    .width();
            });
        });
        output.textures_delta.clear();
        assert!(width <= 40.0, "{width}");
        let tree = output
            .platform_output
            .accesskit_update
            .ok_or("missing accessibility tree")?;
        // Screen readers get the full text even when the badge is truncated;
        // egui exposes a label's text as the node value.
        assert!(tree.nodes.iter().any(|(_, node)| {
            node.role() == egui::Role::Label && node.value() == Some("500 Internal Server Error")
        }));
        Ok(())
    }
}
