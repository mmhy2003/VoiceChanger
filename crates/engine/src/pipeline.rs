//! Block pipeline: gate → ContentVec → RMVPE F0 → pitch shift → RVC v2 generator.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::assets::{self, HUBERT, RMVPE};
use crate::dsp::gate::{FRAME, Gate};
use crate::error::{EngineError, Result};
use crate::f0::{self, Rmvpe};
use crate::generator::{GenInput, Generator};
use crate::hubert::{self, ContentEncoder, DIM};
use crate::runtime::{ActiveProvider, Provider};

/// Samples per ContentVec frame; blocks are padded to a multiple of it.
const CONTENT_HOP: usize = 2 * FRAME;
/// Input sample rate of the pipeline.
const INPUT_SR: usize = 16000;

/// What [`Pipeline::new`] loads and how it warms up.
#[derive(Clone, Debug)]
pub struct PipelineConfig {
    pub model: PathBuf,
    pub sr_override: Option<u32>,
    pub provider: Provider,
    pub warmup_len: usize,
    pub warmup_context: usize,
}

/// Per-block conversion settings.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub pitch_semitones: f32,
    pub gate_db: f32,
    pub speaker_id: i64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            pitch_semitones: 0.0,
            gate_db: -50.0,
            speaker_id: 0,
        }
    }
}

/// The ContentVec, RMVPE and generator sessions, warmed up.
pub struct Pipeline {
    hubert: ContentEncoder,
    rmvpe: Rmvpe,
    generator: Generator,
    warmup: Duration,
}

impl Pipeline {
    /// Loads the base models and the generator on `cfg.provider`, then times one silent warm-up block.
    pub fn new(cfg: &PipelineConfig) -> Result<Self> {
        let hubert = ContentEncoder::load(&assets::asset(HUBERT)?, cfg.provider)?;
        let rmvpe = Rmvpe::load(&assets::asset(RMVPE)?, cfg.provider)?;
        let generator = Generator::load(&cfg.model, cfg.provider, cfg.sr_override)?;
        let mut pipeline = Self {
            hubert,
            rmvpe,
            generator,
            warmup: Duration::ZERO,
        };
        let start = Instant::now();
        pipeline.convert(
            &vec![0.0; cfg.warmup_len],
            cfg.warmup_context,
            &Params::default(),
        )?;
        pipeline.warmup = start.elapsed();
        Ok(pipeline)
    }

    /// Output sample rate of the generator.
    pub fn sample_rate(&self) -> u32 {
        self.generator.sample_rate()
    }

    /// The generator's provider; ContentVec and RMVPE fallbacks were already logged when they loaded.
    pub fn active_provider(&self) -> ActiveProvider {
        self.generator.active_provider()
    }

    pub fn warmup_time(&self) -> Duration {
        self.warmup
    }

    /// Converts a 16 kHz block with `context` samples of extra audio on each side and returns
    /// the core (`len - 2*context` input samples) at [`Pipeline::sample_rate`].
    pub fn convert(
        &mut self,
        block_16k: &[f32],
        context: usize,
        params: &Params,
    ) -> Result<Vec<f32>> {
        check_block(block_16k.len(), context)?;
        let out = self.generate(block_16k, params)?;
        Ok(trim_output(
            &out,
            context,
            block_16k.len() - 2 * context,
            self.sample_rate(),
        ))
    }

    /// Converts a 16 kHz window whose first `context` samples are past audio only and returns
    /// `window_16k[context..]` at [`Pipeline::sample_rate`].
    pub fn convert_past(
        &mut self,
        window_16k: &[f32],
        context: usize,
        params: &Params,
    ) -> Result<Vec<f32>> {
        check_past(window_16k.len(), context)?;
        let out = self.generate(window_16k, params)?;
        Ok(trim_output(
            &out,
            context,
            window_16k.len() - context,
            self.sample_rate(),
        ))
    }

    /// Runs the gate, ContentVec, RMVPE and the generator on a padded 16 kHz block and returns
    /// the full generator output.
    fn generate(&mut self, block_16k: &[f32], params: &Params) -> Result<Vec<f32>> {
        let n = padded_len(block_16k.len());
        let t = n / FRAME;
        let mut padded = block_16k.to_vec();
        padded.resize(n, 0.0);
        let gate = Gate {
            threshold_db: params.gate_db,
            ..Gate::default()
        };
        let (audio, open) = gate.apply(&padded);

        let (feats, frames) = self.hubert.encode(&audio)?;
        let phone = fit_rows(hubert::upsample2(&feats, frames), t);

        let mut pitchf = self.rmvpe.f0(&audio)?;
        pitchf.resize(t, 0.0);
        f0::apply_mask(&mut pitchf, &open);
        f0::shift(&mut pitchf, params.pitch_semitones);
        let pitch: Vec<i64> = pitchf.iter().map(|&f| f0::coarse(f)).collect();

        self.generator.run(&GenInput {
            phone: &phone,
            frames: t,
            pitch: &pitch,
            pitchf: &pitchf,
            speaker_id: params.speaker_id,
        })
    }
}

/// Truncates `[_][DIM]` rows to `t`, or pads by repeating the last row (zeros if there is none).
fn fit_rows(mut rows: Vec<f32>, t: usize) -> Vec<f32> {
    let last = rows
        .len()
        .checked_sub(DIM)
        .map_or_else(|| vec![0.0; DIM], |start| rows[start..].to_vec());
    rows.truncate(t * DIM);
    while rows.len() < t * DIM {
        rows.extend_from_slice(&last);
    }
    rows
}

