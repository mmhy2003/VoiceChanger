//! Voice tab: the Model list with the original voice, Start/Stop and Bypass, live voice controls,
//! level meters, status and the banner.

use eframe::egui;
use vc_audio::SessionInfo;

use crate::app::{AUDIO_DEAD, Tab, VcApp};
use crate::source::{ORIGINAL_VOICE, VoiceSource};
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

    let model_chosen = app.model_chosen();
    let hint = if app.is_running() {
        None
    } else {
        app.start_blocked()
    };
    ui.horizontal(|ui| {
        if app.is_running() {
            if ui.button("■ Stop").clicked() {
                app.stop();
            }
        } else if ui
            .add_enabled(hint.is_none(), egui::Button::new("▶ Start"))
            .clicked()
        {
            app.start();
        }
        ui.add_enabled(
            model_chosen,
            egui::Checkbox::new(&mut app.bypass, "Bypass: send my original voice"),
        );
    });
    if let Some(hint) = hint {
        ui.label(hint);
    }

    let pitch = ui.add_enabled(
        model_chosen,
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
    ui.add_enabled(
        model_chosen,
        egui::Slider::new(&mut voice.gate_db, -90.0..=-20.0)
            .text("Noise gate")
            .suffix(" dB"),
    );
    if !model_chosen {
        ui.label(CONVERTED_ONLY);
    }

    widgets::meter(ui, "Input", app.input_meter.shown);
    widgets::meter(ui, "Output", app.output_meter.shown);
    ui.label(status_line(
        app.session_info,
        app.session_source == Some(VoiceSource::Original),
    ));
}

/// Shown under the sliders while the original voice is selected.
const CONVERTED_ONLY: &str = "Pitch and noise gate apply to converted voices only.";

/// The text beside the Model list: the model's state, or that nothing is converted.
pub fn model_state(model_chosen: bool, slot_label: String) -> String {
    if model_chosen {
        slot_label
    } else {
        "Original voice · no conversion".to_string()
    }
}

/// What was clicked in the Model list.
enum Pick {
    Original,
    Model(String),
}

/// The Model list: "Original voice (no model)" first, then the library entries whose file
/// exists; beside it the model's state, below it a pending switch. Picking an entry switches
/// to it, also while running.
fn model_picker(ui: &mut egui::Ui, app: &mut VcApp) {
    let active = app.chosen_model().map(|e| (e.id.clone(), e.name.clone()));
    let selected = active
        .as_ref()
        .map_or(ORIGINAL_VOICE, |(_, name)| name.as_str());
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
                if ui
                    .selectable_label(active.is_none(), ORIGINAL_VOICE)
                    .clicked()
                {
                    picked = Some(Pick::Original);
                }
                for (id, name) in &choices {
                    let is_active = active.as_ref().is_some_and(|(a, _)| a == id);
                    if ui.selectable_label(is_active, name).clicked() {
                        picked = Some(Pick::Model(id.clone()));
                    }
                }
            });
        ui.label(model_state(active.is_some(), app.model.label()));
    });
    if app.library.entries().is_empty() {
        ui.horizontal(|ui| {
            ui.label("No models yet.");
            if ui.button("Open Models").clicked() {
                app.tab = Tab::Models;
            }
        });
    }
    if let Some(pending) = &app.pending {
        ui.label(format!("Switching to {}…", pending.name));
    }
    match picked {
        Some(Pick::Original) => app.select_original(),
        Some(Pick::Model(id)) => app.select_model(&id),
        None => {}
    }
}

/// Why Start is disabled, or `None` when it can be pressed: a dead audio thread first,
/// then the device `blocker`, then a chosen model that is not loaded.
pub fn start_hint(
    audio_alive: bool,
    blocker: Option<&'static str>,
    voice_ready: bool,
) -> Option<&'static str> {
    if !audio_alive {
        Some(AUDIO_DEAD)
    } else if blocker.is_some() {
        blocker
    } else if !voice_ready {
        Some("The model is not loaded")
    } else {
        None
    }
}

/// "Stopped", or the running session's voice and sample rates.
pub fn status_line(info: Option<SessionInfo>, original: bool) -> String {
    let Some(info) = info else {
        return "Stopped".to_string();
    };
    let monitor = info.monitor_rate.map_or_else(|| "off".to_string(), khz);
    let voice = if original { "original voice · " } else { "" };
    format!(
        "Running · {voice}input {} · cable {} · monitor {monitor}",
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
        assert_eq!(
            start_hint(true, None, false),
            Some("The model is not loaded")
        );
        assert_eq!(start_hint(true, None, true), None);
    }

    #[test]
    fn start_hint_original_voice_needs_no_model() {
        use crate::source::{start_source, wanted_source};
        // (model_chosen, bypass), with a model that is not loaded.
        let ready = |chosen, bypass| start_source(wanted_source(chosen, bypass), false).is_some();
        assert_eq!(start_hint(true, None, ready(false, false)), None);
        assert_eq!(
            start_hint(true, None, ready(true, false)),
            Some("The model is not loaded")
        );
        // Bypass asks for the original voice, which needs no model.
        assert_eq!(start_hint(true, None, ready(true, true)), None);
    }

    #[test]
    fn model_state_original_and_model() {
        assert_eq!(ORIGINAL_VOICE, "Original voice (no model)");
        let slot = "Ana · 40 kHz · DirectML (GPU 0)".to_string();
        assert_eq!(
            model_state(false, slot.clone()),
            "Original voice · no conversion"
        );
        assert_eq!(model_state(true, slot.clone()), slot);
    }

    #[test]
    fn status_line_stopped() {
        assert_eq!(status_line(None, false), "Stopped");
    }

    #[test]
    fn status_line_running_monitor_off() {
        let info = SessionInfo {
            input_rate: 44100,
            cable_rate: 48000,
            monitor_rate: None,
        };
        assert_eq!(
            status_line(Some(info), false),
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
            status_line(Some(info), false),
            "Running · input 48 kHz · cable 48 kHz · monitor 96 kHz"
        );
    }

    #[test]
    fn status_line_running_original() {
        let info = SessionInfo {
            input_rate: 48000,
            cable_rate: 48000,
            monitor_rate: None,
        };
        assert_eq!(
            status_line(Some(info), true),
            "Running · original voice · input 48 kHz · cable 48 kHz · monitor off"
        );
        assert_eq!(status_line(None, true), "Stopped");
    }
}
