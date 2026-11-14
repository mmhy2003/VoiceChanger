//! The hook the worker runs each block through (voice conversion, or passthrough).

use std::any::Any;
use std::time::Duration;

/// Converts mono input blocks on the worker thread. `Any` lets the owner downcast the processor
/// it gets back from `AudioSession::stop`.
pub trait Processor: Any + Send {
    /// Sample rate of the samples `process` appends, for input at `in_rate`.
    fn out_rate(&self, in_rate: u32) -> u32;

    /// Algorithmic delay of an output sample behind its input, not counting the wait for a whole
    /// block (that is in the output backlog once the block is pushed). Valid after `prepare`.
    fn delay(&self) -> Duration;

    /// Called once on the worker thread before the first `process`, with the input device rate.
    fn prepare(&mut self, in_rate: u32) -> Result<(), String> {
        let _ = in_rate;
        Ok(())
    }

    /// Appends converted mono samples (possibly none) for `input` (mono, device rate) to `out`.
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String>;
}

/// The original voice: copies the input unchanged, at the input rate. The app runs it when no
/// voice model is chosen and in Bypass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Passthrough;

impl Processor for Passthrough {
    fn out_rate(&self, in_rate: u32) -> u32 {
        in_rate
    }

    fn delay(&self) -> Duration {
        Duration::ZERO
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String> {
        out.extend_from_slice(input);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_follows_the_input_rate() {
        let mut p = Passthrough;
        assert_eq!(p.out_rate(44_100), 44_100);
        assert_eq!(p.out_rate(48_000), 48_000);
        assert_eq!(p.delay(), Duration::ZERO);
        assert_eq!(p.prepare(48_000), Ok(()));
        let mut out = vec![1.0];
        p.process(&[0.1, -0.2], &mut out).unwrap();
        assert_eq!(out, [1.0, 0.1, -0.2]);
    }
}
