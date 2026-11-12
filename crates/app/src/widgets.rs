//! Reusable widgets: level meters.

use eframe::egui;
use vc_audio::gain_to_db;

/// Lowest level shown on a meter, in dBFS.
const FLOOR_DB: f32 = -60.0;

/// Release time constant of a meter, in seconds.
const RELEASE_S: f32 = 0.3;

/// Maps a linear peak to 0..=1 over -60..0 dBFS.
pub fn level_to_fraction(peak: f32) -> f32 {
    ((gain_to_db(peak) - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// The level a meter shows: fast attack, ~300 ms release.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterState {
    /// Linear peak currently shown.
    pub shown: f32,
}

impl MeterState {
    /// `shown = max(new_peak, shown * exp(-dt / 0.3))`; returns the new `shown`.
    pub fn update(&mut self, new_peak: f32, dt: f32) -> f32 {
        self.shown = new_peak.max(self.shown * (-dt / RELEASE_S).exp());
        self.shown
    }

    /// Back to silence.
    pub fn reset(&mut self) {
        self.shown = 0.0;
    }
}

/// Draws `label` and a bar for the linear peak `shown`, with its level in dB.
pub fn meter(ui: &mut egui::Ui, label: &str, shown: f32) {
    let db = gain_to_db(shown);
    let text = if db < FLOOR_DB {
        "-∞ dB".to_string()
    } else {
        format!("{db:.1} dB")
    };
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::ProgressBar::new(level_to_fraction(shown)).text(text));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use vc_audio::db_to_gain;

    #[test]
    fn fraction_bounds() {
        assert_eq!(level_to_fraction(0.0), 0.0);
        assert_eq!(level_to_fraction(1.0), 1.0);
        assert!((level_to_fraction(db_to_gain(-30.0)) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn meter_attack_is_instant() {
        let mut m = MeterState::default();
        assert_eq!(m.update(0.8, 0.016), 0.8);
        assert_eq!(m.shown, 0.8);
    }

    #[test]
    fn meter_decays() {
        let mut m = MeterState::default();
        m.update(0.8, 0.016);
        let shown = m.update(0.0, 0.3);
        assert!((shown - 0.8 * (-1f32).exp()).abs() < 1e-3);
        m.reset();
        assert_eq!(m.shown, 0.0);
    }
}
