//! RMVPE F0 extraction, salience decoding, pitch shift and RVC coarse pitch.

use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;

use crate::dsp::mel::{MelExtractor, N_MELS};
use crate::error::{EngineError, Result};
use crate::runtime::{self, ActiveProvider, Provider};

/// Salience bins per RMVPE frame (20 cents each).
pub const BINS: usize = 360;
/// A frame whose peak salience is at or below this is unvoiced.
pub const THRESHOLD: f32 = 0.03;
/// RMVPE needs the frame count padded to a multiple of this.
const FRAME_MULTIPLE: usize = 32;
const CENTS_OFFSET: f64 = 1_997.379_408_437_619;
const F0_MIN: f64 = 50.0;
const F0_MAX: f64 = 1100.0;
const MAX_SEMITONES: f32 = 24.0;

/// Decodes `sal`, row-major `[frames][BINS]`, into F0 in Hz (0 for unvoiced frames).
pub fn decode_salience(sal: &[f32], frames: usize, threshold: f32) -> Vec<f32> {
    sal.chunks(BINS)
        .take(frames)
        .map(|s| {
            let (c, &peak) = s
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap_or((0, &0.0));
            if peak <= threshold {
                return 0.0;
            }
            let range = c.saturating_sub(4)..(c + 5).min(BINS);
            let (weighted, total) = range.fold((0.0f64, 0.0f64), |(w, t), j| {
                let v = f64::from(s[j]);
                (w + v * (20.0 * j as f64 + CENTS_OFFSET), t + v)
            });
            (10.0 * 2f64.powf(weighted / total / 1200.0)) as f32
        })
        .collect()
}

/// Shifts voiced frames by `semitones`, clamped to ±24.
pub fn shift(f0: &mut [f32], semitones: f32) {
    let ratio = 2f32.powf(semitones.clamp(-MAX_SEMITONES, MAX_SEMITONES) / 12.0);
    for v in f0 {
        *v *= ratio;
    }
}

/// RVC's coarse pitch: F0 on the mel scale between 50 and 1100 Hz, mapped to 1..=255.
pub fn coarse(f0: f32) -> i64 {
    let to_mel = |f: f64| 1127.0 * (1.0 + f / 700.0).ln();
    let (mel_min, mel_max) = (to_mel(F0_MIN), to_mel(F0_MAX));
    let mut mel = to_mel(f64::from(f0));
    if mel > 0.0 {
        mel = (mel - mel_min) * 254.0 / (mel_max - mel_min) + 1.0;
    }
    mel.clamp(1.0, 255.0).round() as i64
}

/// Sets F0 to 0 wherever `open` is false.
pub fn apply_mask(f0: &mut [f32], open: &[bool]) {
    for (v, &o) in f0.iter_mut().zip(open) {
        if !o {
            *v = 0.0;
        }
    }
}

/// An RMVPE session with its log-mel front end.
pub struct Rmvpe {
    session: Session,
    mel: MelExtractor,
    active: ActiveProvider,
}

impl Rmvpe {
    /// Opens the RMVPE model at `path` on `provider`, falling back to the CPU.
    pub fn load(path: &Path, provider: Provider) -> Result<Self> {
        let (session, active) = runtime::open_session(path, provider)?;
        Ok(Self {
            session,
            mel: MelExtractor::new(),
            active,
        })
    }

    pub fn active_provider(&self) -> ActiveProvider {
        self.active
    }

    /// F0 in Hz of 16 kHz audio, one value per 160 samples plus one (`len/160 + 1`).
    pub fn f0(&mut self, audio_16k: &[f32]) -> Result<Vec<f32>> {
        let (mel, frames) = self.mel.log_mel(audio_16k);
        let padded = frames.div_ceil(FRAME_MULTIPLE) * FRAME_MULTIPLE;
        let mut data = vec![0.0f32; N_MELS * padded];
        for (dst, src) in data.chunks_mut(padded).zip(mel.chunks(frames)) {
            dst[..frames].copy_from_slice(src);
        }
        let input = Tensor::from_array(([1usize, N_MELS, padded], data)).map_err(inference)?;
        let outputs = self.session.run(ort::inputs![input]).map_err(inference)?;
        let (_, sal) = outputs[0].try_extract_tensor::<f32>().map_err(inference)?;
        if sal.len() < frames * BINS {
            return Err(EngineError::Inference(format!(
                "RMVPE returned {} values, expected at least {}",
                sal.len(),
                frames * BINS
            )));
        }
        Ok(decode_salience(&sal[..frames * BINS], frames, THRESHOLD))
    }
}

