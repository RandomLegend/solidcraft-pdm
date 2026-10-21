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
        // With vsync, presenting a frame on Wayland waits for the compositor's frame callback.
        // Hyprland sends none while it has nothing to redraw, so a repaint while the window is
        // unfocused could block the event loop, the ping went unanswered and Hyprland reported
        // the app as not responding. egui only repaints on demand, so vsync buys nothing here.
        vsync: false,
        ..Default::default()
    };
    eframe::run_native(
        "SolidCraft PDM",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, start)))),
    )
}
