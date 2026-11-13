//! Streams a WAV file through [`Streamer`] as fast as possible and reports per-block inference times.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::dsp::resample::resample;
use crate::error::Result;
use crate::offline::{ConvertOptions, read_wav_mono, write_wav_f32};
use crate::pipeline::{Pipeline, PipelineConfig};
use crate::runtime::ActiveProvider;
use crate::stream::{Controls, StreamSettings, StreamStats, Streamer};

/// Timings of one [`stream_file`] run.
#[derive(Clone, Debug)]
pub struct StreamReport {
    pub active_provider: ActiveProvider,
    pub warmup: Duration,
    pub block: Duration,
    /// One entry per converted block, sorted.
    pub inference: Vec<Duration>,
}

impl StreamReport {
    pub fn p50(&self) -> Duration {
        percentile(&self.inference, 0.5)
    }

    pub fn p95(&self) -> Duration {
        percentile(&self.inference, 0.95)
    }

    pub fn max(&self) -> Duration {
        self.inference.last().copied().unwrap_or(Duration::ZERO)
    }

    /// p95 inference time per block length.
    pub fn p95_ratio(&self) -> f64 {
        self.p95().as_secs_f64() / self.block.as_secs_f64()
    }
}

/// Nearest rank: `sorted[ceil(p·n) - 1]` (clamped); [`Duration::ZERO`] when empty.
pub fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Converts `input` to `output` through a [`Streamer`] fed in 20 ms chunks without pacing, and
/// times every block. The output is aligned to the input by dropping the streamer's delay.
pub fn stream_file(
    input: &Path,
    output: &Path,
    opts: &ConvertOptions,
    settings: StreamSettings,
) -> Result<StreamReport> {
    let (wav, wav_rate) = read_wav_mono(input)?;
    let settings = settings.clamped();
    let pipeline = Pipeline::new(&PipelineConfig {
        model: opts.model.clone(),
        sr_override: opts.sr_override,
        provider: opts.provider,
        warmup_len: settings.window_16k(),
        warmup_context: 0,
    })?;
    let active_provider = pipeline.active_provider();
    let warmup = pipeline.warmup_time();
    let sr = pipeline.sample_rate();

    let controls = Arc::new(Controls::new());
    controls.set_pitch(opts.params.pitch_semitones);
    controls.set_gate_db(opts.params.gate_db);
    let stats = Arc::new(StreamStats::new());
    let mut streamer = Streamer::new(pipeline, settings, controls, stats.clone());
    streamer.prepare(wav_rate)?;

    let mut converted = Vec::new();
    let mut inference = Vec::new();
    for chunk in wav.chunks((wav_rate / 50).max(1) as usize) {
        let before = converted.len();
        let start = Instant::now();
        streamer.process(chunk, &mut converted)?;
        if converted.len() > before {
            inference.push(start.elapsed());
        }
    }
    inference.sort();

    let skip = (streamer.delay().as_secs_f64() * f64::from(sr)).round() as usize;
    converted.drain(..skip.min(converted.len()));
    converted.resize(
        (wav.len() as u64 * u64::from(sr) / u64::from(wav_rate)) as usize,
        0.0,
    );
    let out_rate = opts.out_rate.unwrap_or(wav_rate);
    write_wav_f32(output, &resample(&converted, sr, out_rate)?, out_rate)?;
    Ok(StreamReport {
        active_provider,
        warmup,
        block: stats.block(),
        inference,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_nearest_rank() {
        let sorted: Vec<Duration> = (1..=10).map(Duration::from_millis).collect();
        assert_eq!(percentile(&sorted, 0.5), Duration::from_millis(5));
        assert_eq!(percentile(&sorted, 0.95), Duration::from_millis(10));
        assert_eq!(percentile(&sorted, 1.0), Duration::from_millis(10));
        assert_eq!(percentile(&[], 0.5), Duration::ZERO);
    }
}
