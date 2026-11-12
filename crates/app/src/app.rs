//! Top-level app state and the tab bar.

use std::sync::Arc;
use std::time::Duration;

use eframe::egui;
use vc_audio::{
    AudioSession, DEFAULT_MONITOR_DB, DeviceList, Endpoint, Selection, Shared, StreamStatus,
    db_to_gain, list_devices,
};

use crate::tabs;
use crate::widgets::MeterState;

/// Window title, including the app version.
pub fn window_title() -> String {
    format!("VoiceChanger {}", env!("CARGO_PKG_VERSION"))
}

/// Banner text for a session that stopped with `status`; `None` while it is running.
pub fn banner_text(status: StreamStatus, sel: &Selection) -> Option<String> {
    let device = |which| match which {
        Endpoint::Input => ("input", sel.input.as_str()),
        Endpoint::Cable => ("cable output", sel.cable.as_str()),
        Endpoint::Monitor => ("monitor", sel.monitor.as_deref().unwrap_or_default()),
    };
    match status {
        StreamStatus::Running => None,
        StreamStatus::DeviceLost { which } => {
            let (role, name) = device(which);
            Some(format!(
                "Audio stopped: the {role} device \"{name}\" was disconnected. \
                 Pick another device in Settings and press Start."
            ))
        }
        StreamStatus::Failed { which } => {
            let (role, name) = device(which);
            Some(format!(
                "Audio stopped: the {role} device \"{name}\" reported an error. \
                 Press Start to try again."
            ))
        }
    }
}

/// The tabs of the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Voice,
    Models,
    Convert,
    Settings,
}

impl Tab {
    /// All tabs, in display order.
    pub const ALL: [Tab; 4] = [Tab::Voice, Tab::Models, Tab::Convert, Tab::Settings];

    /// Label shown in the tab bar.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Voice => "Voice",
            Tab::Models => "Models",
            Tab::Convert => "Convert",
            Tab::Settings => "Settings",
        }
    }
}

/// Device list and the user's choices, kept apart from the UI so it can be tested without hardware.
#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub devices: DeviceList,
    pub selection: Selection,
    /// Monitor volume in dB.
    pub monitor_db: f32,
    /// Messages from the last Refresh, e.g. a selected device that disappeared.
    pub notes: Vec<String>,
}

impl AppState {
    /// Default selection for `devices`, default monitor volume, no notes.
    pub fn new(devices: DeviceList) -> Self {
        Self {
            selection: Selection::defaults(&devices),
            devices,
            monitor_db: DEFAULT_MONITOR_DB,
            notes: Vec::new(),
        }
    }

    /// Applies a fresh device list and keeps the reconcile notes.
    pub fn refresh_with(&mut self, devices: DeviceList) {
        self.devices = devices;
        self.notes = self.selection.reconcile(&self.devices);
    }
}

/// The eframe application.
pub struct VcApp {
    pub(crate) tab: Tab,
    pub(crate) state: AppState,
    pub(crate) session: Option<AudioSession>,
    /// Created once and reused by every session.
    shared: Arc<Shared>,
    /// Why audio stopped or could not start, shown on the Voice tab.
    pub(crate) banner: Option<String>,
    pub(crate) input_meter: MeterState,
    pub(crate) output_meter: MeterState,
}

impl VcApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            tab: Tab::default(),
            state: AppState::new(list_devices()),
            session: None,
            shared: Arc::new(Shared::new()),
            banner: None,
            input_meter: MeterState::default(),
            output_meter: MeterState::default(),
        }
    }

    /// Re-enumerates the audio devices.
    pub(crate) fn refresh_devices(&mut self) {
        self.state.refresh_with(list_devices());
    }

    pub(crate) fn is_running(&self) -> bool {
        self.session.is_some()
    }

    /// Starts audio with the current selection; on failure, the banner says why.
    pub(crate) fn start(&mut self) {
        self.banner = None;
        match AudioSession::start(&self.state.selection, self.shared.clone()) {
            Ok(session) => self.session = Some(session),
            Err(e) => self.banner = Some(format!("Could not start audio: {e}")),
        }
    }

    /// Stops audio and clears the meters.
    pub(crate) fn stop(&mut self) {
        self.session = None;
        self.input_meter.reset();
        self.output_meter.reset();
    }

    /// Stops a session whose device was lost or failed, and says why in the banner.
    fn poll_status(&mut self) {
        if !self.is_running() {
            return;
        }
        if let Some(text) = banner_text(self.shared.status(), &self.state.selection) {
            self.stop();
            self.banner = Some(text);
            self.refresh_devices();
        }
    }
}

