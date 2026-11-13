//! Lock-free peak/RMS level meters and dB conversions.

use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Peak and RMS of a block of samples (linear, 0.0..=1.0 for full scale).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level {
    /// Largest absolute sample value.
    pub peak: f32,
    /// Root mean square of the samples.
    pub rms: f32,
}

/// Peak = max |x|, RMS = sqrt(mean(x²)). Empty slice -> `Level::default()`.
pub fn measure(samples: &[f32]) -> Level {
    if samples.is_empty() {
        return Level::default();
    }
    let (peak, sum_sq) = samples.iter().fold((0f32, 0f64), |(peak, sum), &x| {
        (peak.max(x.abs()), sum + f64::from(x) * f64::from(x))
    });
    Level {
        peak,
        rms: (sum_sq / samples.len() as f64).sqrt() as f32,
    }
}

/// Lock-free level: f32 bits in `AtomicU32`, Relaxed stores/loads.
#[derive(Debug, Default)]
pub struct LevelMeter {
    peak: AtomicU32,
    rms: AtomicU32,
}

impl LevelMeter {
    /// Publishes a level.
    pub fn store(&self, level: Level) {
        self.peak.store(level.peak.to_bits(), Relaxed);
        self.rms.store(level.rms.to_bits(), Relaxed);
    }

    /// Reads the last published level.
    pub fn load(&self) -> Level {
        Level {
            peak: f32::from_bits(self.peak.load(Relaxed)),
            rms: f32::from_bits(self.rms.load(Relaxed)),
        }
    }

    /// Sets the level back to zero.
    pub fn reset(&self) {
        self.store(Level::default());
    }
}

/// Decibels to linear gain.
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Linear gain to decibels: `20*log10(gain)`, floored at -120.0 for gain <= 1e-6.
pub fn gain_to_db(gain: f32) -> f32 {
    if gain <= 1e-6 {
        -120.0
    } else {
        20.0 * gain.log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_empty_is_zero() {
        assert_eq!(measure(&[]), Level::default());
    }

    #[test]
    fn measure_dc() {
        let level = measure(&[0.5; 100]);
        assert!((level.peak - 0.5).abs() < 1e-6);
        assert!((level.rms - 0.5).abs() < 1e-6);
    }

    #[test]
    fn measure_sine() {
        let samples: Vec<f32> = (0..48_000)
            .map(|i| 0.8 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let level = measure(&samples);
        assert!((level.peak - 0.8).abs() < 1e-3, "peak {}", level.peak);
        assert!(
            (level.rms - 0.8 / 2f32.sqrt()).abs() < 1e-3,
            "rms {}",
            level.rms
        );
    }

    #[test]
    fn measure_uses_absolute_peak() {
        assert_eq!(measure(&[0.1, -0.9]).peak, 0.9);
    }

    #[test]
    fn level_meter_round_trip() {
        let meter = LevelMeter::default();
        let level = Level {
            peak: 0.75,
            rms: 0.25,
        };
        meter.store(level);
        assert_eq!(meter.load(), level);
        meter.reset();
        assert_eq!(meter.load(), Level::default());
    }

    #[test]
    fn db_gain_conversions() {
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(-6.0) - 0.501).abs() < 1e-3);
        assert_eq!(gain_to_db(1.0), 0.0);
        assert_eq!(gain_to_db(0.0), -120.0);
    }
}
