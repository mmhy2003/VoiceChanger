//! Settings tab: audio devices, monitor, Refresh, warnings and Performance.

use eframe::egui;
use vc_audio::{feedback_warnings, is_virtual_cable, start_blocker};

use crate::app::{ProviderChoice, VcApp};
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

    performance(ui, app);
}

/// Settings → Performance: provider, GPU and the block, context and crossfade sliders.
fn performance(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.separator();
    ui.heading("Performance");

    let running = app.is_running();
    let loading = app.model.is_loading();
    let perf = &mut app.state.perf;
    let mut restart = false;
    egui::Grid::new("performance")
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            ui.label("Provider");
            ui.add_enabled_ui(!running && !loading, |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("provider")
                        .selected_text(choice_label(perf.provider))
                        .show_ui(ui, |ui| {
                            for choice in [
                                ProviderChoice::DirectMl,
                                ProviderChoice::Cuda,
                                ProviderChoice::Cpu,
                            ] {
                                ui.selectable_value(
                                    &mut perf.provider,
                                    choice,
                                    choice_label(choice),
                                );
                            }
                        });
                    if perf.provider != ProviderChoice::Cpu {
                        ui.label("GPU");
                        ui.add(egui::DragValue::new(&mut perf.gpu).range(0..=7));
                    }
                });
            });
            ui.end_row();

            let stream = &mut perf.stream;
            for (label, value, range) in [
                ("Block", &mut stream.block_ms, 50..=1000),
                ("Extra context", &mut stream.context_ms, 0..=2000),
                ("Crossfade", &mut stream.crossfade_ms, 10..=200),
            ] {
                ui.label(label);
                let resp = ui.add(egui::Slider::new(value, range).step_by(10.0).suffix(" ms"));
                restart |=
                    needs_restart(running, resp.drag_stopped(), resp.changed(), resp.dragged());
                ui.end_row();
            }
        });
    if restart {
        app.restart_requested = true;
    }

    if running && let Some(warning) = inference_warning(app.stats.ratio()) {
        ui.colored_label(ui.visuals().warn_fg_color, warning);
    }
}

/// True when a running session must restart after this slider interaction.
pub fn needs_restart(running: bool, drag_stopped: bool, changed: bool, dragged: bool) -> bool {
    running && (drag_stopped || (changed && !dragged))
}

/// Shown while running when inference takes more than 60% of each block.
pub fn inference_warning(ratio: f32) -> Option<String> {
    (ratio > 0.6).then(|| {
        let percent = (ratio * 100.0).round();
        format!(
            "⚠ Inference takes {percent}% of each block. Raise the block size or pick a \
             faster provider to avoid dropouts."
        )
    })
}

fn choice_label(choice: ProviderChoice) -> &'static str {
    match choice {
        ProviderChoice::DirectMl => "DirectML",
        ProviderChoice::Cuda => "CUDA",
        ProviderChoice::Cpu => "CPU",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perf_change_needs_restart() {
        // (running, drag_stopped, changed, dragged) -> restart
        assert!(!needs_restart(false, true, true, false));
        assert!(!needs_restart(false, false, true, false));
        assert!(!needs_restart(true, false, true, true));
        assert!(!needs_restart(true, false, false, true));
        assert!(!needs_restart(true, false, false, false));
        assert!(needs_restart(true, true, false, false));
        assert!(needs_restart(true, true, true, false));
        assert!(needs_restart(true, false, true, false));
    }

    #[test]
    fn inference_warning_text() {
        let w = inference_warning(0.65).expect("warning above 60%");
        assert!(w.contains("65%"), "{w}");
        assert_eq!(
            w,
            "⚠ Inference takes 65% of each block. Raise the block size or pick a faster \
             provider to avoid dropouts."
        );
        assert_eq!(inference_warning(0.6), None);
        assert_eq!(inference_warning(0.0), None);
    }
}
