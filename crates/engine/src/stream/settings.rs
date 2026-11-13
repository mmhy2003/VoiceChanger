//! Stream block sizes (restart to change) and live controls (applied on the next block).

use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::pipeline::Params;

pub const BLOCK_MS: RangeInclusive<u32> = 50..=1000;
pub const CONTEXT_MS: RangeInclusive<u32> = 0..=2000;
pub const CROSSFADE_MS: RangeInclusive<u32> = 10..=200;
/// SOLA search slack on top of the crossfade.
pub const SEARCH_MS: u32 = 10;
pub const PITCH_RANGE: RangeInclusive<f32> = -24.0..=24.0;
pub const GATE_DB_RANGE: RangeInclusive<f32> = -90.0..=-20.0;

/// Samples per millisecond at 16 kHz.
const PER_MS: usize = 16;
/// HuBERT hop at 16 kHz; the window must be a whole number of hops.
const HOP: usize = 320;
/// Half a hop: the context gains this when the window would otherwise be misaligned.
const HALF_HOP: usize = HOP / 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSettings {
    pub block_ms: u32,
    pub context_ms: u32,
    pub crossfade_ms: u32,
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self {
            block_ms: 250,
            context_ms: 500,
            crossfade_ms: 50,
        }
    }
}

fn round_clamp(ms: u32, range: &RangeInclusive<u32>) -> u32 {
    (ms.saturating_add(5) / 10 * 10).clamp(*range.start(), *range.end())
}

impl StreamSettings {
    /// Each value rounded to the nearest 10 ms, then clamped to its range.
    pub fn clamped(self) -> Self {
        Self {
            block_ms: round_clamp(self.block_ms, &BLOCK_MS),
            context_ms: round_clamp(self.context_ms, &CONTEXT_MS),
            crossfade_ms: round_clamp(self.crossfade_ms, &CROSSFADE_MS),
        }
    }

    // The sizes below use the clamped settings and are 16 kHz sample counts.

    pub fn block_16k(&self) -> usize {
        self.clamped().block_ms as usize * PER_MS
    }

    pub fn crossfade_16k(&self) -> usize {
        self.clamped().crossfade_ms as usize * PER_MS
    }

    pub fn search_16k(&self) -> usize {
        SEARCH_MS as usize * PER_MS
    }

    /// `context_ms` at 16 kHz, plus 160 samples when needed so `window_16k() % 320 == 0`.
    pub fn context_16k(&self) -> usize {
        let context = self.clamped().context_ms as usize * PER_MS;
        let rest = context + self.crossfade_16k() + self.search_16k() + self.block_16k();
        if rest.is_multiple_of(HOP) {
            context
        } else {
            context + HALF_HOP
        }
    }

    /// Past context + crossfade + search + block.
    pub fn window_16k(&self) -> usize {
        self.context_16k() + self.crossfade_16k() + self.search_16k() + self.block_16k()
    }
}

/// Live voice controls, shared between the UI and the inference worker (f32 bits in atomics).
#[derive(Debug)]
pub struct Controls {
    pitch: AtomicU32,
    gate_db: AtomicU32,
}

impl Default for Controls {
    fn default() -> Self {
        Self::new()
    }
}

impl Controls {
    /// Pitch 0 semitones, gate -50 dB.
    pub fn new() -> Self {
        let defaults = Params::default();
        Self {
            pitch: AtomicU32::new(defaults.pitch_semitones.to_bits()),
            gate_db: AtomicU32::new(defaults.gate_db.to_bits()),
        }
    }

    /// Clamped to [`PITCH_RANGE`].
    pub fn set_pitch(&self, semitones: f32) {
        let v = semitones.clamp(*PITCH_RANGE.start(), *PITCH_RANGE.end());
        self.pitch.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn pitch(&self) -> f32 {
        f32::from_bits(self.pitch.load(Ordering::Relaxed))
    }

    /// Clamped to [`GATE_DB_RANGE`].
    pub fn set_gate_db(&self, db: f32) {
        let v = db.clamp(*GATE_DB_RANGE.start(), *GATE_DB_RANGE.end());
        self.gate_db.store(v.to_bits(), Ordering::Relaxed);
    }

    pub fn gate_db(&self) -> f32 {
        f32::from_bits(self.gate_db.load(Ordering::Relaxed))
    }

    /// The current controls as pipeline parameters (speaker 0).
    pub fn params(&self) -> Params {
        Params {
            pitch_semitones: self.pitch(),
            gate_db: self.gate_db(),
            speaker_id: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(block_ms: u32, context_ms: u32, crossfade_ms: u32) -> StreamSettings {
        StreamSettings {
            block_ms,
            context_ms,
            crossfade_ms,
        }
    }

    #[test]
    fn defaults_are_prd() {
        assert_eq!(StreamSettings::default(), settings(250, 500, 50));
    }

    #[test]
    fn clamps_and_rounds() {
        assert_eq!(settings(10, 5000, 999).clamped(), settings(50, 2000, 200));
        assert_eq!(settings(255, 500, 50).clamped().block_ms, 260);
    }

    #[test]
    fn sizes_at_16k() {
        let s = StreamSettings::default();
        assert_eq!(s.block_16k(), 4000);
        assert_eq!(s.crossfade_16k(), 800);
        assert_eq!(s.search_16k(), 160);
        assert!(s.context_16k() == 8000 || s.context_16k() == 8160);
        assert_eq!(s.window_16k() % 320, 0);
    }

    #[test]
    fn window_is_aligned_for_all() {
        for block in (50..=1000).step_by(10) {
            for context in (0..=2000).step_by(10) {
                for crossfade in [10, 50, 200] {
                    let s = settings(block, context, crossfade);
                    let c = s.context_16k();
                    let ms16 = context as usize * 16;
                    assert_eq!(s.window_16k() % 320, 0, "{s:?}");
                    assert_eq!(c % 160, 0, "{s:?}");
                    assert!(ms16 <= c && c < ms16 + 320, "{s:?}");
                }
            }
        }
    }

    #[test]
    fn controls_defaults_and_clamp() {
        let c = Controls::new();
        assert_eq!(c.pitch(), 0.0);
        assert_eq!(c.gate_db(), -50.0);
        c.set_pitch(30.0);
        assert_eq!(c.pitch(), 24.0);
        c.set_pitch(-30.0);
        assert_eq!(c.pitch(), -24.0);
        c.set_gate_db(-100.0);
        assert_eq!(c.gate_db(), -90.0);
        c.set_gate_db(0.0);
        assert_eq!(c.gate_db(), -20.0);
        let p = c.params();
        assert_eq!(p.pitch_semitones, -24.0);
        assert_eq!(p.gate_db, -20.0);
        assert_eq!(p.speaker_id, 0);
    }
}
