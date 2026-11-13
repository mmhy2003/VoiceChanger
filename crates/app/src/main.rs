#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! VoiceChanger desktop app.

mod app;
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "wired into the app in the next step")
)]
mod config;
mod engine;
mod model;
#[expect(dead_code, reason = "wired into the app in the next step")]
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::window_title())
            .with_inner_size([720.0, 520.0])
            .with_min_inner_size([520.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native(
        &app::window_title(),
        options,
        Box::new(|cc| Ok(Box::new(app::VcApp::new(cc)))),
    )
}
