//! SolidCraft PDM: a desktop window onto a vault of SolidCraft designs.
//!
//! `solidcraft-pdm [folder]` opens the vault containing `folder`, else the last one used.

use scpdm_gui::app;

fn main() -> eframe::Result {
    let start = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("SolidCraft PDM")
            .with_app_id("solidcraft-pdm")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "SolidCraft PDM",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, start)))),
    )
}
