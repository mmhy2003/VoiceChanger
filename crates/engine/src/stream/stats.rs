//! Per-block streaming stats written by the inference worker and read by the UI.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct StreamStats {
    inference_us: AtomicU64,
    block_us: AtomicU64,
    /// Smoothed inference / block ratio (f32 bits).
    ratio: AtomicU32,
    blocks: AtomicU64,
    late_blocks: AtomicU64,
}

fn micros(d: Duration) -> u64 {
    u64::try_from(d.as_micros()).unwrap_or(u64::MAX)
}

impl StreamStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets everything to zero.
    pub fn reset(&self) {
        self.inference_us.store(0, Ordering::Relaxed);
        self.block_us.store(0, Ordering::Relaxed);
        self.ratio.store(0, Ordering::Relaxed);
        self.blocks.store(0, Ordering::Relaxed);
        self.late_blocks.store(0, Ordering::Relaxed);
    }

    pub fn set_block(&self, block: Duration) {
        self.block_us.store(micros(block), Ordering::Relaxed);
    }

    /// Counts one block and stores its inference time. The ratio r = inference / block is taken as is
    /// for the first block, then smoothed: ratio += alpha·(r − ratio), alpha = min(block_secs / 3, 1).
    pub fn record(&self, inference: Duration) {
        self.inference_us
            .store(micros(inference), Ordering::Relaxed);
        let block = self.block().as_secs_f32();
        let r = if block > 0.0 {
            inference.as_secs_f32() / block
        } else {
            0.0
        };
        let ratio = if self.blocks.fetch_add(1, Ordering::Relaxed) == 0 {
            r
        } else {
            let alpha = (block / 3.0).min(1.0);
            let old = self.ratio();
            old + alpha * (r - old)
        };
        self.ratio.store(ratio.to_bits(), Ordering::Relaxed);
    }

    pub fn record_late(&self) {
        self.late_blocks.fetch_add(1, Ordering::Relaxed);
    }

    pub fn inference(&self) -> Duration {
        Duration::from_micros(self.inference_us.load(Ordering::Relaxed))
    }

    pub fn block(&self) -> Duration {
        Duration::from_micros(self.block_us.load(Ordering::Relaxed))
    }

    pub fn ratio(&self) -> f32 {
        f32::from_bits(self.ratio.load(Ordering::Relaxed))
    }

    pub fn blocks(&self) -> u64 {
        self.blocks.load(Ordering::Relaxed)
    }

    pub fn late_blocks(&self) -> u64 {
        self.late_blocks.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn first_block_sets_ratio() {
        let s = StreamStats::new();
        s.set_block(ms(250));
        s.record(ms(100));
        assert!((s.ratio() - 0.4).abs() < 1e-6, "{}", s.ratio());
        assert_eq!(s.blocks(), 1);
        assert_eq!(s.inference(), ms(100));
        assert_eq!(s.block(), ms(250));
    }

    #[test]
    fn ratio_is_smoothed() {
        let s = StreamStats::new();
        s.set_block(ms(250));
        s.record(ms(100));
        s.record(ms(250));
        let want = 0.4 + (1.0 / 12.0) * (1.0 - 0.4);
        assert!((s.ratio() - want).abs() < 1e-4, "{}", s.ratio());
        assert_eq!(s.blocks(), 2);
        assert_eq!(s.inference(), ms(250));
    }

    #[test]
    fn late_and_reset() {
        let s = StreamStats::new();
        s.set_block(ms(250));
        s.record(ms(100));
        s.record_late();
        s.record_late();
        assert_eq!(s.late_blocks(), 2);
        s.reset();
        assert_eq!(s.late_blocks(), 0);
        assert_eq!(s.blocks(), 0);
        assert_eq!(s.ratio(), 0.0);
        assert_eq!(s.inference(), Duration::ZERO);
        assert_eq!(s.block(), Duration::ZERO);
    }
}
