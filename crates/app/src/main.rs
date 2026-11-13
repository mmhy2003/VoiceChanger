#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! VoiceChanger desktop app.

mod app;
mod config;
mod engine;
mod model;
mod paths;
mod status_bar;
mod tabs;
#[cfg(test)]
mod testutil;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .init();
    let dirs = paths::AppDirs::system();
    let (config, note) = config::load(&dirs.settings_file());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::window_title())
            .with_inner_size([720.0, 520.0])
            .with_min_inner_size([520.0, 400.0]),
        ..Default::default()
    };
    let startup = app::Startup {
        dirs,
        config,
        notes: note.into_iter().collect(),
    };
    eframe::run_native(
        &app::window_title(),
        options,
        Box::new(|cc| Ok(Box::new(app::VcApp::new(cc, startup)))),
    )
}
