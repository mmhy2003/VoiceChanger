//! Noise gate on 16 kHz audio: per-frame RMS threshold with hold, applied through a ramped gain.

/// Gate frame length in samples (10 ms at 16 kHz).
pub const FRAME: usize = 160;

#[derive(Clone, Copy, Debug)]
pub struct Gate {
    pub threshold_db: f32,
    pub hold_frames: usize,
    pub ramp: usize,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            hold_frames: 5,
            ramp: 80,
        }
    }
}

/// One flag per `FRAME` samples (the last frame may be partial); a frame is open when its RMS level
/// reaches `threshold_db`, and stays open for `hold_frames` frames after the last open one.
pub fn frame_mask(x: &[f32], threshold_db: f32, hold_frames: usize) -> Vec<bool> {
    let mut held = 0;
    x.chunks(FRAME)
        .map(|frame| {
            let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
            if 20.0 * (rms + 1e-9).log10() >= threshold_db {
                held = hold_frames;
                true
            } else if held > 0 {
                held -= 1;
                true
            } else {
                false
            }
        })
        .collect()
}

/// Per-sample gain for `len` samples that ramps linearly toward each frame's state (1 open, 0 closed)
/// by `1/ramp` per sample, starting at the first frame's state.
pub fn envelope(mask: &[bool], len: usize, ramp: usize) -> Vec<f32> {
    let target = |i: usize| match mask.get(i / FRAME) {
        Some(&open) => f32::from(u8::from(open)),
        None => 0.0,
    };
    let step = 1.0 / ramp.max(1) as f32;
    let mut gain = target(0);
    (0..len)
        .map(|i| {
            let t = target(i);
            gain = if t > gain {
                (gain + step).min(t)
            } else {
                (gain - step).max(t)
            };
            gain
        })
        .collect()
}

impl Gate {
    /// Returns the gated signal and the frame mask.
    pub fn apply(&self, x: &[f32]) -> (Vec<f32>, Vec<bool>) {
        let mask = frame_mask(x, self.threshold_db, self.hold_frames);
        let gain = envelope(&mask, x.len(), self.ramp);
        let y = x.iter().zip(&gain).map(|(s, g)| s * g).collect();
        (y, mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(amp: f32, frames: usize) -> Vec<f32> {
        vec![amp; frames * FRAME]
    }

    fn loud_then_silent() -> Vec<f32> {
        let mut x = constant(0.1, 10);
        x.extend(constant(0.0, 10));
        x
    }

    #[test]
    fn loud_frames_open_quiet_closed() {
        let mask = frame_mask(&loud_then_silent(), -50.0, 0);
        let mut expected = vec![true; 10];
        expected.extend([false; 10]);
        assert_eq!(mask, expected);
    }

    #[test]
    fn hold_extends_open() {
        let mask = frame_mask(&loud_then_silent(), -50.0, 3);
        assert_eq!(mask.len(), 20);
        assert!(mask[..13].iter().all(|&m| m));
        assert!(mask[13..].iter().all(|&m| !m));
    }

    #[test]
    fn threshold_respected() {
        let x = constant(10f32.powf(-60.0 / 20.0), 4);
        assert_eq!(frame_mask(&x, -50.0, 0), vec![false; 4]);
        assert_eq!(frame_mask(&x, -70.0, 0), vec![true; 4]);
    }

    #[test]
    fn envelope_has_no_jumps() {
        let mask: Vec<bool> = (0..20).map(|i| i % 2 == 0).collect();
        let gain = envelope(&mask, mask.len() * FRAME, 80);
        assert_eq!(gain.len(), mask.len() * FRAME);
        assert!(gain.iter().all(|g| (0.0..=1.0).contains(g)));
        assert_eq!(gain[0], 1.0);
        let max_step = gain
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step <= 1.0 / 80.0 + 1e-6, "max step {max_step}");
        assert!(gain.contains(&0.0), "gain never closes");
    }

    #[test]
    fn silent_input_is_zero() {
        let (y, mask) = Gate::default().apply(&constant(0.0, 5));
        assert_eq!(mask, vec![false; 5]);
        assert!(y.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn loud_input_passes_unchanged() {
        let x = constant(0.1, 5);
        let (y, mask) = Gate::default().apply(&x);
        assert_eq!(mask, vec![true; 5]);
        assert_eq!(y, x);
    }

    #[test]
    fn partial_last_frame_counted() {
        assert_eq!(frame_mask(&vec![0.1; 170], -50.0, 0), vec![true, true]);
    }
}
