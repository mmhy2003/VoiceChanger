//! Checks a converted model on the CPU: it loads, and one second of
//! synthetic input gives audio of the right length that is finite and not silent.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use vc_engine::generator::{GenInput, Generator};
use vc_engine::runtime::Provider;

use crate::ConvertError;

/// Frames of synthetic input: 1 s at 100 frames/s.
pub const FRAMES: usize = 100;

/// Features per frame of the v2 content input.
const PHONE_DIM: usize = 768;

/// Pitch of the synthetic input, in Hz.
const PITCH_HZ: f32 = 220.0;

/// Output quieter than this (RMS, dBFS) counts as silent.
const SILENCE_DB: f32 = -60.0;

fn failed(e: impl ToString) -> ConvertError {
    ConvertError::ValidationFailed(e.to_string())
}

/// The model's output for [`FRAMES`] frames of synthetic input, and its sample rate.
pub fn synthetic_output(onnx: &[u8]) -> Result<(Vec<f32>, u32), ConvertError> {
    let mut generator = Generator::load_from_memory(onnx, Provider::Cpu, None).map_err(failed)?;
    let mut rng = StdRng::seed_from_u64(0);
    let phone: Vec<f32> = (0..FRAMES * PHONE_DIM)
        .map(|_| rng.random_range(-1.0..1.0))
        .collect();
    let pitchf = vec![PITCH_HZ; FRAMES];
    let pitch = vec![vc_engine::f0::coarse(PITCH_HZ); FRAMES];
    let audio = generator
        .run(&GenInput {
            phone: &phone,
            frames: FRAMES,
            pitch: &pitch,
            pitchf: &pitchf,
            speaker_id: 0,
        })
        .map_err(failed)?;
    Ok((audio, generator.sample_rate()))
}

/// Whether `audio` looks like the output of a working generator for `frames` frames.
pub fn judge(audio: &[f32], frames: usize, sample_rate: u32) -> Result<(), String> {
    let want = frames * (sample_rate as usize / 100);
    if audio.len() != want {
        return Err(format!("length {}, expected {want}", audio.len()));
    }
    if !audio.iter().all(|v| v.is_finite()) {
        return Err("non-finite output".into());
    }
    let mean_square = audio
        .iter()
        .map(|&v| f64::from(v) * f64::from(v))
        .sum::<f64>()
        / audio.len().max(1) as f64;
    let db = (10.0 * mean_square.log10()) as f32;
    if db.is_nan() || db <= SILENCE_DB {
        return Err(format!("silent output ({db:.1} dBFS)"));
    }
    Ok(())
}

/// Loads `onnx` on the CPU, runs synthetic input through it and judges the output.
pub fn check(onnx: &[u8]) -> Result<(), ConvertError> {
    let (audio, sample_rate) = synthetic_output(onnx)?;
    judge(&audio, FRAMES, sample_rate).map_err(ConvertError::ValidationFailed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::assets_or_skip;
    use vc_engine::assets::ORT_DLL;

    fn sine(len: usize, amplitude: f32) -> Vec<f32> {
        (0..len)
            .map(|i| amplitude * (i as f32 * 440.0 * std::f32::consts::TAU / 40000.0).sin())
            .collect()
    }

    #[test]
    fn judge_accepts_good() {
        assert_eq!(judge(&sine(100 * 400, 0.1), 100, 40000), Ok(()));
    }

    #[test]
    fn judge_rejects_length() {
        assert_eq!(
            judge(&sine(100 * 400 - 1, 0.1), 100, 40000),
            Err("length 39999, expected 40000".into())
        );
        assert_eq!(
            judge(&sine(100 * 480, 0.1), 100, 40000),
            Err("length 48000, expected 40000".into())
        );
    }

    #[test]
    fn judge_rejects_nan_and_inf() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut audio = sine(100 * 400, 0.1);
            audio[123] = bad;
            assert_eq!(
                judge(&audio, 100, 40000),
                Err("non-finite output".into()),
                "{bad}"
            );
        }
    }

    #[test]
    fn judge_rejects_silence() {
        assert_eq!(
            judge(&vec![0.0; 100 * 400], 100, 40000),
            Err("silent output (-inf dBFS)".into())
        );
        let r = judge(&sine(100 * 400, 1e-4), 100, 40000);
        assert!(
            r.as_ref()
                .is_err_and(|e| e.starts_with("silent output (-83.")),
            "got {r:?}"
        );
    }

    #[test]
    fn check_rejects_not_onnx() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let r = check(b"not an onnx model");
        assert!(
            matches!(r, Err(ConvertError::ValidationFailed(_))),
            "got {r:?}"
        );
        let r = synthetic_output(b"not an onnx model");
        assert!(
            matches!(r, Err(ConvertError::ValidationFailed(_))),
            "got {r:?}"
        );
    }
}