impl eframe::App for VcApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_status();
        self.shared
            .set_monitor_gain(db_to_gain(self.state.monitor_db));
        if self.is_running() {
            let dt = ctx.input(|i| i.stable_dt);
            self.input_meter
                .update(self.shared.input_meter.load().peak, dt);
            self.output_meter
                .update(self.shared.output_meter.load().peak, dt);
        }

        // A volume change is not a selection change, so it doesn't restart the session.
        let before = self.state.selection.clone();
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for t in Tab::ALL {
                    ui.selectable_value(&mut self.tab, t, t.label());
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Voice => tabs::voice::show(ui, self),
            Tab::Models => tabs::models::show(ui),
            Tab::Convert => tabs::convert::show(ui),
            Tab::Settings => tabs::settings::show(ui, self),
        });
        if self.is_running() && self.state.selection != before {
            self.stop();
            self.start();
        }

        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_title_has_version() {
        assert_eq!(
            window_title(),
            format!("VoiceChanger {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn tabs_in_order() {
        assert_eq!(
            Tab::ALL.map(Tab::label),
            ["Voice", "Models", "Convert", "Settings"]
        );
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn vc_app_state_defaults() {
        let list = DeviceList::from_names(
            names(&["USB Mic", "Microphone (Realtek)"]),
            names(&["Headphones", "CABLE Input (VB-Audio Virtual Cable)"]),
            Some("USB Mic".into()),
            Some("Headphones".into()),
        );
        let mut state = AppState::new(list.clone());
        assert_eq!(state.devices, list);
        assert_eq!(state.selection, Selection::defaults(&list));
        assert_eq!(state.monitor_db, DEFAULT_MONITOR_DB);
        assert!(state.notes.is_empty());

        let unplugged = DeviceList::from_names(
            names(&["Microphone (Realtek)"]),
            names(&["Headphones", "CABLE Input (VB-Audio Virtual Cable)"]),
            Some("Microphone (Realtek)".into()),
            Some("Headphones".into()),
        );
        state.refresh_with(unplugged.clone());
        assert_eq!(state.devices, unplugged);
        assert_eq!(state.selection.input, "Microphone (Realtek)");
        assert_eq!(state.notes.len(), 1);
    }

    fn selection() -> Selection {
        Selection {
            input: "USB Mic".into(),
            cable: "CABLE Input (VB-Audio Virtual Cable)".into(),
            monitor: Some("Headphones".into()),
            monitor_enabled: true,
        }
    }

    #[test]
    fn banner_text_device_lost_input() {
        assert_eq!(
            banner_text(
                StreamStatus::DeviceLost {
                    which: Endpoint::Input
                },
                &selection()
            )
            .as_deref(),
            Some(
                "Audio stopped: the input device \"USB Mic\" was disconnected. \
                 Pick another device in Settings and press Start."
            )
        );
    }

    #[test]
    fn banner_text_failed_cable() {
        assert_eq!(
            banner_text(
                StreamStatus::Failed {
                    which: Endpoint::Cable
                },
                &selection()
            )
            .as_deref(),
            Some(
                "Audio stopped: the cable output device \"CABLE Input (VB-Audio Virtual Cable)\" \
                 reported an error. Press Start to try again."
            )
        );
    }

    #[test]
    fn banner_text_running_none() {
        assert_eq!(banner_text(StreamStatus::Running, &selection()), None);
    }
}
