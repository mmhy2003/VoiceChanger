//! Processing worker: downmix to mono, resample per output, monitor gain and level meters.
//!
//! Every buffer is allocated in `State::new` or before the loop in `spawn`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use ringbuf::traits::{Consumer, Observer};
use ringbuf::{HeapCons, HeapProd};
use rubato::{FftFixedIn, Resampler};

use crate::meter::measure;
use crate::ring::push_output;
use crate::shared::Shared;

/// Average the channels of an interleaved buffer into `out` (cleared first). channels >= 1.
/// A trailing partial frame is ignored.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    let channels = channels.max(1);
    if channels == 1 {
        out.extend_from_slice(interleaved);
        return;
    }
    let scale = 1.0 / channels as f32;
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() * scale),
    );
}

/// What an output device is used for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind {
    /// The virtual cable: full level, drives the output meter.
    Cable,
    /// The monitor: scaled by the shared monitor gain.
    Monitor,
}

/// One output of the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputConfig {
    /// What the output is used for.
    pub kind: OutputKind,
    /// Sample rate of the output device.
    pub rate: u32,
}

/// Input format and outputs of the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    /// Sample rate of the input device.
    pub in_rate: u32,
    /// Channel count of the interleaved input.
    pub in_channels: usize,
    /// Outputs, in the order of `State::output` indices.
    pub outputs: Vec<OutputConfig>,
}

struct OutputPath {
    kind: OutputKind,
    resampler: Option<FftFixedIn<f32>>, // None when rate == in_rate
    pending: Vec<f32>,                  // mono input waiting for a full resampler chunk
    chunk_out: Vec<Vec<f32>>,           // [1][output_frames_max()]
    ready: Vec<f32>,                    // processed mono samples from the last process_block
}

/// Worker state: preallocated buffers and one resampler per output that needs one.
pub struct State {
    in_rate: usize,
    in_channels: usize,
    mono: Vec<f32>,
    paths: Vec<OutputPath>,
}

