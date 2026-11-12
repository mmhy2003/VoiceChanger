//! Top-level app state and the tab bar.

use eframe::egui;

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

/// The eframe application.
pub struct VcApp {
    tab: Tab,
}

impl VcApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            tab: Tab::default(),
        }
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
            Tab::Settings => tabs::settings::show(ui),
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
}
