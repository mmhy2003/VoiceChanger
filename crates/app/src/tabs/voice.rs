//! Voice tab: model picker, Start/Stop, level meters, status and the device-loss banner.

use eframe::egui;
use vc_audio::{AudioSession, SessionInfo, start_blocker};

use crate::app::VcApp;
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

    ui.horizontal(|ui| {
        ui.label(app.model.label());
        let busy = app.is_running() || app.model.is_loading() || app.pick_rx.is_some();
        if ui.add_enabled(!busy, egui::Button::new("Model…")).clicked() {
            app.pick_model();
        }
    });

    if app.is_running() {
        if ui.button("■ Stop").clicked() {
            app.stop();
        }
    } else {
        let blocker = start_blocker(&app.state.devices, &app.state.selection);
        if ui
            .add_enabled(blocker.is_none(), egui::Button::new("▶ Start"))
            .clicked()
        {
            app.start();
        }
        if let Some(blocker) = blocker {
            ui.label(blocker);
        }
    }

    widgets::meter(ui, "Input", app.input_meter.shown);
    widgets::meter(ui, "Output", app.output_meter.shown);
    ui.label(status_line(app.session.as_ref().map(AudioSession::info)));
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
