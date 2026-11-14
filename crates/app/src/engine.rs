//! Adapter that runs the engine's `Streamer` as the audio worker's `Processor`.

use std::fmt::Display;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use vc_engine::{Convert, Streamer};

/// The processor's last error message, read by the UI for the banner.
pub type ErrorSlot = Arc<Mutex<Option<String>>>;

/// A `Streamer` that the audio worker can run; errors also land in `error`.
pub struct EngineProcessor<C: Convert> {
    streamer: Streamer<C>,
    error: ErrorSlot,
}

impl<C: Convert> EngineProcessor<C> {
    pub fn new(streamer: Streamer<C>, error: ErrorSlot) -> Self {
        Self { streamer, error }
    }

    /// Gives back the converter (the loaded model).
    pub fn into_inner(self) -> C {
        self.streamer.into_inner()
    }

    /// Turns `e` into text and keeps a copy in the error slot.
    fn report(&self, e: impl Display) -> String {
        let text = e.to_string();
        *self.error.lock().unwrap_or_else(PoisonError::into_inner) = Some(text.clone());
        text
    }
}

impl<C: Convert + Send + 'static> vc_audio::Processor for EngineProcessor<C> {
    fn out_rate(&self, _in_rate: u32) -> u32 {
        self.streamer.sample_rate()
    }

    fn delay(&self) -> Duration {
        self.streamer.delay()
    }

    fn prepare(&mut self, in_rate: u32) -> Result<(), String> {
        self.streamer.prepare(in_rate).map_err(|e| self.report(e))
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        self.streamer
            .process(input, out)
            .map_err(|e| self.report(e))
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    use std::time::Instant;

    use ringbuf::traits::{Consumer, Producer};
    use vc_audio::Processor;
    use vc_audio::ring::ring;
    use vc_audio::worker::{OutputConfig, OutputKind, PRIME_MS, State, WorkerConfig, spawn};
    use vc_audio::{Passthrough, Shared};
    use vc_engine::{Controls, EngineError, Params, StreamSettings, StreamStats};

    use super::*;

    /// Upsamples 16 kHz to 40 kHz by sample-and-hold; fails from call number `fail_from` on.
    struct FakeConvert {
        calls: usize,
        fail_from: usize,
    }

    impl Convert for FakeConvert {
        fn sample_rate(&self) -> u32 {
            40_000
        }

        fn convert_past(
            &mut self,
            window: &[f32],
            context: usize,
            _params: &Params,
        ) -> vc_engine::Result<Vec<f32>> {
            self.calls += 1;
            if self.calls >= self.fail_from {
                return Err(EngineError::Inference("fake failure".into()));
            }
            let n = (window.len() - context) * 5 / 2;
            Ok((0..n).map(|i| window[context + i * 2 / 5]).collect())
        }
    }

    fn processor_with(convert: FakeConvert) -> (EngineProcessor<FakeConvert>, ErrorSlot) {
        let streamer = Streamer::new(
            convert,
            StreamSettings::default(),
            Arc::new(Controls::new()),
            Arc::new(StreamStats::new()),
        );
        let error = ErrorSlot::default();
        (EngineProcessor::new(streamer, error.clone()), error)
    }

    fn processor(fail_from: usize) -> (EngineProcessor<FakeConvert>, ErrorSlot) {
        processor_with(FakeConvert {
            calls: 0,
            fail_from,
        })
    }

    /// The converter inside the processor a worker handed back.
    fn into_convert(p: Box<dyn Processor>) -> FakeConvert {
        (p as Box<dyn Any>)
            .downcast::<EngineProcessor<FakeConvert>>()
            .expect("an EngineProcessor<FakeConvert>")
            .into_inner()
    }

    /// Runs `processor` on a real worker thread: 16 kHz mono in, one cable ring at the processor's
    /// rate. Feeds `chunk` every few milliseconds until `done` accepts what reached the cable (after
    /// the priming silence) or 5 s pass. Returns those samples and the processor the worker gives back.
    fn run_on_worker(
        processor: Box<dyn Processor>,
        chunk: &[f32],
        done: impl Fn(&[f32]) -> bool,
    ) -> (Vec<f32>, Box<dyn Processor>) {
        const IN_RATE: u32 = 16_000;
        let rate = processor.out_rate(IN_RATE);
        let cfg = WorkerConfig {
            in_rate: IN_RATE,
            in_channels: 1,
            proc_rate: rate,
            outputs: vec![OutputConfig {
                kind: OutputKind::Cable,
                rate,
            }],
        };
        let state = State::new(&cfg, processor).map_err(|(e, _)| e).unwrap();
        let (mut in_prod, in_cons) = ring(IN_RATE as usize);
        let (out_prod, mut out_cons) = ring(2 * rate as usize);
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::new());
        let worker = spawn(state, in_cons, vec![out_prod], shared, stop.clone()).unwrap();
        let prime = (rate * PRIME_MS / 1000) as usize;
        let mut got = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(got.get(prime..).unwrap_or_default()) && Instant::now() < deadline {
            in_prod.push_slice(chunk);
            std::thread::sleep(Duration::from_millis(5));
            got.extend(out_cons.pop_iter());
        }
        stop.store(true, Relaxed);
        let processor = worker.join().unwrap();
        let voice = got.split_off(prime.min(got.len()));
        assert!(
            got.iter().all(|&s| s == 0.0),
            "the ring starts with silence"
        );
        (voice, processor)
    }

    /// One default block (250 ms) at 16 kHz.
    fn block() -> Vec<f32> {
        vec![0.1; 4000]
    }

    #[test]
    fn processor_reports_rate_and_delay() {
        let (mut p, error) = processor(usize::MAX);
        p.prepare(16_000).unwrap();
        assert_eq!(p.out_rate(16_000), 40_000);
        assert_eq!(p.delay(), Duration::from_millis(60));
        let mut out = Vec::new();
        p.process(&block(), &mut out).unwrap();
        assert_eq!(out.len(), 10_000);
        assert_eq!(*error.lock().unwrap(), None);
    }

    #[test]
    fn process_error_fills_error_slot() {
        // Call 1 is the warm-up in `prepare`; the first block's conversion fails.
        let (mut p, error) = processor(2);
        p.prepare(16_000).unwrap();
        let e = p.process(&block(), &mut Vec::new()).unwrap_err();
        assert_eq!(e, "inference failed: fake failure");
        assert_eq!(error.lock().unwrap().as_deref(), Some(e.as_str()));
    }

    #[test]
    fn prepare_error_fills_error_slot() {
        let (mut p, error) = processor(1);
        let e = p.prepare(16_000).unwrap_err();
        assert_eq!(e, "inference failed: fake failure");
        assert_eq!(error.lock().unwrap().as_deref(), Some(e.as_str()));
    }

    #[test]
    fn downcast_round_trip() {
        let (mut p, _) = processor(usize::MAX);
        p.prepare(16_000).unwrap();
        let boxed: Box<dyn Processor> = Box::new(p);
        let ep = (boxed as Box<dyn Any>)
            .downcast::<EngineProcessor<FakeConvert>>()
            .expect("an EngineProcessor<FakeConvert>");
        assert_eq!(ep.into_inner().calls, 1);
    }

    #[test]
    fn bypass_round_trip_keeps_the_converter() {
        // 25 ms at 16 kHz; ten of them make one default block, which converts to 10 000 samples.
        let voice = vec![0.1; 400];
        let one_block = |out: &[f32]| out.len() >= 10_000;

        // The converted voice.
        let (p, _) = processor(usize::MAX);
        let (converted, back) = run_on_worker(Box::new(p), &voice, one_block);
        assert!(
            one_block(&converted),
            "no converted block reached the cable"
        );
        assert!(
            converted.iter().any(|&s| s > 0.05),
            "the converted voice is silent"
        );
        let convert = into_convert(back);
        let calls = convert.calls;
        assert!(calls >= 2, "the warm-up and a block, got {calls} calls");

        // Bypass on: the original voice, sample for sample. The chunk has an even length and the
        // input ring holds a whole number of chunks, so the pairs stay aligned.
        let dry = [0.25, -0.5].repeat(80);
        let enough = |out: &[f32]| out.len() >= 1600;
        let (original, back) = run_on_worker(Box::new(Passthrough), &dry, enough);
        assert!(
            enough(&original),
            "the original voice did not reach the cable"
        );
        assert!(
            original
                .as_chunks::<2>()
                .0
                .iter()
                .all(|&pair| pair == [0.25, -0.5]),
            "the original voice was changed"
        );
        assert!((back as Box<dyn Any>).downcast::<Passthrough>().is_ok());

        // Bypass off: the same converter streams again.
        let (p, error) = processor_with(convert);
        let (converted, back) = run_on_worker(Box::new(p), &voice, one_block);
        assert!(one_block(&converted), "the converter did not stream again");
        assert!(into_convert(back).calls > calls);
        assert_eq!(*error.lock().unwrap(), None);
    }
}
