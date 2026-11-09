#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod editor;
mod recording;
mod scaling;
mod sources;
mod theme;
mod tiles;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Sensor Plotter")
            .with_inner_size([1400.0, 900.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Sensor Plotter",
        options,
        Box::new(|cc| Ok(Box::new(app::SensorPlotterApp::new(cc)))),
    )
}