/// `len` rounded up to a multiple of 320 samples (one ContentVec frame), at least 320.
pub fn padded_len(len: usize) -> usize {
    len.div_ceil(CONTENT_HOP).max(1) * CONTENT_HOP
}

/// F0 frames (10 ms) of a block of `len` samples after padding.
pub fn frames_for(len: usize) -> usize {
    padded_len(len) / FRAME
}

/// A slice of a generator output at `sr`: keeps `len` input samples' worth after skipping `skip`
/// input samples (floored to whole 10 ms frames), padded with zeros when the output is short.
/// Both counts are 16 kHz input samples.
pub fn trim_output(out: &[f32], skip: usize, len: usize, sr: u32) -> Vec<f32> {
    let sr = sr as usize;
    let hop = sr / 100;
    let skip = (skip / FRAME) * hop;
    let len = len * sr / INPUT_SR;
    let mut trimmed: Vec<f32> = out.iter().skip(skip).take(len).copied().collect();
    trimmed.resize(len, 0.0);
    trimmed
}

/// Rejects a block whose context is not frame-aligned or that leaves no core.
pub fn check_block(len: usize, context: usize) -> Result<()> {
    if !context.is_multiple_of(FRAME) {
        return Err(EngineError::Inference(format!(
            "context of {context} samples is not a multiple of {FRAME}"
        )));
    }
    if len <= 2 * context {
        return Err(EngineError::Inference(format!(
            "block of {len} samples leaves no core around {context} samples of context"
        )));
    }
    Ok(())
}

/// Rejects a past-context window whose context is not frame-aligned or that leaves no audio.
pub fn check_past(len: usize, context: usize) -> Result<()> {
    if !context.is_multiple_of(FRAME) {
        return Err(EngineError::Inference(format!(
            "context of {context} samples is not a multiple of {FRAME}"
        )));
    }
    if len <= context {
        return Err(EngineError::Inference(format!(
            "window of {len} samples leaves no audio after {context} samples of context"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_len_rounds_up() {
        assert_eq!(padded_len(1), 320);
        assert_eq!(padded_len(320), 320);
        assert_eq!(padded_len(321), 640);
    }

    #[test]
    fn padded_len_of_empty_is_one_unit() {
        assert_eq!(padded_len(0), 320);
    }

    #[test]
    fn frames_for_matches() {
        assert_eq!(frames_for(168_000), 1050);
    }

    #[test]
    fn trim_output_drops_context() {
        let out: Vec<f32> = (0..1050 * 400).map(|i| i as f32).collect();
        let got = trim_output(&out, 8000, 160_000, 40000);
        assert_eq!(got.len(), 400_000);
        assert_eq!(got[0], (50 * 400) as f32);
        assert_eq!(got[399_999], (50 * 400 + 399_999) as f32);
    }

    #[test]
    fn trim_output_pads_short() {
        let out = vec![1.0f32; 1000];
        let got = trim_output(&out, 160, 1600, 40000);
        assert_eq!(got.len(), 4000);
        assert!(got[..600].iter().all(|&v| v == 1.0));
        assert!(got[600..].iter().all(|&v| v == 0.0));
    }

    #[test]
    fn trim_output_44k_style_rounding() {
        let core = 16_001;
        let out = vec![0.5f32; 100_000];
        let got = trim_output(&out, 0, core, 40000);
        assert_eq!(got.len(), core * 40000 / 16000);
    }

    #[test]
    fn convert_rejects_misaligned_context() {
        assert!(matches!(
            check_block(32_000, 100),
            Err(EngineError::Inference(_))
        ));
    }

    #[test]
    fn fit_rows_truncates_and_repeats_last_row() {
        let rows: Vec<f32> = (0..3 * DIM).map(|i| (i / DIM) as f32).collect();
        assert_eq!(fit_rows(rows.clone(), 2), rows[..2 * DIM]);
        let padded = fit_rows(rows, 5);
        assert_eq!(padded.len(), 5 * DIM);
        assert!(padded[3 * DIM..].iter().all(|&v| v == 2.0));
        assert_eq!(fit_rows(Vec::new(), 1), vec![0.0; DIM]);
    }

    #[test]
    fn convert_rejects_block_without_core() {
        assert!(matches!(
            check_block(3200, 1600),
            Err(EngineError::Inference(_))
        ));
        assert!(check_block(3201, 1600).is_ok());
        assert!(check_block(320, 0).is_ok());
    }

    #[test]
    fn check_past_rules() {
        assert!(matches!(
            check_past(9600, 100),
            Err(EngineError::Inference(_))
        ));
        assert!(matches!(
            check_past(8000, 8000),
            Err(EngineError::Inference(_))
        ));
        assert!(check_past(8001, 8000).is_ok());
        assert!(check_past(320, 0).is_ok());
    }

    #[test]
    fn trim_output_past_keeps_tail() {
        let out: Vec<f32> = (0..60 * 400).map(|i| i as f32).collect();
        let got = trim_output(&out, 8000, 1600, 40000);
        assert_eq!(got.len(), 4000);
        assert_eq!(got[0], (50 * 400) as f32);
    }
}
