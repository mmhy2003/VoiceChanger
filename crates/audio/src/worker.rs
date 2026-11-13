//! Processing worker: downmix to mono, gains, the processor, resample per output, monitor gain
//! and level meters.
//!
//! Every buffer is allocated in `State::new` or before the loop in `spawn`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ringbuf::traits::{Consumer, Observer};
use ringbuf::{HeapCons, HeapProd};
use rubato::{FftFixedIn, Resampler};

use crate::meter::measure;
use crate::processor::Processor;
use crate::ring::push_block;
use crate::shared::{Shared, StreamStatus};

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

/// Input format, processor rate and outputs of the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    /// Sample rate of the input device.
    pub in_rate: u32,
    /// Channel count of the interleaved input.
    pub in_channels: usize,
    /// Sample rate of the processor's output (`Processor::out_rate`).
    pub proc_rate: u32,
    /// Outputs, in the order of `State::output` indices.
    pub outputs: Vec<OutputConfig>,
}

/// Zeros pushed into each output ring before its first block, so the callback has a cushion.
pub const PRIME_MS: u32 = 40;
/// Backlog allowed on top of the block being pushed (and the prime) before a block is dropped.
pub const SLACK_MS: u32 = 100;

/// The terms of the end-to-end latency (see `end_to_end`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatencyParts {
    /// Input frames still queued in the input ring.
    pub input_backlog: usize,
    /// Input device rate.
    pub in_rate: u32,
    /// Algorithmic delay: the processor's plus the cable resampler's.
    pub delay: Duration,
    /// Time of the last `Processor::process` call.
    pub inference: Duration,
    /// Samples queued in the cable ring after the push.
    pub cable_backlog: usize,
    /// Cable device rate.
    pub cable_rate: u32,
    /// Cable device latency (playback minus callback time).
    pub device: Duration,
}

fn frames_to_duration(frames: usize, rate: u32) -> Duration {
    Duration::from_nanos(frames as u64 * 1_000_000_000 / u64::from(rate.max(1)))
}

/// input_backlog/in_rate + delay + inference + cable_backlog/cable_rate + device.
pub fn end_to_end(p: &LatencyParts) -> Duration {
    frames_to_duration(p.input_backlog, p.in_rate)
        + p.delay
        + p.inference
        + frames_to_duration(p.cable_backlog, p.cable_rate)
        + p.device
}

struct OutputPath {
    kind: OutputKind,
    rate: u32,
    primed: bool,                       // the ring got its PRIME_MS of zeros
    resampler: Option<FftFixedIn<f32>>, // None when rate == proc_rate
    pending: Vec<f32>,                  // processed samples waiting for a full resampler chunk
    chunk_out: Vec<Vec<f32>>,           // [1][output_frames_max()]
    ready: Vec<f32>,                    // output samples from the last process_block
}

/// Worker state: the processor, preallocated buffers and one resampler per output that needs one.
pub struct State {
    in_rate: usize,
    in_channels: usize,
    mono: Vec<f32>,
    processed: Vec<f32>,
    processor: Box<dyn Processor>,
    failed: bool,
    last_process: Duration,
    paths: Vec<OutputPath>,
}

impl State {
    /// All buffers preallocated (capacities: mono = in_rate, processed/pending = 2 * proc_rate,
    /// ready = 2 * out rate). Outputs are resampled from `proc_rate`. On a construction error the
    /// processor comes back with the error's Display string.
    pub fn new(
        cfg: &WorkerConfig,
        processor: Box<dyn Processor>,
    ) -> Result<State, (String, Box<dyn Processor>)> {
        let in_rate = cfg.in_rate as usize;
        let proc_rate = cfg.proc_rate as usize;
        let paths = cfg
            .outputs
            .iter()
            .map(|out| {
                let resampler = if out.rate == cfg.proc_rate {
                    None
                } else {
                    Some(
                        FftFixedIn::<f32>::new(proc_rate, out.rate as usize, proc_rate / 100, 1, 1)
                            .map_err(|e| e.to_string())?,
                    )
                };
                let chunk_out = resampler
                    .as_ref()
                    .map_or_else(Vec::new, |r| r.output_buffer_allocate(true));
                Ok(OutputPath {
                    kind: out.kind,
                    rate: out.rate,
                    primed: false,
                    resampler,
                    pending: Vec::with_capacity(2 * proc_rate),
                    chunk_out,
                    ready: Vec::with_capacity(2 * out.rate as usize),
                })
            })
            .collect::<Result<Vec<_>, String>>();
        let paths = match paths {
            Ok(paths) => paths,
            Err(e) => return Err((e, processor)),
        };
        Ok(State {
            in_rate,
            in_channels: cfg.in_channels.max(1),
            mono: Vec::with_capacity(in_rate),
            processed: Vec::with_capacity(2 * proc_rate),
            processor,
            failed: false,
            last_process: Duration::ZERO,
            paths,
        })
    }

