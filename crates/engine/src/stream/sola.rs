//! SOLA (synchronised overlap-add) crossfade between consecutive converted blocks, at the model rate.

use std::f32::consts::FRAC_PI_2;

/// Joins converted blocks: each block is shifted by the offset whose start best matches the tail kept
/// from the previous block, then crossfaded into that tail.
#[derive(Debug, Clone)]
pub struct Sola {
    block: usize,
    crossfade: usize,
    search: usize,
    /// Tail of the previous block, faded out over the next block's first `crossfade` samples.
    prev: Vec<f32>,
    fade_in: Vec<f32>,
}

impl Sola {
    pub fn new(block: usize, crossfade: usize, search: usize) -> Self {
        let fade_in = if crossfade == 1 {
            vec![1.0]
        } else {
            (0..crossfade)
                .map(|i| {
                    (FRAC_PI_2 * i as f32 / (crossfade - 1) as f32)
                        .sin()
                        .powi(2)
                })
                .collect()
        };
        Self {
            block,
            crossfade,
            search,
            prev: vec![0.0; crossfade],
            fade_in,
        }
    }

    /// Samples `process` expects per call: crossfade + search + block.
    pub fn input_len(&self) -> usize {
        self.crossfade + self.search + self.block
    }

    /// k in 0..search maximising Σ conv[k+i]·prev[i] / sqrt(Σ conv[k+i]² + 1e-8).
    /// Ties and an all-zero prev give 0.
    pub fn best_offset(&self, conv: &[f32]) -> usize {
        if self.prev.iter().all(|&p| p == 0.0) {
            return 0;
        }
        let mut best = 0;
        let mut best_score = f64::NEG_INFINITY;
        for k in 0..self.search {
            let seg = &conv[k..k + self.crossfade];
            let (dot, energy) =
                seg.iter()
                    .zip(&self.prev)
                    .fold((0.0f64, 0.0f64), |(dot, energy), (&c, &p)| {
                        let c = f64::from(c);
                        (dot + c * f64::from(p), energy + c * c)
                    });
            let score = dot / (energy + 1e-8).sqrt();
            if score > best_score {
                best = k;
                best_score = score;
            }
        }
        best
    }

    /// Appends `block` samples to `out`, starting at `best_offset(conv)`, with the first `crossfade`
    /// samples crossfaded from the previous tail. Keeps the samples right after them as the new tail.
    pub fn process(&mut self, conv: &[f32], out: &mut Vec<f32>) {
        debug_assert_eq!(conv.len(), self.input_len());
        let k = self.best_offset(conv);
        // Settings allow a crossfade longer than the block; only the block's samples can be faded then.
        let cf = self.crossfade.min(self.block);
        out.extend(
            conv[k..k + cf]
                .iter()
                .zip(&self.prev)
                .zip(&self.fade_in)
                .map(|((&c, &p), &f)| c * f + p * (1.0 - f)),
        );
        out.extend_from_slice(&conv[k + cf..k + self.block]);
        let tail = k + self.block;
        self.prev
            .copy_from_slice(&conv[tail..tail + self.crossfade]);
    }

    /// Forgets the previous tail, so the next block fades in from silence.
    pub fn reset(&mut self) {
        self.prev.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::TAU;

    const BLOCK: usize = 10000;
    const CROSSFADE: usize = 2000;
    const SEARCH: usize = 400;
    const RATE: f32 = 40000.0;

    fn sola() -> Sola {
        Sola::new(BLOCK, CROSSFADE, SEARCH)
    }

    fn sine(freq: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|n| (TAU * freq * n as f32 / RATE).sin())
            .collect()
    }

    #[test]
    fn input_len_covers_crossfade_search_and_block() {
        assert_eq!(sola().input_len(), CROSSFADE + SEARCH + BLOCK);
    }

    #[test]
    fn finds_known_offset() {
        // A pure 1 kHz sine repeats every 40 samples, so every k ≡ 137 (mod 40) would match up to rounding.
        // A 1.37 kHz partial makes the match at 137 unique within the search range.
        let s: Vec<f32> = sine(1000.0, 30000)
            .iter()
            .zip(sine(1370.0, 30000))
            .map(|(a, b)| a + 0.5 * b)
            .collect();
        let mut sola = sola();
        let mut out = Vec::new();
        sola.process(&s[0..12400], &mut out);
        let start = 10000 - 137;
        assert_eq!(sola.best_offset(&s[start..start + 12400]), 137);
    }

    #[test]
    fn all_zero_prev_gives_offset_zero() {
        let s = sine(1000.0, 12400);
        assert_eq!(sola().best_offset(&s), 0);
    }

    #[test]
    fn joins_sine_without_step() {
        let s = sine(220.0, 6 * BLOCK + 12400);
        let sine_step = s
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        let mut sola = sola();
        let mut out = Vec::new();
        for n in 0..5 {
            let start = n * BLOCK;
            sola.process(&s[start..start + sola.input_len()], &mut out);
        }
        let max_step = out[BLOCK..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            max_step <= 1.5 * sine_step,
            "max step {max_step}, sine step {sine_step}"
        );
    }

    #[test]
    fn emits_exactly_block() {
        let s = sine(220.0, 12400);
        let mut sola = sola();
        let mut out = Vec::new();
        for _ in 0..5 {
            sola.process(&s, &mut out);
        }
        assert_eq!(out.len(), 5 * BLOCK);
    }

    #[test]
    fn first_block_fades_in_from_silence() {
        let dc = vec![0.5; 12400];
        let mut sola = sola();
        let mut out = Vec::new();
        sola.process(&dc, &mut out);
        assert!(out[0].abs() < 1e-3, "{}", out[0]);
        assert!((out[CROSSFADE] - 0.5).abs() < 1e-6, "{}", out[CROSSFADE]);
    }

    #[test]
    fn reset_fades_in_again() {
        let dc = vec![0.5; 12400];
        let mut sola = sola();
        let mut out = Vec::new();
        sola.process(&dc, &mut out);
        sola.reset();
        out.clear();
        sola.process(&dc, &mut out);
        assert!(out[0].abs() < 1e-3, "{}", out[0]);
    }

    #[test]
    fn crossfade_longer_than_block_emits_block() {
        let mut sola = Sola::new(4, 8, 2);
        let mut out = Vec::new();
        for _ in 0..3 {
            sola.process(&[0.5; 14], &mut out);
        }
        assert_eq!(out.len(), 12);
    }

    #[test]
    fn single_sample_crossfade_takes_new_signal() {
        let mut sola = Sola::new(4, 1, 1);
        let mut out = Vec::new();
        sola.process(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &mut out);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
    }
}
