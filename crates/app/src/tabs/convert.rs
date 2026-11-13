//! Convert tab.

use eframe::egui;

use crate::app::VcApp;

pub fn show(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.heading("Convert");
    if let Some(path) = &app.convert_file {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        ui.label(format!("File: {name}"));
    }
    ui.label("Converting PyTorch models arrives in M5.");
}
