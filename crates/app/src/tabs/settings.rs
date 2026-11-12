//! Settings tab: audio devices, monitor, Refresh and warnings.

use eframe::egui;
use vc_audio::{feedback_warnings, is_virtual_cable, start_blocker};

use crate::app::VcApp;
use crate::widgets;

const VB_CABLE_URL: &str = "https://vb-audio.com/Cable/";

pub fn show(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.heading("Audio devices");

    let input_level = app.is_running().then_some(app.input_meter.shown);
    let state = &mut app.state;
    let devices = &state.devices;
    let sel = &mut state.selection;
    egui::Grid::new("devices")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Input");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("input")
                    .selected_text(name_or_none(&sel.input))
                    .show_ui(ui, |ui| {
                        for name in &devices.inputs {
                            ui.selectable_value(&mut sel.input, name.clone(), name);
                        }
                    });
                if let Some(level) = input_level {
                    widgets::meter(ui, "Level", level);
                }
            });
            ui.end_row();

            ui.label("Cable output");
            egui::ComboBox::from_id_salt("cable")
                .selected_text(output_label(&sel.cable))
                .show_ui(ui, |ui| {
                    for name in &devices.outputs {
                        ui.selectable_value(&mut sel.cable, name.clone(), output_label(name));
                    }
                });
            ui.end_row();

            ui.label("Monitor");
            ui.vertical(|ui| {
                ui.checkbox(&mut sel.monitor_enabled, "Monitor (hear yourself)");
                ui.add_enabled_ui(sel.monitor_enabled, |ui| {
                    egui::ComboBox::from_id_salt("monitor")
                        .selected_text(name_or_none(sel.monitor.as_deref().unwrap_or_default()))
                        .show_ui(ui, |ui| {
                            for name in &devices.outputs {
                                ui.selectable_value(&mut sel.monitor, Some(name.clone()), name);
                            }
                        });
                    ui.add(
                        egui::Slider::new(&mut state.monitor_db, -40.0..=0.0)
                            .suffix(" dB")
                            .text("Volume"),
                    );
                });
            });
            ui.end_row();
        });

    if ui.button("Refresh").clicked() {
        app.refresh_devices();
    }

    let state = &app.state;
    let warn = ui.visuals().warn_fg_color;
    if !state.devices.has_cable() {
        ui.colored_label(
            warn,
            "No virtual audio cable found. Install VB-CABLE so other apps can use VoiceChanger \
             as a microphone. You can still pick any output to test.",
        );
        if ui.link("Download VB-CABLE (free)").clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(VB_CABLE_URL));
        }
    }
    for warning in feedback_warnings(&state.selection) {
        ui.colored_label(warn, format!("⚠ {}", warning.message()));
    }
    for note in &state.notes {
        ui.label(note);
    }
    if let Some(blocker) = start_blocker(&state.devices, &state.selection) {
        ui.colored_label(ui.visuals().error_fg_color, blocker);
    }
}

fn name_or_none(name: &str) -> &str {
    if name.is_empty() { "(none)" } else { name }
}

/// Virtual cables are marked as the recommended output.
fn output_label(name: &str) -> String {
    if is_virtual_cable(name) {
        format!("★ {name} (recommended)")
    } else {
        name_or_none(name).to_string()
    }
}
