//! Voice tab: the Model list, Start/Stop, live voice controls, level meters, status and the banner.

use eframe::egui;
use vc_audio::{SessionInfo, start_blocker};

use crate::app::{AUDIO_DEAD, Tab, VcApp};
use crate::widgets;

pub fn show(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.heading("Voice");

    if let Some(banner) = &app.banner {
        let mut dismiss = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.colored_label(ui.visuals().error_fg_color, banner);
            dismiss = ui.button("Dismiss").clicked();
        });
        if dismiss {
            app.banner = None;
        }
    }

    model_picker(ui, app);

    if app.is_running() {
        if ui.button("■ Stop").clicked() {
            app.stop();
        }
    } else {
        let hint = start_hint(
            app.audio.is_alive(),
            start_blocker(&app.state.devices, &app.state.selection),
            app.model.is_ready(),
        );
        if ui
            .add_enabled(hint.is_none(), egui::Button::new("▶ Start"))
            .clicked()
        {
            app.start();
        }
        if let Some(hint) = hint {
            ui.label(hint);
        }
    }

    let pitch = ui.add(
        egui::Slider::new(&mut app.state.voice.pitch, -24..=24)
            .text("Pitch")
            .suffix(" st"),
    );
    if pitch.changed() {
        app.pitch_changed();
    }
    let voice = &mut app.state.voice;
    ui.add(
        egui::Slider::new(&mut voice.input_db, -20.0..=20.0)
            .text("Input gain")
            .suffix(" dB"),
    );
    ui.add(
        egui::Slider::new(&mut voice.output_db, -20.0..=20.0)
            .text("Output gain")
            .suffix(" dB"),
    );
    ui.add(
        egui::Slider::new(&mut voice.gate_db, -90.0..=-20.0)
            .text("Noise gate")
            .suffix(" dB"),
    );

    widgets::meter(ui, "Input", app.input_meter.shown);
    widgets::meter(ui, "Output", app.output_meter.shown);
    ui.label(status_line(app.session_info));
}

/// The Model list of library entries whose file exists, the model's state and a pending switch.
/// Picking an entry switches to it, also while running.
fn model_picker(ui: &mut egui::Ui, app: &mut VcApp) {
    if app.library.entries().is_empty() {
        ui.horizontal(|ui| {
            ui.label("No models yet.");
            if ui.button("Open Models").clicked() {
                app.tab = Tab::Models;
            }
        });
        return;
    }
    let active = app.prefs.last_model.clone();
    let selected = active
        .as_deref()
        .and_then(|id| app.library.get(id))
        .map_or_else(|| "Choose a model".to_string(), |e| e.name.clone());
    let choices: Vec<(String, String)> = app
        .library
        .entries()
        .iter()
        .filter(|e| app.library.file_exists(e))
        .map(|e| (e.id.clone(), e.name.clone()))
        .collect();
    let mut picked = None;
    ui.horizontal(|ui| {
        egui::ComboBox::from_label("Model")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (id, name) in &choices {
                    let is_active = active.as_deref() == Some(id.as_str());
                    if ui.selectable_label(is_active, name).clicked() {
                        picked = Some(id.clone());
                    }
                }
            });
        ui.label(app.model.label());
    });
    if let Some(pending) = &app.pending {
        ui.label(format!("Switching to {}…", pending.name));
    }
    if let Some(id) = picked {
        app.select_model(&id);
    }
}

/// Why Start is disabled, or `None` when it can be pressed: a dead audio thread first,
/// then the device `blocker`, then a missing model.
pub fn start_hint(
    audio_alive: bool,
    blocker: Option<&'static str>,
    model_ready: bool,
) -> Option<&'static str> {
    if !audio_alive {
        Some(AUDIO_DEAD)
    } else if blocker.is_some() {
        blocker
    } else if !model_ready {
        Some("Pick a model first")
    } else {
        None
    }
}

/// "Stopped", or the running session's sample rates.
pub fn status_line(info: Option<SessionInfo>) -> String {
    let Some(info) = info else {
        return "Stopped".to_string();
    };
    let monitor = info.monitor_rate.map_or_else(|| "off".to_string(), khz);
    format!(
        "Running · input {} · cable {} · monitor {monitor}",
        khz(info.input_rate),
        khz(info.cable_rate)
    )
}

/// 48000 -> "48 kHz", 44100 -> "44.1 kHz".
pub(crate) fn khz(rate: u32) -> String {
    let text = format!("{:.1}", f64::from(rate) / 1000.0);
    format!("{} kHz", text.strip_suffix(".0").unwrap_or(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_hint_order() {
        const BLOCKER: &str = "Pick a cable output";
        assert_eq!(start_hint(false, Some(BLOCKER), false), Some(AUDIO_DEAD));
        assert_eq!(start_hint(false, None, true), Some(AUDIO_DEAD));
        assert_eq!(start_hint(true, Some(BLOCKER), false), Some(BLOCKER));
        assert_eq!(start_hint(true, Some(BLOCKER), true), Some(BLOCKER));
        assert_eq!(start_hint(true, None, false), Some("Pick a model first"));
        assert_eq!(start_hint(true, None, true), None);
    }

    #[test]
    fn status_line_stopped() {
        assert_eq!(status_line(None), "Stopped");
    }

    #[test]
    fn status_line_running_monitor_off() {
        let info = SessionInfo {
            input_rate: 44100,
            cable_rate: 48000,
            monitor_rate: None,
        };
        assert_eq!(
            status_line(Some(info)),
            "Running · input 44.1 kHz · cable 48 kHz · monitor off"
        );
    }

    #[test]
    fn status_line_running_monitor_on() {
        let info = SessionInfo {
            input_rate: 48000,
            cable_rate: 48000,
            monitor_rate: Some(96000),
        };
        assert_eq!(
            status_line(Some(info)),
            "Running · input 48 kHz · cable 48 kHz · monitor 96 kHz"
        );
    }
}
