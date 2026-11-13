//! Models tab: import, the sample-rate prompt, notices and the library list.

use eframe::egui;

use crate::app::{Tab, VcApp};
use crate::files;
use crate::import::{Notice, offer_convert_text};
use crate::tabs::voice::khz;

/// The sample rates RVC v2 models use.
const RATES: [u32; 3] = [32000, 40000, 48000];

pub fn show(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.heading("Models");

    ui.horizontal(|ui| {
        if ui
            .add_enabled(app.import_pick_rx.is_none(), egui::Button::new("Import…"))
            .clicked()
        {
            app.pick_import();
        }
        if ui.button("Open folder").clicked() {
            files::open_folder(app.library.root());
        }
    });

    if let Some(src) = app.importer.needs_rate() {
        let file = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut choice = None;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(format!("\"{file}\" does not say its sample rate. Pick it:"));
            ui.horizontal(|ui| {
                for sr in RATES {
                    if ui.button(khz(sr)).clicked() {
                        choice = Some(sr);
                    }
                }
                cancel = ui.button("Cancel").clicked();
            });
        });
        if let Some(sr) = choice {
            app.importer.choose_rate(sr, &app.library);
        } else if cancel {
            app.importer.cancel_rate();
        }
    }

    notices(ui, app);

    ui.separator();
    if app.library.entries().is_empty() {
        ui.label("No models yet. Import an RVC v2 .onnx file or drop one on the window.");
    }
    let mut picked = None;
    for entry in app.library.entries() {
        ui.horizontal(|ui| {
            let is_active = app.prefs.last_model.as_deref() == Some(entry.id.as_str());
            if ui.selectable_label(is_active, &entry.name).clicked() {
                picked = Some(entry.id.clone());
            }
            ui.label(khz(entry.sample_rate));
            if !app.library.file_exists(entry) {
                ui.colored_label(ui.visuals().error_fg_color, "file missing");
            }
        });
    }
    if let Some(id) = picked {
        app.select_model(&id);
    }
}

/// Shows the notices, each with its buttons, and applies the one clicked.
fn notices(ui: &mut egui::Ui, app: &mut VcApp) {
    let mut dismiss = None;
    let mut convert = None;
    for (i, notice) in app.notices.iter().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| match notice {
                Notice::Error(msg) => {
                    ui.colored_label(ui.visuals().error_fg_color, msg);
                    if ui.button("Dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
                Notice::Info(msg) => {
                    ui.label(msg);
                    if ui.button("Dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
                Notice::OfferConvert(path) => {
                    ui.label(offer_convert_text(path));
                    if ui.button("Open in Convert").clicked() {
                        convert = Some((i, path.clone()));
                    }
                    if ui.button("Dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
            });
        });
    }
    if let Some((i, path)) = convert {
        app.notices.remove(i);
        app.convert_file = Some(path);
        app.tab = Tab::Convert;
    } else if let Some(i) = dismiss {
        app.notices.remove(i);
    }
}
