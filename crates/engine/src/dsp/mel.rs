//! RMVPE log-mel front end, matching RVC `rmvpe.py`'s `MelSpectrogram(128, 16000, 1024, 160, 30, 8000)`.

use std::sync::Arc;

use rustfft::{Fft, FftPlanner, num_complex::Complex32};

pub const N_FFT: usize = 1024;
pub const HOP: usize = 160;
pub const N_MELS: usize = 128;
pub const FMIN: f32 = 30.0;
pub const FMAX: f32 = 8000.0;

const SAMPLE_RATE: f32 = 16000.0;
const N_BINS: usize = N_FFT / 2 + 1;

pub struct MelExtractor {
    window: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    /// Row-major `[N_MELS][N_BINS]`.
    filterbank: Vec<f32>,
}

impl MelExtractor {
    pub fn new() -> Self {
        // Periodic Hann, as `torch.hann_window(1024)`.
        let window = (0..N_FFT)
            .map(|n| {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / N_FFT as f64).cos()) as f32
            })
            .collect();
        let fft = FftPlanner::new().plan_fft_forward(N_FFT);
        Self {
            window,
            fft,
            filterbank: filterbank(),
        }
    }

    /// Returns the log-mel spectrogram, row-major `[N_MELS][frames]`, and `frames = len/HOP + 1`.
    pub fn log_mel(&mut self, audio: &[f32]) -> (Vec<f32>, usize) {
        let padded = centre_pad(audio);
        let frames = audio.len() / HOP + 1;
        let mut out = vec![0.0f32; N_MELS * frames];
        let mut buf = vec![Complex32::default(); N_FFT];
        let mut scratch = vec![Complex32::default(); self.fft.get_inplace_scratch_len()];
        let mut mag = [0.0f32; N_BINS];
        for f in 0..frames {
            let frame = &padded[f * HOP..f * HOP + N_FFT];
            for ((c, &s), &w) in buf.iter_mut().zip(frame).zip(&self.window) {
                *c = Complex32::new(s * w, 0.0);
            }
            self.fft.process_with_scratch(&mut buf, &mut scratch);
            for (m, c) in mag.iter_mut().zip(&buf) {
                *m = c.norm();
            }
            for (m, row) in self.filterbank.chunks(N_BINS).enumerate() {
                let energy: f32 = row.iter().zip(&mag).map(|(w, x)| w * x).sum();
                out[m * frames + f] = energy.max(1e-5).ln();
            }
        }
        (out, frames)
    }
}

/// Centres the STFT: `N_FFT/2` samples on each side, reflected (as `torch.stft(center=True)`), or
/// zeros when the input is too short to reflect.
fn centre_pad(audio: &[f32]) -> Vec<f32> {
    let half = N_FFT / 2;
    let len = audio.len();
    let mut padded = Vec::with_capacity(len + N_FFT);
    if len > half {
        padded.extend((1..=half).rev().map(|i| audio[i]));
        padded.extend_from_slice(audio);
        padded.extend((0..half).map(|j| audio[len - 2 - j]));
    } else {
        padded.resize(half, 0.0);
        padded.extend_from_slice(audio);
        padded.resize(len + N_FFT, 0.0);
    }
    padded
}

/// librosa `mel(sr=16000, n_fft=1024, n_mels=128, fmin=30, fmax=8000, htk=True, norm="slaney")`,
/// row-major `[N_MELS][N_BINS]`.
fn filterbank() -> Vec<f32> {
    let hz = band_edges();
    let mut fb = vec![0.0f32; N_MELS * N_BINS];
    for (m, row) in fb.chunks_mut(N_BINS).enumerate() {
        let (lo, centre, hi) = (hz[m], hz[m + 1], hz[m + 2]);
        let norm = 2.0 / (hi - lo);
        for (k, w) in row.iter_mut().enumerate() {
            let f = k as f64 * f64::from(SAMPLE_RATE) / N_FFT as f64;
            let rise = (f - lo) / (centre - lo);
            let fall = (hi - f) / (hi - centre);
            *w = (rise.min(fall).max(0.0) * norm) as f32;
        }
    }
    fb
}

impl Default for MelExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// The `N_MELS + 2` band edges in Hz, evenly spaced on the HTK mel scale from `FMIN` to `FMAX`.
fn band_edges() -> Vec<f64> {
    let to_mel = |f: f64| 2595.0 * (1.0 + f / 700.0).log10();
    let to_hz = |m: f64| 700.0 * (10f64.powf(m / 2595.0) - 1.0);
    let (lo, hi) = (to_mel(f64::from(FMIN)), to_mel(f64::from(FMAX)));
    let n = N_MELS + 1;
    (0..=n)
        .map(|i| to_hz(lo + (hi - lo) * i as f64 / n as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG_FLOOR: f32 = -11.512925;

    fn sine(freq: f32, amp: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| amp * (2.0 * std::f32::consts::PI * freq * n as f32 / SAMPLE_RATE).sin())
            .collect()
    }

    #[test]
    fn frame_count() {
        let mut mel = MelExtractor::new();
        let (out, frames) = mel.log_mel(&vec![0.0; 16000]);
        assert_eq!(frames, 101);
        assert_eq!(out.len(), N_MELS * 101);
        assert_eq!(mel.log_mel(&vec![0.0; 1600]).1, 11);
    }

    #[test]
    fn silence_is_log_floor() {
        let (out, _) = MelExtractor::new().log_mel(&vec![0.0; 4000]);
        assert!(out.iter().all(|v| (v - LOG_FLOOR).abs() < 1e-4));
    }

    #[test]
    fn filterbank_is_slaney_triangles() {
        let mel = MelExtractor::new();
        assert_eq!(mel.filterbank.len(), N_MELS * N_BINS);
        let mut last_peak = 0;
        for (m, row) in mel.filterbank.chunks(N_BINS).enumerate() {
            assert!(row.iter().sum::<f32>() > 0.0, "row {m} is empty");
            let peak = row
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0;
            if m > 0 {
                assert!(
                    peak >= last_peak,
                    "row {m} peaks at {peak} below {last_peak}"
                );
            }
            last_peak = peak;
        }
        // Slaney normalisation: the widest (last) band has the smallest peak weight.
        let peak_of = |m: usize| {
            mel.filterbank[m * N_BINS..(m + 1) * N_BINS]
                .iter()
                .fold(0.0f32, |a, &b| a.max(b))
        };
        assert!(peak_of(N_MELS - 1) < peak_of(N_MELS / 2));
    }

    #[test]
    fn sine_440_peaks_in_right_band() {
        let (out, frames) = MelExtractor::new().log_mel(&sine(440.0, 0.5, 16000));
        let mid = frames / 2;
        let (row, value) = (0..N_MELS)
            .map(|m| (m, out[m * frames + mid]))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let hz = band_edges();
        assert!(
            hz[row] <= 440.0 && 440.0 <= hz[row + 2],
            "row {row} spans {}..{}",
            hz[row],
            hz[row + 2]
        );
        assert!(value > 0.0, "peak value {value}");
    }

    #[test]
    fn short_input_works() {
        let (out, frames) = MelExtractor::new().log_mel(&sine(440.0, 0.5, 100));
        assert_eq!(frames, 1);
        assert_eq!(out.len(), N_MELS);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