impl State {
    /// All buffers preallocated (capacities: mono/pending = in_rate, ready = out rate).
    /// Construction errors are returned as their Display string.
    pub fn new(cfg: &WorkerConfig) -> Result<State, String> {
        let in_rate = cfg.in_rate as usize;
        let paths = cfg
            .outputs
            .iter()
            .map(|out| {
                let resampler = if out.rate == cfg.in_rate {
                    None
                } else {
                    Some(
                        FftFixedIn::<f32>::new(in_rate, out.rate as usize, in_rate / 100, 1, 1)
                            .map_err(|e| e.to_string())?,
                    )
                };
                let chunk_out = resampler
                    .as_ref()
                    .map_or_else(Vec::new, |r| r.output_buffer_allocate(true));
                Ok(OutputPath {
                    kind: out.kind,
                    resampler,
                    pending: Vec::with_capacity(in_rate),
                    chunk_out,
                    ready: Vec::with_capacity(out.rate as usize),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(State {
            in_rate,
            in_channels: cfg.in_channels.max(1),
            mono: Vec::with_capacity(in_rate),
            paths,
        })
    }

    /// Processed samples of output `index` (same order as the config's outputs) from the last
    /// `process_block`.
    pub fn output(&self, index: usize) -> &[f32] {
        &self.paths[index].ready
    }

    /// Number of outputs.
    pub fn outputs(&self) -> usize {
        self.paths.len()
    }
}

/// Processes one interleaved input block into every output's `ready` buffer and updates the
/// input and output meters.
pub fn process_block(state: &mut State, interleaved: &[f32], shared: &Shared) {
    downmix(interleaved, state.in_channels, &mut state.mono);
    shared.input_meter.store(measure(&state.mono));

    for path in &mut state.paths {
        path.ready.clear();
        path.pending.extend_from_slice(&state.mono);
        match &mut path.resampler {
            None => {
                path.ready.extend_from_slice(&path.pending);
                path.pending.clear();
            }
            Some(r) => {
                while path.pending.len() >= r.input_frames_next() {
                    let n = r.input_frames_next();
                    // Cannot fail: chunk_out comes from the resampler and n from input_frames_next().
                    let (_, out_n) = r
                        .process_into_buffer(&[&path.pending[..n]], &mut path.chunk_out, None)
                        .expect("buffers sized from the resampler");
                    path.ready.extend_from_slice(&path.chunk_out[0][..out_n]);
                    path.pending.drain(..n);
                }
            }
        }
        if path.kind == OutputKind::Monitor {
            let gain = shared.monitor_gain();
            path.ready.iter_mut().for_each(|s| *s *= gain);
        }
    }

    if let Some(cable) = state.paths.iter().find(|p| p.kind == OutputKind::Cable)
        && !cable.ready.is_empty()
    {
        shared.output_meter.store(measure(&cable.ready));
    }
}

/// Spawns thread "vc-audio-worker". `outputs[i]` feeds `state.output(i)`; `max_backlogs[i]` is
/// that output's backlog limit in samples. Runs until `stop` is true.
pub fn spawn(
    mut state: State,
    mut input: HeapCons<f32>,
    mut outputs: Vec<HeapProd<f32>>,
    max_backlogs: Vec<usize>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("vc-audio-worker".into())
        .spawn(move || {
            let ch = state.in_channels;
            let mut scratch = vec![0.0f32; ch * (state.in_rate / 50).max(1)];
            while !stop.load(Relaxed) {
                let take = input.occupied_len().min(scratch.len()) / ch * ch;
                if take == 0 {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                input.pop_slice(&mut scratch[..take]);
                process_block(&mut state, &scratch[..take], &shared);
                for (i, (out, &max_backlog)) in outputs.iter_mut().zip(&max_backlogs).enumerate() {
                    push_output(out, state.output(i), max_backlog, &shared);
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    use std::time::{Duration, Instant};

    use ringbuf::traits::{Consumer, Observer, Producer};

    use crate::meter::measure;
    use crate::ring::ring;
    use crate::shared::Shared;

    fn cfg(in_rate: u32, in_channels: usize, outputs: &[(OutputKind, u32)]) -> WorkerConfig {
        WorkerConfig {
            in_rate,
            in_channels,
            outputs: outputs
                .iter()
                .map(|&(kind, rate)| OutputConfig { kind, rate })
                .collect(),
        }
    }

    fn sine(rate: u32, freq: f32, amp: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    /// Feeds `mono` (duplicated to `channels`) in blocks of `block` frames and collects every output.
    fn run(state: &mut State, mono: &[f32], channels: usize, block: usize) -> Vec<Vec<f32>> {
        let shared = Shared::new();
        let mut collected = vec![Vec::new(); state.outputs()];
        let mut interleaved = Vec::new();
        for chunk in mono.chunks(block) {
            interleaved.clear();
            for &s in chunk {
                interleaved.extend(std::iter::repeat_n(s, channels));
            }
            process_block(state, &interleaved, &shared);
            for (i, out) in collected.iter_mut().enumerate() {
                out.extend_from_slice(state.output(i));
            }
        }
        collected
    }

    #[test]
    fn downmix_stereo_averages() {
        let mut out = vec![9.0];
        downmix(&[1.0, 0.0, 0.5, 0.5], 2, &mut out);
        assert_eq!(out, [0.5, 0.5]);
    }

    #[test]
    fn downmix_mono_is_copy() {
        let mut out = Vec::new();
        downmix(&[0.1, -0.2, 0.3], 1, &mut out);
        assert_eq!(out, [0.1, -0.2, 0.3]);
    }

    #[test]
    fn downmix_ignores_partial_frame() {
        let mut out = Vec::new();
        downmix(&[0.2, 0.4, 0.6], 2, &mut out);
        assert_eq!(out.len(), 1);
        assert!((out[0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn same_rate_passthrough() {
        let mut state = State::new(&cfg(48_000, 1, &[(OutputKind::Cable, 48_000)])).unwrap();
        let shared = Shared::new();
        let input = sine(48_000, 440.0, 0.7, 480);
        process_block(&mut state, &input, &shared);
        assert_eq!(state.output(0), &input[..]);
        assert_eq!(shared.input_meter.load().peak, measure(&input).peak);
    }

    #[test]
    fn resample_44k1_to_48k_length_and_sine() {
        let mut state = State::new(&cfg(44_100, 2, &[(OutputKind::Cable, 48_000)])).unwrap();
        let input = sine(44_100, 1000.0, 0.5, 44_100);
        let out = &run(&mut state, &input, 2, 441)[0];

        assert!(out.len().abs_diff(48_000) <= 960, "len {}", out.len());

        let tail = &out[4800..];
        let crossings = tail
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        let freq = crossings as f32 / (tail.len() as f32 / 48_000.0);
        assert!((freq - 1000.0).abs() <= 20.0, "freq {freq}");
        let peak = measure(tail).peak;
        assert!((peak - 0.5).abs() <= 0.05, "peak {peak}");
    }

    #[test]
    fn monitor_gain_applied() {
        let mut state = State::new(&cfg(
            48_000,
            1,
            &[(OutputKind::Cable, 48_000), (OutputKind::Monitor, 48_000)],
        ))
        .unwrap();
        let shared = Shared::new();
        shared.set_monitor_gain(0.5);
        process_block(&mut state, &[0.8; 480], &shared);
        assert_eq!(state.output(0), &[0.8; 480][..]);
        assert_eq!(state.output(1), &[0.4; 480][..]);
        assert_eq!(shared.output_meter.load().peak, 0.8);
    }

    #[test]
    fn two_output_rates() {
        let mut state = State::new(&cfg(
            44_100,
            1,
            &[(OutputKind::Cable, 48_000), (OutputKind::Monitor, 44_100)],
        ))
        .unwrap();
        let input = sine(44_100, 1000.0, 0.5, 44_100);
        let out = run(&mut state, &input, 1, 441);
        assert!(out[0].len().abs_diff(48_000) <= 960, "len {}", out[0].len());
        assert_eq!(out[1].len(), 44_100);
    }

    #[test]
    fn worker_thread_moves_samples_and_stops() {
        let state = State::new(&cfg(48_000, 1, &[(OutputKind::Cable, 48_000)])).unwrap();
        let (mut in_prod, in_cons) = ring(48_000);
        let (out_prod, mut out_cons) = ring(48_000);
        let input = sine(48_000, 440.0, 0.5, 4800);
        assert_eq!(in_prod.push_slice(&input), input.len());

        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(
            state,
            in_cons,
            vec![out_prod],
            vec![48_000 / 5],
            shared,
            stop.clone(),
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        while out_cons.occupied_len() < input.len() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Relaxed);
        handle.join().unwrap();

        let got: Vec<f32> = out_cons.pop_iter().collect();
        assert_eq!(got, input);
    }
}