    /// Prepares the processor for the input rate; a failure is handled like a `process` error.
    pub fn prepare(&mut self, shared: &Shared) {
        if let Err(e) = self.processor.prepare(self.in_rate as u32) {
            self.fail(shared, e);
        }
    }

    /// Stops calling the processor, logs why and reports `ProcessorFailed`.
    fn fail(&mut self, shared: &Shared, e: String) {
        tracing::error!(reason = %e, "voice processor failed");
        shared.report(StreamStatus::ProcessorFailed);
        self.failed = true;
    }

    /// Gives the processor back (the worker thread returns it when it stops).
    pub fn into_processor(self) -> Box<dyn Processor> {
        self.processor
    }

    /// How long the last `Processor::process` call took.
    pub fn last_process(&self) -> Duration {
        self.last_process
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

    /// Index of the cable output, if there is one.
    pub fn cable_index(&self) -> Option<usize> {
        self.paths.iter().position(|p| p.kind == OutputKind::Cable)
    }

    /// Algorithmic delay of output `index`: the processor's delay plus its resampler's.
    pub fn path_delay(&self, index: usize) -> Duration {
        let path = &self.paths[index];
        let resampler = path.resampler.as_ref().map_or(Duration::ZERO, |r| {
            frames_to_duration(r.output_delay(), path.rate)
        });
        self.processor.delay() + resampler
    }
}

/// Processes one interleaved input block: downmix, input gain and meter, the processor (skipped
/// once it failed), output gain, then every output's `ready` buffer and the output meter.
pub fn process_block(state: &mut State, interleaved: &[f32], shared: &Shared) {
    downmix(interleaved, state.in_channels, &mut state.mono);
    let input_gain = shared.input_gain();
    state.mono.iter_mut().for_each(|s| *s *= input_gain);
    shared.input_meter.store(measure(&state.mono));

    state.processed.clear();
    if !state.failed {
        let started = Instant::now();
        let result = state.processor.process(&state.mono, &mut state.processed);
        state.last_process = started.elapsed();
        if let Err(e) = result {
            state.fail(shared, e);
            state.processed.clear();
        }
    }
    let output_gain = shared.output_gain();
    state.processed.iter_mut().for_each(|s| *s *= output_gain);

    for path in &mut state.paths {
        path.ready.clear();
        path.pending.extend_from_slice(&state.processed);
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

/// Spawns thread "vc-audio-worker". `outputs[i]` feeds `state.output(i)`. Each ring is primed
/// with `PRIME_MS` of zeros before its first block, and a block is dropped when it would take the
/// backlog past the block plus `PRIME_MS + SLACK_MS`. After each block that reached the cable,
/// stores the end-to-end latency in `shared.latency_us`. Each iteration processes the whole input
/// backlog (up to the ring's capacity). Prepares the processor, skips the input
/// queued meanwhile, then runs until `stop` is true and returns the processor.
pub fn spawn(
    mut state: State,
    mut input: HeapCons<f32>,
    mut outputs: Vec<HeapProd<f32>>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<Box<dyn Processor>>> {
    thread::Builder::new()
        .name("vc-audio-worker".into())
        .spawn(move || {
            state.prepare(&shared);
            input.skip(input.occupied_len());
            let ch = state.in_channels;
            let cable = state.cable_index();
            // The whole backlog goes to the processor at once, so a processor slower than real
            // time sees it (and can drop late blocks) instead of it piling up in the input ring.
            let mut scratch = vec![0.0f32; input.capacity().get()];
            while !stop.load(Relaxed) {
                let take = input.occupied_len().min(scratch.len()) / ch * ch;
                if take == 0 {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                input.pop_slice(&mut scratch[..take]);
                process_block(&mut state, &scratch[..take], &shared);
                for (out, path) in outputs.iter_mut().zip(&mut state.paths) {
                    let rate = path.rate as usize;
                    let prime = rate * PRIME_MS as usize / 1000;
                    let slack = rate * (PRIME_MS + SLACK_MS) as usize / 1000;
                    push_block(out, &path.ready, &mut path.primed, prime, slack, &shared);
                }
                if let Some(i) = cable
                    && !state.output(i).is_empty()
                {
                    let latency = end_to_end(&LatencyParts {
                        input_backlog: input.occupied_len() / ch,
                        in_rate: state.in_rate as u32,
                        delay: state.path_delay(i),
                        inference: state.last_process(),
                        cable_backlog: outputs[i].occupied_len(),
                        cable_rate: state.paths[i].rate,
                        device: Duration::from_micros(shared.device_latency_us.load(Relaxed)),
                    });
                    shared.latency_us.store(latency.as_micros() as u64, Relaxed);
                }
            }
            state.into_processor()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
    use std::time::{Duration, Instant};

    use ringbuf::traits::{Consumer, Observer, Producer};

    use crate::meter::measure;
    use crate::processor::{Passthrough, Processor};
    use crate::ring::ring;
    use crate::shared::{Shared, StreamStatus};

    fn cfg(in_rate: u32, in_channels: usize, outputs: &[(OutputKind, u32)]) -> WorkerConfig {
        WorkerConfig {
            in_rate,
            in_channels,
            proc_rate: in_rate,
            outputs: outputs
                .iter()
                .map(|&(kind, rate)| OutputConfig { kind, rate })
                .collect(),
        }
    }

    /// Passthrough state for `cfg` (processor rate = input rate).
    fn passthrough(cfg: &WorkerConfig) -> State {
        State::new(cfg, Box::new(Passthrough::new(cfg.in_rate)))
            .map_err(|(e, _)| e)
            .unwrap()
    }

    /// Repeats every sample `factor` times, declares `rate`; fails every call when `fail`,
    /// and `prepare` fails when `fail_prepare`.
    struct Fake {
        rate: u32,
        factor: usize,
        fail: bool,
        fail_prepare: bool,
        calls: usize,
    }

    impl Fake {
        fn new(rate: u32, factor: usize) -> Self {
            Fake {
                rate,
                factor,
                fail: false,
                fail_prepare: false,
                calls: 0,
            }
        }
    }

    impl Processor for Fake {
        fn out_rate(&self) -> u32 {
            self.rate
        }

        fn delay(&self) -> Duration {
            Duration::ZERO
        }

        fn prepare(&mut self, _in_rate: u32) -> Result<(), String> {
            if self.fail_prepare {
                Err("no model".into())
            } else {
                Ok(())
            }
        }

        fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
            self.calls += 1;
            if self.fail {
                return Err("inference failed".into());
            }
            for &s in input {
                out.extend(std::iter::repeat_n(s, self.factor));
            }
            Ok(())
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
        let mut state = passthrough(&cfg(48_000, 1, &[(OutputKind::Cable, 48_000)]));
        let shared = Shared::new();
        let input = sine(48_000, 440.0, 0.7, 480);
        process_block(&mut state, &input, &shared);
        assert_eq!(state.output(0), &input[..]);
        assert_eq!(shared.input_meter.load().peak, measure(&input).peak);
    }

    #[test]
    fn resample_44k1_to_48k_length_and_sine() {
        let mut state = passthrough(&cfg(44_100, 2, &[(OutputKind::Cable, 48_000)]));
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
        let mut state = passthrough(&cfg(
            48_000,
            1,
            &[(OutputKind::Cable, 48_000), (OutputKind::Monitor, 48_000)],
        ));
        let shared = Shared::new();
        shared.set_monitor_gain(0.5);
        process_block(&mut state, &[0.8; 480], &shared);
        assert_eq!(state.output(0), &[0.8; 480][..]);
        assert_eq!(state.output(1), &[0.4; 480][..]);
        assert_eq!(shared.output_meter.load().peak, 0.8);
    }

    #[test]
    fn two_output_rates() {
        let mut state = passthrough(&cfg(
            44_100,
            1,
            &[(OutputKind::Cable, 48_000), (OutputKind::Monitor, 44_100)],
        ));
        let input = sine(44_100, 1000.0, 0.5, 44_100);
        let out = run(&mut state, &input, 1, 441);
        assert!(out[0].len().abs_diff(48_000) <= 960, "len {}", out[0].len());
        assert_eq!(out[1].len(), 44_100);
    }

    #[test]
    fn worker_thread_moves_samples_and_stops() {
        let state = passthrough(&cfg(48_000, 1, &[(OutputKind::Cable, 48_000)]));
        let (mut in_prod, in_cons) = ring(48_000);
        let (out_prod, mut out_cons) = ring(48_000);
        let input = sine(48_000, 440.0, 0.5, 4800);
        const MARKER: f32 = 9.0;

        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(state, in_cons, vec![out_prod], shared, stop.clone()).unwrap();

        // The backlog queued before `prepare` is skipped: feed markers until one comes out,
        // so the input that follows is processed in full.
        // The first push is preceded by PRIME_MS of zeros.
        let prime = (48_000 * PRIME_MS / 1000) as usize;
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        while got.len() <= prime && Instant::now() < deadline {
            in_prod.push_slice(&[MARKER; 480]);
            std::thread::sleep(Duration::from_millis(5));
            got.extend(out_cons.pop_iter());
        }
        assert_eq!(in_prod.push_slice(&input), input.len());

        let deadline = Instant::now() + Duration::from_secs(1);
        while got[prime..].iter().filter(|&&s| s != MARKER).count() < input.len()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
            got.extend(out_cons.pop_iter());
        }
        stop.store(true, Relaxed);
        handle.join().unwrap();

        assert!(got[..prime].iter().all(|&s| s == 0.0));
        let got: Vec<f32> = got
            .into_iter()
            .skip(prime)
            .skip_while(|&s| s == MARKER)
            .collect();
        assert_eq!(got, input);
    }

    #[test]
    fn end_to_end_sums_parts() {
        let parts = LatencyParts {
            input_backlog: 4800,
            in_rate: 48_000,
            delay: Duration::from_millis(310),
            inference: Duration::from_millis(120),
            cable_backlog: 2400,
            cable_rate: 48_000,
            device: Duration::from_millis(20),
        };
        assert_eq!(end_to_end(&parts), Duration::from_millis(600));
    }

    #[test]
    fn worker_sets_latency() {
        let state = passthrough(&cfg(48_000, 1, &[(OutputKind::Cable, 48_000)]));
        let (mut in_prod, in_cons) = ring(48_000);
        let (out_prod, out_cons) = ring(2 * 48_000);
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(state, in_cons, vec![out_prod], shared.clone(), stop.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while out_cons.occupied_len() == 0 && Instant::now() < deadline {
            in_prod.push_slice(&[0.1; 480]);
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Relaxed);
        handle.join().unwrap();
        assert!(out_cons.occupied_len() > 0);
        assert!(shared.latency_us.load(Relaxed) > 0);
    }

    #[test]
    fn processor_out_rate_is_resampled() {
        let mut c = cfg(24_000, 1, &[(OutputKind::Cable, 48_000)]);
        c.proc_rate = 48_000;
        let mut state = State::new(&c, Box::new(Fake::new(48_000, 2)))
            .map_err(|(e, _)| e)
            .unwrap();
        assert!(state.paths[0].resampler.is_none());
        let shared = Shared::new();
        process_block(&mut state, &[0.25; 240], &shared);
        assert_eq!(state.output(0), &[0.25; 480][..]);
    }

    #[test]
    fn gains_applied_and_metered() {
        let mut state = passthrough(&cfg(48_000, 1, &[(OutputKind::Cable, 48_000)]));
        let shared = Shared::new();
        shared.set_input_gain(2.0);
        shared.set_output_gain(0.5);
        process_block(&mut state, &[0.5; 480], &shared);
        assert_eq!(shared.input_meter.load().peak, 1.0);
        assert_eq!(state.output(0), &[0.5; 480][..]);
        assert_eq!(shared.output_meter.load().peak, 0.5);
    }

    #[test]
    fn processor_error_reports_and_stops_calling() {
        let mut fake = Fake::new(48_000, 1);
        fake.fail = true;
        let c = cfg(
            48_000,
            1,
            &[(OutputKind::Cable, 48_000), (OutputKind::Monitor, 48_000)],
        );
        let mut state = State::new(&c, Box::new(fake)).map_err(|(e, _)| e).unwrap();
        let shared = Shared::new();
        for _ in 0..2 {
            process_block(&mut state, &[0.5; 480], &shared);
            assert!(state.output(0).is_empty());
            assert!(state.output(1).is_empty());
        }
        assert_eq!(shared.status(), StreamStatus::ProcessorFailed);
        let fake = (state.into_processor() as Box<dyn Any>)
            .downcast::<Fake>()
            .unwrap();
        assert_eq!(fake.calls, 1);
    }

    #[test]
    fn prepare_error_reports() {
        let mut fake = Fake::new(48_000, 1);
        fake.fail_prepare = true;
        let c = cfg(48_000, 1, &[(OutputKind::Cable, 48_000)]);
        let mut state = State::new(&c, Box::new(fake)).map_err(|(e, _)| e).unwrap();
        let shared = Shared::new();
        state.prepare(&shared);
        assert_eq!(shared.status(), StreamStatus::ProcessorFailed);
        process_block(&mut state, &[0.5; 480], &shared);
        assert!(state.output(0).is_empty());
        let fake = (state.into_processor() as Box<dyn Any>)
            .downcast::<Fake>()
            .unwrap();
        assert_eq!(fake.calls, 0);
    }

    /// Converts `block` samples at a time (like `Streamer`), each taking `cost`. When a whole extra
    /// block is waiting, the oldest one is dropped and counted in `late`.
    struct SlowBlocks {
        block: usize,
        cost: Duration,
        fresh: Vec<f32>,
        late: Arc<AtomicUsize>,
    }

    impl Processor for SlowBlocks {
        fn out_rate(&self) -> u32 {
            16_000
        }

        fn delay(&self) -> Duration {
            Duration::ZERO
        }

        fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
            self.fresh.extend_from_slice(input);
            while self.fresh.len() >= 2 * self.block {
                self.fresh.drain(..self.block);
                self.late.fetch_add(1, Relaxed);
            }
            if self.fresh.len() >= self.block {
                std::thread::sleep(self.cost);
                out.extend(self.fresh.drain(..self.block));
            }
            Ok(())
        }
    }

    #[test]
    fn slow_processor_drops_late_blocks_and_bounds_backlog() {
        const RATE: usize = 16_000;
        let block = RATE / 10; // 100 ms
        let late = Arc::new(AtomicUsize::new(0));
        let slow = SlowBlocks {
            block,
            cost: Duration::from_millis(150),
            fresh: Vec::new(),
            late: late.clone(),
        };
        let c = cfg(RATE as u32, 1, &[(OutputKind::Cable, RATE as u32)]);
        let state = State::new(&c, Box::new(slow)).map_err(|(e, _)| e).unwrap();
        let (mut in_prod, in_cons) = ring(RATE);
        let (out_prod, _out_cons) = ring(2 * RATE);
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(state, in_cons, vec![out_prod], shared, stop.clone()).unwrap();

        // Feed in real time for 2 s; inference runs at 1.5x real time.
        let started = Instant::now();
        let mut fed = 0;
        let mut max_backlog = 0;
        while started.elapsed() < Duration::from_secs(2) {
            let due = (started.elapsed().as_micros() as usize * RATE / 1_000_000).min(2 * RATE);
            fed += in_prod.push_slice(&vec![0.1; due - fed]);
            max_backlog = max_backlog.max(in_prod.occupied_len());
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Relaxed);
        handle.join().unwrap();

        assert!(late.load(Relaxed) > 0, "no late block counted");
        // One conversion's worth of input (150 ms) plus scheduling jitter, far below the ring.
        assert!(
            max_backlog < 3 * block,
            "input backlog reached {max_backlog}"
        );
    }

    #[test]
    fn worker_returns_processor() {
        let c = cfg(48_000, 1, &[(OutputKind::Cable, 48_000)]);
        let state = State::new(&c, Box::new(Fake::new(48_000, 1)))
            .map_err(|(e, _)| e)
            .unwrap();
        let (mut in_prod, in_cons) = ring(48_000);
        let (out_prod, out_cons) = ring(48_000);
        let shared = Arc::new(Shared::new());
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(state, in_cons, vec![out_prod], shared, stop.clone()).unwrap();
        // Pushed after spawn: anything queued before `prepare` finishes is skipped.
        let deadline = Instant::now() + Duration::from_secs(1);
        while out_cons.occupied_len() == 0 && Instant::now() < deadline {
            in_prod.push_slice(&[0.1; 480]);
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Relaxed);
        let p = handle.join().unwrap();
        let fake = (p as Box<dyn Any>).downcast::<Fake>().unwrap();
        assert!(fake.calls > 0);
    }
}