fn inference(e: ort::Error) -> EngineError {
    EngineError::Inference(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{self, ORT_DLL, RMVPE};
    use crate::runtime::Provider;
    use crate::testutil::assets_or_skip;

    fn one_hot(frames: usize, bin: usize, value: f32) -> Vec<f32> {
        let mut sal = vec![0.0; frames * BINS];
        for f in 0..frames {
            sal[f * BINS + bin] = value;
        }
        sal
    }

    #[test]
    fn decode_peak_at_bin_100() {
        let f0 = decode_salience(&one_hot(2, 100, 0.9), 2, THRESHOLD);
        let want = 10.0 * 2f64.powf((2000.0 + CENTS_OFFSET) / 1200.0);
        assert_eq!(f0.len(), 2);
        for v in f0 {
            assert!((f64::from(v) - want).abs() < 0.01, "{v} vs {want}");
        }
    }

    #[test]
    fn decode_below_threshold_is_zero() {
        let f0 = decode_salience(&one_hot(3, 100, 0.03), 3, THRESHOLD);
        assert_eq!(f0, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn decode_weighted_between_bins() {
        let mut sal = vec![0.0; BINS];
        sal[100] = 0.5;
        sal[101] = 0.5;
        let f0 = decode_salience(&sal, 1, THRESHOLD);
        let want = 10.0 * 2f64.powf((2010.0 + CENTS_OFFSET) / 1200.0);
        assert!(
            (f64::from(f0[0]) - want).abs() < 0.01,
            "{} vs {want}",
            f0[0]
        );
    }

    #[test]
    fn shift_octave_doubles() {
        let mut f0 = [110.0, 220.0];
        shift(&mut f0, 12.0);
        assert!((f0[0] - 220.0).abs() < 1e-3 && (f0[1] - 440.0).abs() < 1e-3);
    }

    #[test]
    fn shift_zero_stays_zero() {
        let mut f0 = [0.0, 100.0];
        shift(&mut f0, 7.0);
        assert_eq!(f0[0], 0.0);
        assert!(f0[1] > 100.0);
    }

    #[test]
    fn shift_clamped_to_24() {
        let mut up = [100.0];
        shift(&mut up, 40.0);
        assert!((up[0] - 400.0).abs() < 1e-3, "{}", up[0]);
        let mut down = [400.0];
        shift(&mut down, -40.0);
        assert!((down[0] - 100.0).abs() < 1e-3, "{}", down[0]);
    }

    #[test]
    fn coarse_known_values() {
        assert_eq!(coarse(0.0), 1);
        assert_eq!(coarse(50.0), 1);
        assert_eq!(coarse(1100.0), 255);
        assert_eq!(coarse(5000.0), 255);
        let mel = |f: f64| 1127.0 * (1.0 + f / 700.0).ln();
        let want = ((mel(220.0) - mel(50.0)) * 254.0 / (mel(1100.0) - mel(50.0)) + 1.0).round();
        assert_eq!(coarse(220.0), want as i64);
    }

    #[test]
    fn mask_forces_unvoiced() {
        let mut f0 = [100.0, 200.0, 300.0];
        apply_mask(&mut f0, &[true, false, true]);
        assert_eq!(f0, [100.0, 0.0, 300.0]);
    }

    #[test]
    fn rmvpe_220hz_sine() {
        if !assets_or_skip(&[ORT_DLL, RMVPE]) {
            return;
        }
        let mut rmvpe = Rmvpe::load(&assets::asset(RMVPE).unwrap(), Provider::Cpu).unwrap();
        assert_eq!(rmvpe.active_provider(), Provider::Cpu);
        let audio: Vec<f32> = (0..16000)
            .map(|n| 0.3 * (2.0 * std::f32::consts::PI * 220.0 * n as f32 / 16000.0).sin())
            .collect();
        let f0 = rmvpe.f0(&audio).unwrap();
        assert_eq!(f0.len(), audio.len() / 160 + 1);
        let mut voiced: Vec<f32> = f0[10..90].iter().copied().filter(|&v| v > 0.0).collect();
        assert!(
            voiced.len() >= 64,
            "only {} of 80 frames voiced",
            voiced.len()
        );
        voiced.sort_by(f32::total_cmp);
        let median = voiced[voiced.len() / 2];
        assert!((median - 220.0).abs() <= 220.0 * 0.02, "median {median}");
    }
}
