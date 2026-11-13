//! Block-wise real-time conversion: resamples the mic to 16 kHz, keeps a past-context history, converts
//! one block at a time and SOLA-crossfades the result at the model rate.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::dsp::resample::StreamResampler;
use crate::error::{EngineError, Result};
use crate::pipeline::{Params, Pipeline};
use crate::stream::{Controls, Sola, StreamSettings, StreamStats};

/// A converter of 16 kHz windows whose first `context` samples are past audio only.
pub trait Convert {
    /// Sample rate of the converted output.
    fn sample_rate(&self) -> u32;
    /// Returns `window[context..]` converted, at [`Convert::sample_rate`].
    fn convert_past(&mut self, window: &[f32], context: usize, params: &Params)
    -> Result<Vec<f32>>;
}

impl Convert for Pipeline {
    fn sample_rate(&self) -> u32 {
        Pipeline::sample_rate(self)
    }

    fn convert_past(
        &mut self,
        window: &[f32],
        context: usize,
        params: &Params,
    ) -> Result<Vec<f32>> {
        Pipeline::convert_past(self, window, context, params)
    }
}

/// Converts a live mono stream block by block. At most one conversion runs per `process` call.
pub struct Streamer<C: Convert> {
    inner: C,
    /// Clamped.
    settings: StreamSettings,
    controls: Arc<Controls>,
    stats: Arc<StreamStats>,
    resampler: Option<StreamResampler>,
    resampled: Vec<f32>,
    /// 16 kHz input not yet converted.
    fresh: Vec<f32>,
    /// The last `window_16k` input samples at 16 kHz.
    history: Vec<f32>,
    sola: Sola,
}

/// Duration of `n` samples at 16 kHz (62.5 µs each).
fn duration_16k(n: usize) -> Duration {
    Duration::from_nanos(n as u64 * 62_500)
}

impl<C: Convert> Streamer<C> {
    pub fn new(
        inner: C,
        settings: StreamSettings,
        controls: Arc<Controls>,
        stats: Arc<StreamStats>,
    ) -> Self {
        let settings = settings.clamped();
        let sr = inner.sample_rate() as usize;
        let m = |n: usize| n * sr / 16_000;
        let sola = Sola::new(
            m(settings.block_16k()),
            m(settings.crossfade_16k()),
            m(settings.search_16k()),
        );
        Self {
            inner,
            settings,
            controls,
            stats,
            resampler: None,
            resampled: Vec::new(),
            fresh: Vec::with_capacity(2 * settings.block_16k()),
            history: vec![0.0; settings.window_16k()],
            sola,
        }
    }

    pub fn settings(&self) -> StreamSettings {
        self.settings
    }

    /// Sample rate of the samples `process` appends.
    pub fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    /// Sets up for input at `in_rate` Hz, forgets past audio, warms up and resets the stats.
    pub fn prepare(&mut self, in_rate: u32) -> Result<()> {
        self.resampler = if in_rate == 16_000 {
            None
        } else {
            Some(StreamResampler::new(in_rate, 16_000)?)
        };
        self.fresh.clear();
        self.history.fill(0.0);
        self.sola.reset();
        self.warm_up()?;
        self.stats.reset();
        self.stats
            .set_block(duration_16k(self.settings.block_16k()));
        Ok(())
    }

    /// Runs one conversion of a silent window and discards it.
    pub fn warm_up(&mut self) -> Result<()> {
        let window = vec![0.0; self.settings.window_16k()];
        self.inner
            .convert_past(&window, self.settings.context_16k(), &Params::default())?;
        Ok(())
    }

    /// Algorithmic delay: block + crossfade + search + input resampler delay. Valid after `prepare`.
    pub fn delay(&self) -> Duration {
        let s = &self.settings;
        let resampler = self.resampler.as_ref().map_or(0, |r| r.output_delay());
        duration_16k(s.block_16k() + s.crossfade_16k() + s.search_16k() + resampler)
    }

