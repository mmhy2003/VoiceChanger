//! Mono sample-rate conversion with rubato: a streaming resampler and a one-shot helper.

use rubato::{FftFixedIn, Resampler};

use crate::error::{EngineError, Result};

/// Input frames per rubato chunk.
const CHUNK: usize = 1024;

/// Streaming mono resampler. Output lags input by `output_delay()` samples.
pub struct StreamResampler {
    inner: FftFixedIn<f32>,
    pending: Vec<f32>,
    chunk_out: Vec<Vec<f32>>,
}

impl StreamResampler {
    pub fn new(from: u32, to: u32) -> Result<Self> {
        let inner = FftFixedIn::<f32>::new(from as usize, to as usize, CHUNK, 2, 1)
            .map_err(|e| EngineError::Inference(format!("resampler {from}→{to} Hz: {e}")))?;
        let chunk_out = inner.output_buffer_allocate(true);
        Ok(Self {
            inner,
            pending: Vec::with_capacity(CHUNK),
            chunk_out,
        })
    }

    /// Buffers `input` and appends the output of every full chunk to `out`.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.pending.extend_from_slice(input);
        let mut start = 0;
        while self.pending.len() - start >= self.inner.input_frames_next() {
            let n = self.inner.input_frames_next();
            let (_, out_n) = self
                .inner
                .process_into_buffer(
                    &[&self.pending[start..start + n]],
                    &mut self.chunk_out,
                    None,
                )
                .map_err(|e| EngineError::Inference(format!("resampling: {e}")))?;
            out.extend_from_slice(&self.chunk_out[0][..out_n]);
            start += n;
        }
        self.pending.drain(..start);
        Ok(())
    }

    /// Processes the buffered remainder, zero-padded to a full chunk, and appends it to `out`.
    pub fn flush(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let chunk = self
            .inner
            .process_partial(Some(&[&self.pending[..]]), None)
            .map_err(|e| EngineError::Inference(format!("resampling: {e}")))?;
        out.extend_from_slice(&chunk[0]);
        self.pending.clear();
        Ok(())
    }

    /// Output samples that precede the first sample corresponding to the input start.
    pub fn output_delay(&self) -> usize {
        self.inner.output_delay()
    }
}

/// Resamples a whole mono signal from `from` to `to` Hz, compensating the resampler delay.
/// The output has `round(len·to/from)` samples.
pub fn resample(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
    if from == to {
        return Ok(input.to_vec());
    }
    let (from64, to64) = (u64::from(from), u64::from(to));
    let expected = ((input.len() as u64 * to64 + from64 / 2) / from64) as usize;
    let mut r = StreamResampler::new(from, to)?;
    let mut out = Vec::with_capacity(expected + r.output_delay() + CHUNK);
    r.push(input, &mut out)?;
    r.flush(&mut out)?;
    let delay = r.output_delay();
    let zeros = [0.0; CHUNK];
    while out.len() < delay + expected {
        r.push(&zeros, &mut out)?;
    }
    out.drain(..delay);
    out.truncate(expected);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn same_rate_is_copy() {
        let x = sine(16_000, 440.0, 0.5, 1234);
        assert_eq!(resample(&x, 16_000, 16_000).unwrap(), x);
    }

    #[test]
    fn lengths_48k_to_16k() {
        assert_eq!(
            resample(&vec![0.0; 48_000], 48_000, 16_000).unwrap().len(),
            16_000
        );
    }

    #[test]
    fn lengths_16k_to_40k() {
        assert_eq!(
            resample(&vec![0.0; 16_000], 16_000, 40_000).unwrap().len(),
            40_000
        );
    }

    #[test]
    fn lengths_44k1_to_16k() {
        assert_eq!(
            resample(&vec![0.0; 44_100], 44_100, 16_000).unwrap().len(),
            16_000
        );
    }

    #[test]
    fn odd_length() {
        assert_eq!(
            resample(&vec![0.0; 1001], 16_000, 48_000).unwrap().len(),
            3003
        );
    }

    #[test]
    fn sine_amplitude_kept() {
        let y = resample(&sine(48_000, 440.0, 0.5, 48_000), 48_000, 16_000).unwrap();
        let middle = &y[y.len() / 4..3 * y.len() / 4];
        let peak = middle.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.5).abs() <= 0.02, "peak {peak}");
    }

    #[test]
    fn stream_matches_oneshot_length() {
        let x = sine(48_000, 440.0, 0.5, 10_007);
        let oneshot = resample(&x, 48_000, 16_000).unwrap();

        let mut r = StreamResampler::new(48_000, 16_000).unwrap();
        let mut streamed = Vec::new();
        for chunk in x.chunks(333) {
            r.push(chunk, &mut streamed).unwrap();
        }
        r.flush(&mut streamed).unwrap();
        let delay = r.output_delay();
        while streamed.len() < delay + oneshot.len() {
            r.push(&[0.0; 1024], &mut streamed).unwrap();
        }
        assert_eq!(&streamed[delay..delay + oneshot.len()], &oneshot[..]);
    }
}
