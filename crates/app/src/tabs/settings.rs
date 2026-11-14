//! Settings tab: audio devices, monitor, Refresh, warnings, Performance and General.

use eframe::egui;
use vc_audio::{feedback_warnings, is_virtual_cable, start_blocker};

use crate::app::{ProviderChoice, VcApp};
use crate::files;
use crate::hotkey::{RecordOutcome, record_key};
use crate::paths::AppDirs;
use crate::update;
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
            ui.label("Microphone");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("input")
                    .selected_text(name_or_none(&sel.input.name))
                    .show_ui(ui, |ui| {
                        for d in &devices.inputs {
                            if ui
                                .selectable_label(sel.input.same(&d.into()), &d.name)
                                .clicked()
                            {
                                sel.input = d.into();
                            }
                        }
                    });
                if let Some(level) = input_level {
                    widgets::meter(ui, "Level", level);
                }
            });
            ui.end_row();

            ui.label("Virtual mic");
            egui::ComboBox::from_id_salt("cable")
                .selected_text(output_label(&sel.cable.name))
                .show_ui(ui, |ui| {
                    for d in &devices.outputs {
                        let label = output_label(&d.name);
                        if ui
                            .selectable_label(sel.cable.same(&d.into()), label)
                            .clicked()
                        {
                            sel.cable = d.into();
                        }
                    }
                });
            ui.end_row();

            ui.label("Monitor");
            ui.vertical(|ui| {
                ui.checkbox(&mut sel.monitor_enabled, "Monitor (hear yourself)");
                ui.add_enabled_ui(sel.monitor_enabled, |ui| {
                    egui::ComboBox::from_id_salt("monitor")
                        .selected_text(name_or_none(
                            sel.monitor.as_ref().map_or("", |m| m.name.as_str()),
                        ))
                        .show_ui(ui, |ui| {
                            for d in &devices.outputs {
                                let selected =
                                    sel.monitor.as_ref().is_some_and(|m| m.same(&d.into()));
                                if ui.selectable_label(selected, &d.name).clicked() {
                                    sel.monitor = Some(d.into());
                                }
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
    general(ui, app);
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

/// Settings → General: the Start/Stop hotkey, Start minimised, updates, the logs folder and About.
fn general(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.separator();
    ui.heading("General");

    ui.horizontal(|ui| {
        ui.label("Hotkey:");
        let current = app.prefs.hotkey.as_deref().unwrap_or("Off");
        // A recording error is not about the saved hotkey, which stays active.
        if app.hotkey_error.is_some() && app.prefs.hotkey.is_some() && !app.recording_hotkey {
            ui.label(format!("{current} (not active)"));
        } else {
            ui.label(current);
        }
        if ui
            .add_enabled(!app.recording_hotkey, egui::Button::new("Set…"))
            .clicked()
        {
            app.recording_hotkey = true;
        }
        if ui.button("Clear").clicked() {
            // Clearing never fails.
            let _ = app.hotkeys.set(None);
            app.prefs.hotkey = None;
            app.hotkey_error = None;
            app.recording_hotkey = false;
        }
    });
    if app.recording_hotkey {
        ui.label("Press a key combination (Esc cancels)…");
        let pressed = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        if let Some((key, mods)) = pressed {
            record(app, key, mods);
        }
    }
    if let Some(error) = &app.hotkey_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    }

    ui.checkbox(&mut app.prefs.start_minimised, "Start minimised");
    update::ui::settings_rows(ui, app);
    ui.horizontal(|ui| {
        if ui.button("Open logs folder").clicked() {
            files::open_folder(&app.dirs.logs);
        }
        if ui.button("About…").clicked() {
            app.show_about = true;
        }
    });

    let lines = about_lines(&app.dirs);
    egui::Window::new("About VoiceChanger")
        .open(&mut app.show_about)
        .collapsible(false)
        .show(ui.ctx(), |ui| {
            ui.heading("VoiceChanger");
            egui::Grid::new("about")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (label, value) in &lines {
                        ui.label(*label);
                        if *label == "Repository" {
                            ui.hyperlink(value);
                        } else {
                            ui.label(value);
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Applies a key press made while recording the hotkey.
fn record(app: &mut VcApp, key: egui::Key, mods: egui::Modifiers) {
    match record_key(key, mods) {
        RecordOutcome::Cancel => app.recording_hotkey = false,
        RecordOutcome::Invalid(msg) => app.hotkey_error = Some(msg),
        RecordOutcome::Combo(combo) => {
            match app.hotkeys.set(Some(combo)) {
                Ok(()) => {
                    app.prefs.hotkey = Some(combo.to_string());
                    app.hotkey_error = None;
                }
                Err(msg) => {
                    app.hotkey_error = Some(msg);
                    app.prefs.hotkey = None;
                }
            }
            app.recording_hotkey = false;
        }
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

/// The About box rows: version, licence, repository and the app's folders.
pub fn about_lines(dirs: &AppDirs) -> Vec<(&'static str, String)> {
    let folder = |p: &std::path::Path| p.display().to_string();
    vec![
        ("Version", env!("CARGO_PKG_VERSION").to_string()),
        ("Licence", env!("CARGO_PKG_LICENSE").to_string()),
        ("Repository", env!("CARGO_PKG_REPOSITORY").to_string()),
        ("Models folder", folder(&dirs.models)),
        ("Settings folder", folder(&dirs.config)),
        ("Logs folder", folder(&dirs.logs)),
    ]
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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

    #[test]
    fn about_lines_has_licence_and_folders() {
        let dirs = AppDirs::under(Path::new("C:/r"), Path::new("C:/l"));
        let folder = |p: &Path| p.display().to_string();
        assert_eq!(
            about_lines(&dirs),
            vec![
                ("Version", env!("CARGO_PKG_VERSION").to_string()),
                ("Licence", "MIT".to_string()),
                (
                    "Repository",
                    "https://github.com/mmhy2003/VoiceChanger".to_string()
                ),
                ("Models folder", folder(&dirs.models)),
                ("Settings folder", folder(&dirs.config)),
                ("Logs folder", folder(&dirs.logs)),
            ]
        );
    }
}