    /// Appends converted samples (one block, or none) for `input` (mono, at the `prepare` rate) to `out`.
    /// When a whole extra block is waiting, the oldest one is dropped and counted as late.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        match &mut self.resampler {
            Some(r) => {
                self.resampled.clear();
                r.push(input, &mut self.resampled)?;
                self.fresh.extend_from_slice(&self.resampled);
            }
            None => self.fresh.extend_from_slice(input),
        }
        let block = self.settings.block_16k();
        while self.fresh.len() >= 2 * block {
            self.fresh.drain(..block);
            self.stats.record_late();
        }
        if self.fresh.len() < block {
            return Ok(());
        }
        self.history.copy_within(block.., 0);
        let tail = self.history.len() - block;
        self.history[tail..].copy_from_slice(&self.fresh[..block]);
        self.fresh.drain(..block);

        let params = self.controls.params();
        let start = Instant::now();
        let conv = self
            .inner
            .convert_past(&self.history, self.settings.context_16k(), &params)?;
        self.stats.record(start.elapsed());
        if conv.len() != self.sola.input_len() {
            return Err(EngineError::Inference(format!(
                "converter returned {} samples, expected {}",
                conv.len(),
                self.sola.input_len()
            )));
        }
        self.sola.process(&conv, out);
        Ok(())
    }

    pub fn into_inner(self) -> C {
        self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Upsamples 16 kHz to 40 kHz by sample-and-hold and records every call.
    struct FakeConvert {
        rate: u32,
        calls: Vec<(usize, usize, Params)>,
    }

    impl FakeConvert {
        fn new() -> Self {
            Self {
                rate: 40_000,
                calls: Vec::new(),
            }
        }
    }

    impl Convert for FakeConvert {
        fn sample_rate(&self) -> u32 {
            self.rate
        }

        fn convert_past(
            &mut self,
            window: &[f32],
            context: usize,
            params: &Params,
        ) -> Result<Vec<f32>> {
            self.calls.push((window.len(), context, *params));
            let n = (window.len() - context) * 5 / 2;
            Ok((0..n).map(|i| window[context + i * 2 / 5]).collect())
        }
    }

    fn streamer() -> (Streamer<FakeConvert>, Arc<Controls>, Arc<StreamStats>) {
        let controls = Arc::new(Controls::new());
        let stats = Arc::new(StreamStats::new());
        let s = Streamer::new(
            FakeConvert::new(),
            StreamSettings::default(),
            controls.clone(),
            stats.clone(),
        );
        (s, controls, stats)
    }

    #[test]
    fn output_length_and_rate() {
        let (mut s, _, _) = streamer();
        s.prepare(16_000).unwrap();
        let sine: Vec<f32> = (0..16_000)
            .map(|i| (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 16_000.0).sin() * 0.5)
            .collect();
        let mut out = Vec::new();
        for chunk in sine.chunks(320) {
            s.process(chunk, &mut out).unwrap();
        }
        assert_eq!(out.len(), 4 * 10_000);
        assert_eq!(s.sample_rate(), 40_000);
        let settings = s.settings();
        let fake = s.into_inner();
        assert_eq!(fake.calls.len(), 1 + 4);
        for &(window, ctx, _) in &fake.calls[1..] {
            assert_eq!(window, settings.window_16k());
            assert_eq!(ctx, settings.context_16k());
        }
    }

    #[test]
    fn delay_at_16k_is_block_crossfade_search() {
        let (mut s, _, _) = streamer();
        s.prepare(16_000).unwrap();
        assert_eq!(s.delay(), Duration::from_millis(310));
    }

    #[test]
    fn delay_includes_input_resampler() {
        let (mut s, _, _) = streamer();
        s.prepare(48_000).unwrap();
        assert!(s.delay() > Duration::from_millis(310), "{:?}", s.delay());
    }

    #[test]
    fn drops_late_block() {
        let (mut s, _, stats) = streamer();
        s.prepare(16_000).unwrap();
        let block = s.settings().block_16k();
        let mut out = Vec::new();
        s.process(&vec![0.1; 2 * block], &mut out).unwrap();
        assert_eq!(out.len(), 10_000);
        assert_eq!(stats.late_blocks(), 1);
        assert_eq!(stats.blocks(), 1);
    }

    #[test]
    fn reads_controls_per_block() {
        let (mut s, controls, _) = streamer();
        s.prepare(16_000).unwrap();
        controls.set_pitch(5.0);
        controls.set_gate_db(-40.0);
        let block = s.settings().block_16k();
        let mut out = Vec::new();
        s.process(&vec![0.1; block], &mut out).unwrap();
        let fake = s.into_inner();
        let (_, _, params) = fake.calls.last().copied().unwrap();
        assert_eq!(fake.calls.len(), 2);
        assert_eq!(params.pitch_semitones, 5.0);
        assert_eq!(params.gate_db, -40.0);
    }

    #[test]
    fn prepare_warms_up_and_resets() {
        let (mut s, _, stats) = streamer();
        stats.record(Duration::from_millis(5));
        s.prepare(16_000).unwrap();
        assert_eq!(stats.blocks(), 0);
        assert_eq!(stats.block(), Duration::from_millis(250));
        let fake = s.into_inner();
        assert_eq!(fake.calls.len(), 1);
    }

    #[test]
    fn into_inner_returns_converter() {
        let (s, _, _) = streamer();
        let fake = s.into_inner();
        assert_eq!(fake.rate, 40_000);
        assert!(fake.calls.is_empty());
    }
}
