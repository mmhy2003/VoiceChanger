//! `voicechanger.exe --self-test`: checks an installed copy without opening a window.

use std::fmt;
use std::path::PathBuf;

use vc_engine::Provider;
use vc_engine::assets::{self, HUBERT, RMVPE};
use vc_engine::f0::Rmvpe;
use vc_engine::hubert::ContentEncoder;

pub const FLAG: &str = "--self-test";
const SAMPLE_RATE: usize = 16_000;
const TONE_HZ: f32 = 220.0;

/// True when the command line asks for the self-test.
pub fn requested(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().skip(1).any(|a| a == FLAG)
}

/// What the base models made of the test tone.
#[derive(Debug)]
pub struct Report {
    pub content_frames: usize,
    pub pitch_hz: f32,
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "self-test passed: {} content frames, pitch {:.0} Hz",
            self.content_frames, self.pitch_hz
        )
    }
}

/// Loads the base models from `dirs` on the CPU and runs one second of a 220 Hz tone
/// through them.
pub fn run(dirs: &[PathBuf]) -> Result<Report, String> {
    let hubert = assets::resolve(HUBERT, dirs).map_err(|e| e.to_string())?;
    let rmvpe = assets::resolve(RMVPE, dirs).map_err(|e| e.to_string())?;
    let tone: Vec<f32> = (0..SAMPLE_RATE)
        .map(|i| 0.5 * (std::f32::consts::TAU * TONE_HZ * i as f32 / SAMPLE_RATE as f32).sin())
        .collect();
    let mut encoder = ContentEncoder::load(&hubert, Provider::Cpu).map_err(|e| e.to_string())?;
    let (_, content_frames) = encoder.encode(&tone).map_err(|e| e.to_string())?;
    let mut f0 = Rmvpe::load(&rmvpe, Provider::Cpu).map_err(|e| e.to_string())?;
    let mut voiced: Vec<f32> = f0
        .f0(&tone)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|hz| *hz > 0.0)
        .collect();
    if voiced.is_empty() {
        return Err("RMVPE found no pitch in the test tone".into());
    }
    voiced.sort_by(f32::total_cmp);
    Ok(Report {
        content_frames,
        pitch_hz: voiced[voiced.len() / 2],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;
    use vc_engine::assets;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flag_is_recognised_after_the_program_name() {
        assert!(requested(args(&["voicechanger.exe", "--self-test"])));
        assert!(!requested(args(&["voicechanger.exe"])));
        assert!(!requested(args(&["--self-test"])));
    }

    #[test]
    fn missing_models_are_named() {
        let dir = TempDir::new("self-test-missing");
        let err = run(&[dir.path().to_path_buf()]).unwrap_err();
        assert!(
            err.replace('\\', "/").contains("models/hubert.onnx"),
            "{err}"
        );
    }

    #[test]
    fn base_models_run_on_the_cpu() {
        let dirs = assets::candidates();
        if [assets::HUBERT, assets::RMVPE, assets::ORT_DLL]
            .iter()
            .any(|rel| assets::resolve(rel, &dirs).is_err())
        {
            assert_ne!(
                std::env::var("VC_REQUIRE_ASSETS").as_deref(),
                Ok("1"),
                "missing base models: run `cargo xtask fetch-models`"
            );
            eprintln!("skipped: base models not fetched");
            return;
        }
        let report = run(&dirs).unwrap();
        assert_eq!(report.content_frames, 50, "{report}");
        assert!((report.pitch_hz - TONE_HZ).abs() < 5.0, "{report}");
    }
}
