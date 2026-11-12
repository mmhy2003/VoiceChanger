//! Top-level app state and the tab bar.

use eframe::egui;
use vc_audio::{DEFAULT_MONITOR_DB, DeviceList, Selection, list_devices};

use crate::tabs;

/// Window title, including the app version.
pub fn window_title() -> String {
    format!("VoiceChanger {}", env!("CARGO_PKG_VERSION"))
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
}

impl VcApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            tab: Tab::default(),
            state: AppState::new(list_devices()),
        }
    }

    /// Re-enumerates the audio devices.
    pub(crate) fn refresh_devices(&mut self) {
        self.state.refresh_with(list_devices());
    }
}

impl eframe::App for VcApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for t in Tab::ALL {
                    ui.selectable_value(&mut self.tab, t, t.label());
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Voice => tabs::voice::show(ui),
            Tab::Models => tabs::models::show(ui),
            Tab::Convert => tabs::convert::show(ui),
            Tab::Settings => tabs::settings::show(ui, self),
        });
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
}
