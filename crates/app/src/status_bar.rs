//! Status bar: measured latency, inference time per block and the active provider.

use std::time::Duration;

use eframe::egui;

/// What the status bar shows, read once per frame while running.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusSnapshot {
    /// End-to-end latency; zero until the first block.
    pub latency: Duration,
    /// Last inference time per block.
    pub inference: Duration,
    pub block: Duration,
    /// Smoothed inference time / block length.
    pub ratio: f32,
    pub provider: String,
    pub late_blocks: u64,
    pub underruns: u64,
}

/// Latency above this is shown as a warning.
pub const LATENCY_WARN: Duration = Duration::from_millis(300);
/// Inference above this share of a block is shown as a warning.
pub const RATIO_WARN: f32 = 0.6;

/// "Latency 480 ms", or "Latency –" before the first block.
pub fn latency_text(s: &StatusSnapshot) -> String {
    if s.latency.is_zero() {
        "Latency –".into()
    } else {
        format!("Latency {} ms", ms(s.latency))
    }
}

/// "Inference 140 ms / 250 ms (56%)".
pub fn inference_text(s: &StatusSnapshot) -> String {
    format!(
        "Inference {} ms / {} ms ({}%)",
        ms(s.inference),
        ms(s.block),
        (s.ratio * 100.0).round()
    )
}

/// All parts joined by " · ", as `show` lays them out.
#[cfg(test)]
pub fn status_text(s: &StatusSnapshot) -> String {
    parts(s)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

pub fn latency_warn(s: &StatusSnapshot) -> bool {
    s.latency > LATENCY_WARN
}

pub fn ratio_warn(s: &StatusSnapshot) -> bool {
    s.ratio > RATIO_WARN
}

/// Draws the parts in one row, warnings in the theme's warning colour.
pub fn show(ui: &mut egui::Ui, s: &StatusSnapshot) {
    let warn_color = ui.visuals().warn_fg_color;
    ui.horizontal(|ui| {
        for (i, (text, warn)) in parts(s).into_iter().enumerate() {
            if i > 0 {
                ui.label(SEPARATOR.trim());
            }
            if warn {
                ui.colored_label(warn_color, text);
            } else {
                ui.label(text);
            }
        }
    });
}

const SEPARATOR: &str = " · ";

/// Each part's text and whether it is a warning.
fn parts(s: &StatusSnapshot) -> [(String, bool); 5] {
    [
        (latency_text(s), latency_warn(s)),
        (inference_text(s), ratio_warn(s)),
        (s.provider.clone(), false),
        (format!("late blocks {}", s.late_blocks), false),
        (format!("underruns {}", s.underruns), false),
    ]
}

/// Whole milliseconds, rounded.
fn ms(d: Duration) -> u128 {
    (d.as_micros() + 500) / 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> StatusSnapshot {
        StatusSnapshot {
            latency: Duration::from_millis(480),
            inference: Duration::from_millis(140),
            block: Duration::from_millis(250),
            ratio: 0.56,
            provider: "DirectML (GPU 0)".into(),
            late_blocks: 0,
            underruns: 3,
        }
    }

    #[test]
    fn status_text_full() {
        assert_eq!(
            status_text(&snapshot()),
            "Latency 480 ms · Inference 140 ms / 250 ms (56%) · DirectML (GPU 0) \
             · late blocks 0 · underruns 3"
        );
    }

    #[test]
    fn latency_placeholder_before_first_block() {
        let s = StatusSnapshot {
            latency: Duration::ZERO,
            ..snapshot()
        };
        assert_eq!(latency_text(&s), "Latency –");
        assert!(!latency_warn(&s));
    }

    #[test]
    fn ms_are_rounded() {
        let s = StatusSnapshot {
            latency: Duration::from_micros(479_600),
            inference: Duration::from_micros(139_400),
            ratio: 0.5649,
            ..snapshot()
        };
        assert_eq!(latency_text(&s), "Latency 480 ms");
        assert_eq!(inference_text(&s), "Inference 139 ms / 250 ms (56%)");
    }

    #[test]
    fn thresholds() {
        let at = |ms, ratio| StatusSnapshot {
            latency: Duration::from_millis(ms),
            ratio,
            ..snapshot()
        };
        assert!(!latency_warn(&at(300, 0.5)));
        assert!(latency_warn(&at(301, 0.5)));
        assert!(!ratio_warn(&at(200, 0.6)));
        assert!(ratio_warn(&at(200, 0.61)));
    }
}
