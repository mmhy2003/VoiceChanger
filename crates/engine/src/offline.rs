//! Offline conversion: reads a WAV file, converts it in blocks with context and writes a WAV.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::dsp::resample::resample;
use crate::error::{EngineError, Result};
use crate::pipeline::{Params, Pipeline, PipelineConfig};
use crate::runtime::{ActiveProvider, Provider};

/// Sample rate the pipeline consumes.
const PIPELINE_SR: u32 = 16000;
/// Blocks and context are aligned to one ContentVec frame.
const ALIGN: usize = 320;

/// What [`convert_file`] loads and how it slices the input.
#[derive(Clone, Debug)]
pub struct ConvertOptions {
    pub model: PathBuf,
    pub sr_override: Option<u32>,
    pub provider: Provider,
    pub params: Params,
    pub out_rate: Option<u32>,
    pub block_secs: f32,
    pub context_secs: f32,
}

impl ConvertOptions {
    /// Defaults: the default provider and params, the input rate for output, 10 s blocks with 0.5 s of context.
    pub fn new(model: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            sr_override: None,
            provider: Provider::default(),
            params: Params::default(),
            out_rate: None,
            block_secs: 10.0,
            context_secs: 0.5,
        }
    }
}

/// Timings of one [`convert_file`] run.
#[derive(Clone, Debug)]
pub struct ConvertReport {
    pub active_provider: ActiveProvider,
    pub warmup: Duration,
    pub audio_secs: f64,
    pub convert_secs: f64,
}

impl ConvertReport {
    /// Conversion time per second of audio; below 1 is faster than real time.
    pub fn real_time_factor(&self) -> f64 {
        self.convert_secs / self.audio_secs
    }
}

/// `samples` rounded to the nearest multiple of `step`, at least `step`.
pub fn align(samples: usize, step: usize) -> usize {
    ((samples + step / 2) / step).max(1) * step
}

/// Block ranges in the padded signal: `context` zeros, the `total` samples, then zeros up to
/// `n·block + 2·context`. Each range covers one `block` core plus `context` on each side.
pub fn plan_blocks(total: usize, block: usize, context: usize) -> Vec<Range<usize>> {
    let n = total.div_ceil(block).max(1);
    (0..n)
        .map(|i| i * block..i * block + block + 2 * context)
        .collect()
}

fn wav_err(path: &Path, e: impl std::fmt::Display) -> EngineError {
    EngineError::Wav(format!("{}: {e}", path.display()))
}

/// Reads a float 32 or int 8/16/24/32 WAV file, averaged to mono, with its sample rate.
pub fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)> {
    let mut reader = hound::WavReader::open(path).map_err(|e| wav_err(path, e))?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| wav_err(path, e))?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f64;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| (f64::from(v) * scale) as f32))
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| wav_err(path, e))?
        }
    };
    let channels = usize::from(spec.channels.max(1));
    let mono = interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    Ok((mono, spec.sample_rate))
}

/// Writes `samples` as a 32-bit float mono WAV file.
pub fn write_wav_f32(path: &Path, samples: &[f32], rate: u32) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| wav_err(path, e))?;
    for &s in samples {
        writer.write_sample(s).map_err(|e| wav_err(path, e))?;
    }
    writer.finalize().map_err(|e| wav_err(path, e))
}

