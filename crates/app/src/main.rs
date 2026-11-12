#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! VoiceChanger desktop app.

mod app;
mod tabs;
mod widgets;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::window_title())
            .with_inner_size([720.0, 520.0])
            .with_min_inner_size([520.0, 400.0])
            // winit's OLE drag-and-drop puts the UI thread in STA, which breaks cpal's WASAPI COM init.
            .with_drag_and_drop(false),
        ..Default::default()
    };
    eframe::run_native(
        &app::window_title(),
        options,
        Box::new(|cc| Ok(Box::new(app::VcApp::new(cc)))),
    )
}
