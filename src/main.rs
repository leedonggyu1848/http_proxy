#![forbid(unsafe_code)]

fn main() -> eframe::Result {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        http_proxy::APP_NAME,
        options,
        Box::new(|_| Ok(Box::<ScaffoldApp>::default())),
    )
}

#[derive(Default)]
struct ScaffoldApp;

impl eframe::App for ScaffoldApp {
    fn ui(&mut self, ui: &mut eframe::egui::Ui, _frame: &mut eframe::Frame) {
        ui.heading(http_proxy::APP_NAME);
        ui.label("Proxy implementation is not connected yet.");
    }
}