/// Converts `input` to `output` block by block through a warmed-up [`Pipeline`].
pub fn convert_file(input: &Path, output: &Path, opts: &ConvertOptions) -> Result<ConvertReport> {
    let (wav, in_rate) = read_wav_mono(input)?;
    let audio_secs = wav.len() as f64 / f64::from(in_rate);
    let signal = resample(&wav, in_rate, PIPELINE_SR)?;
    let total = signal.len();

    let block = align(secs_to_samples(opts.block_secs), ALIGN);
    let context = align(secs_to_samples(opts.context_secs), ALIGN);
    let mut pipeline = Pipeline::new(&PipelineConfig {
        model: opts.model.clone(),
        sr_override: opts.sr_override,
        provider: opts.provider,
        warmup_len: block + 2 * context,
        warmup_context: context,
    })?;
    let sr = pipeline.sample_rate();

    let ranges = plan_blocks(total, block, context);
    let mut padded = vec![0.0; context];
    padded.extend_from_slice(&signal);
    padded.resize(ranges.len() * block + 2 * context, 0.0);

    let start = Instant::now();
    let mut converted = Vec::new();
    for range in ranges {
        converted.extend(pipeline.convert(&padded[range], context, &opts.params)?);
    }
    let convert_secs = start.elapsed().as_secs_f64();
    converted.truncate((total as u64 * u64::from(sr) / u64::from(PIPELINE_SR)) as usize);

    let out_rate = opts.out_rate.unwrap_or(in_rate);
    write_wav_f32(output, &resample(&converted, sr, out_rate)?, out_rate)?;
    Ok(ConvertReport {
        active_provider: pipeline.active_provider(),
        warmup: pipeline.warmup_time(),
        audio_secs,
        convert_secs,
    })
}

/// Seconds at 16 kHz to whole samples; negative values give 0.
fn secs_to_samples(secs: f32) -> usize {
    (f64::from(secs) * f64::from(PIPELINE_SR)).round().max(0.0) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::EngineError;
    use crate::testutil::temp_path;

    #[test]
    fn plan_one_short_block() {
        assert_eq!(plan_blocks(100, 320, 160), vec![0..640]);
    }

    #[test]
    fn plan_exact_multiple() {
        assert_eq!(plan_blocks(640, 320, 160), vec![0..640, 320..960]);
    }

    #[test]
    fn plan_covers_all() {
        let (total, block, context) = (16_000 * 25 + 7, 160_000, 8000);
        let ranges = plan_blocks(total, block, context);
        assert_eq!(ranges.len(), 3);
        let padded = ranges.len() * block + 2 * context;
        assert_eq!(ranges.last().unwrap().end, padded);
        assert!(padded >= total + 2 * context);
        assert!(ranges.windows(2).all(|w| w[1].start - w[0].start == block));
    }

    #[test]
    fn plan_empty_signal_has_one_block() {
        assert_eq!(plan_blocks(0, 320, 0), vec![0..320]);
    }

    #[test]
    fn align_rounds() {
        assert_eq!(align(160_000, 320), 160_000);
        assert_eq!(align(8000, 320), 8000);
        assert_eq!(align(479, 320), 320);
        assert_eq!(align(481, 320), 640);
        assert_eq!(align(0, 320), 320);
    }

    #[test]
    fn wav_roundtrip_f32() {
        let path = temp_path("roundtrip.wav");
        let x: Vec<f32> = (0..1000).map(|i| (i as f32 / 1000.0) - 0.5).collect();
        write_wav_f32(&path, &x, 40_000).unwrap();
        let (y, rate) = read_wav_mono(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(rate, 40_000);
        assert_eq!(y, x);
    }

    #[test]
    fn wav_int16_stereo_downmix() {
        let path = temp_path("stereo16.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..100 {
            w.write_sample(16_384i16).unwrap();
            w.write_sample(-16_384i16).unwrap();
        }
        w.finalize().unwrap();
        let (y, rate) = read_wav_mono(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(y.len(), 100);
        assert!(y.iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn wav_int16_scaled_to_unit_range() {
        let path = temp_path("mono16.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        w.write_sample(16_384i16).unwrap();
        w.write_sample(i16::MIN).unwrap();
        w.finalize().unwrap();
        let (y, _) = read_wav_mono(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(y, vec![0.5, -1.0]);
    }

    #[test]
    fn wav_missing_file_is_wav_error() {
        let err = read_wav_mono(&temp_path("does-not-exist.wav")).unwrap_err();
        assert!(matches!(err, EngineError::Wav(_)), "{err:?}");
    }
}
