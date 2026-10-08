mod app;
mod audio;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let initial = std::env::args().nth(1);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Orpheus")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Orpheus",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::OrpheusGui::new(cc, initial)))),
    )
}
