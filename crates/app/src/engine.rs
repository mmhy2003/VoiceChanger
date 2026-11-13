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
    fn out_rate(&self) -> u32 {
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

    use vc_audio::Processor;
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

    fn processor(fail_from: usize) -> (EngineProcessor<FakeConvert>, ErrorSlot) {
        let streamer = Streamer::new(
            FakeConvert {
                calls: 0,
                fail_from,
            },
            StreamSettings::default(),
            Arc::new(Controls::new()),
            Arc::new(StreamStats::new()),
        );
        let error = ErrorSlot::default();
        (EngineProcessor::new(streamer, error.clone()), error)
    }

    /// One default block (250 ms) at 16 kHz.
    fn block() -> Vec<f32> {
        vec![0.1; 4000]
    }

    #[test]
    fn processor_reports_rate_and_delay() {
        let (mut p, error) = processor(usize::MAX);
        p.prepare(16_000).unwrap();
        assert_eq!(p.out_rate(), 40_000);
        assert_eq!(p.delay(), Duration::from_millis(310));
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
}
