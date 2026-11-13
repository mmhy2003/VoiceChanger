//! Lock-free SPSC rings and the audio callback bodies that use them.
//!
//! The callback bodies never allocate, lock, block or log.

use std::sync::atomic::Ordering::Relaxed;

use cpal::{FromSample, Sample};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
pub use ringbuf::{HeapCons, HeapProd};

use crate::shared::Shared;

/// Preallocated SPSC ring of f32 (capacity >= 1).
pub fn ring(capacity: usize) -> (HeapProd<f32>, HeapCons<f32>) {
    HeapRb::<f32>::new(capacity.max(1)).split()
}

/// Input callback body. Pushes the whole interleaved buffer or none of it, so frames stay
/// channel-aligned. A buffer that does not fit is counted in `overflows`.
pub fn push_input<T>(data: &[T], prod: &mut HeapProd<f32>, shared: &Shared)
where
    T: Sample,
    f32: FromSample<T>,
{
    if prod.vacant_len() < data.len() {
        shared.overflows.fetch_add(data.len() as u64, Relaxed);
        return;
    }
    for &s in data {
        let _ = prod.try_push(s.to_sample::<f32>());
    }
}

/// Output callback body. For each frame of `data` pops one mono sample (0.0 when the ring is
/// empty) and writes it to every channel. An empty ring counts as an underrun only once a sample
/// has been popped (`*started`), so the wait for the first block is not counted.
pub fn fill_output<T>(
    data: &mut [T],
    channels: usize,
    cons: &mut HeapCons<f32>,
    started: &mut bool,
    shared: &Shared,
) where
    T: Sample + FromSample<f32>,
{
    let mut underruns = 0u64;
    for frame in data.chunks_mut(channels.max(1)) {
        let v = match cons.try_pop() {
            Some(v) => {
                *started = true;
                v
            }
            None => {
                underruns += u64::from(*started);
                0.0
            }
        };
        frame.fill(T::from_sample(v));
    }
    if underruns > 0 {
        shared.underruns.fetch_add(underruns, Relaxed);
    }
}

/// Worker-side push. A block that would take the backlog past `max_backlog` is dropped and
/// counted in `overflows`, which keeps latency bounded when device clocks drift. Samples that do
/// not fit in the ring are counted too.
pub fn push_output(prod: &mut HeapProd<f32>, samples: &[f32], max_backlog: usize, shared: &Shared) {
    if prod.occupied_len() + samples.len() > max_backlog {
        shared.overflows.fetch_add(samples.len() as u64, Relaxed);
        return;
    }
    let pushed = prod.push_slice(samples);
    let dropped = samples.len() - pushed;
    if dropped > 0 {
        shared.overflows.fetch_add(dropped as u64, Relaxed);
    }
}

/// Worker-side push of one processed block. On the first non-empty push (`*primed == false`)
/// pushes `prime` zeros first and sets `primed`, so the output callback has a cushion. The block
/// is then pushed with a backlog limit of `samples.len() + slack`.
pub fn push_block(
    prod: &mut HeapProd<f32>,
    samples: &[f32],
    primed: &mut bool,
    prime: usize,
    slack: usize,
    shared: &Shared,
) {
    if samples.is_empty() {
        return;
    }
    if !*primed {
        prod.push_iter(std::iter::repeat_n(0.0, prime));
        *primed = true;
    }
    push_output(prod, samples, samples.len() + slack, shared);
}

/// Output callback helper: stores (playback − callback) in µs in `shared.device_latency_us`.
/// Leaves it unchanged when playback is earlier than the callback.
pub fn store_device_latency(ts: &cpal::OutputStreamTimestamp, shared: &Shared) {
    if let Some(d) = ts.playback.duration_since(&ts.callback) {
        shared
            .device_latency_us
            .store(d.as_micros().min(u128::from(u64::MAX)) as u64, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pop_all(cons: &mut HeapCons<f32>) -> Vec<f32> {
        cons.pop_iter().collect()
    }

    #[test]
    fn push_input_converts_i16() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(8);
        push_input(&[i16::MAX, 0, i16::MIN], &mut prod, &shared);
        let got = pop_all(&mut cons);
        assert_eq!(got.len(), 3);
        for (g, e) in got.iter().zip([1.0, 0.0, -1.0]) {
            assert!((g - e).abs() < 1e-3, "{g} != {e}");
        }
        assert_eq!(shared.overflows.load(Relaxed), 0);
    }

    #[test]
    fn push_input_converts_u16() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(8);
        push_input(&[32768u16], &mut prod, &shared);
        let got = pop_all(&mut cons);
        assert_eq!(got.len(), 1);
        assert!(got[0].abs() < 1e-3, "{}", got[0]);
    }

    #[test]
    fn push_input_overflow_drops_whole_buffer() {
        let shared = Shared::new();
        let (mut prod, cons) = ring(4);
        push_input(&[0.1f32, 0.2, 0.3], &mut prod, &shared);
        push_input(&[0.4f32, 0.5], &mut prod, &shared);
        assert_eq!(shared.overflows.load(Relaxed), 2);
        assert_eq!(cons.occupied_len(), 3);
    }

    #[test]
    fn fill_output_no_underruns_before_first_sample() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(8);
        let mut started = false;
        let mut out = [9.0f32; 4];
        fill_output(&mut out, 2, &mut cons, &mut started, &shared);
        assert_eq!(out, [0.0; 4]);
        assert!(!started);
        assert_eq!(shared.underruns.load(Relaxed), 0);

        prod.push_slice(&[0.5]);
        fill_output(&mut out, 2, &mut cons, &mut started, &shared);
        assert_eq!(out, [0.5, 0.5, 0.0, 0.0]);
        assert!(started);
        assert_eq!(shared.underruns.load(Relaxed), 1);

        fill_output(&mut out, 2, &mut cons, &mut started, &shared);
        assert_eq!(shared.underruns.load(Relaxed), 3);
    }

    #[test]
    fn fill_output_duplicates_to_channels() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(8);
        prod.push_slice(&[0.5, -0.5]);
        let mut out = [9.0f32; 4];
        fill_output(&mut out, 2, &mut cons, &mut false, &shared);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
        assert_eq!(shared.underruns.load(Relaxed), 0);
    }

    #[test]
    fn fill_output_underrun_fills_silence() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(8);
        prod.push_slice(&[0.25]);
        let mut out = [9.0f32; 6];
        fill_output(&mut out, 2, &mut cons, &mut false, &shared);
        assert_eq!(out, [0.25, 0.25, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(shared.underruns.load(Relaxed), 2);
    }

    #[test]
    fn fill_output_i16() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(8);
        prod.push_slice(&[1.0]);
        let mut out = [0i16; 1];
        fill_output(&mut out, 1, &mut cons, &mut false, &shared);
        assert_eq!(out, [i16::MAX]);
    }

    #[test]
    fn push_output_respects_backlog() {
        let shared = Shared::new();
        let (mut prod, cons) = ring(16);
        push_output(&mut prod, &[0.1, 0.2, 0.3], 4, &shared);
        push_output(&mut prod, &[0.4, 0.5, 0.6], 4, &shared);
        assert_eq!(shared.overflows.load(Relaxed), 3);
        assert_eq!(cons.occupied_len(), 3);
    }

    #[test]
    fn push_block_primes_once() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(48_000);
        let mut primed = false;
        push_block(&mut prod, &[0.5; 10], &mut primed, 1920, 6720, &shared);
        assert!(primed);
        let got = pop_all(&mut cons);
        assert_eq!(got.len(), 1930);
        assert!(got[..1920].iter().all(|&s| s == 0.0));
        assert_eq!(got[1920..], [0.5; 10]);

        push_block(&mut prod, &[0.25; 10], &mut primed, 1920, 6720, &shared);
        assert_eq!(pop_all(&mut cons), [0.25; 10]);
        assert_eq!(shared.overflows.load(Relaxed), 0);
    }

    #[test]
    fn push_block_empty_does_not_prime() {
        let shared = Shared::new();
        let (mut prod, cons) = ring(48_000);
        let mut primed = false;
        push_block(&mut prod, &[], &mut primed, 1920, 6720, &shared);
        assert!(!primed);
        assert_eq!(cons.occupied_len(), 0);
    }

    #[test]
    fn push_block_allows_full_block() {
        let shared = Shared::new();
        let (mut prod, mut cons) = ring(2 * 48_000);
        let mut primed = true;
        let block = vec![0.1; 12_000];
        push_block(&mut prod, &block, &mut primed, 1920, 6720, &shared);
        assert_eq!(cons.occupied_len(), 12_000);
        assert_eq!(shared.overflows.load(Relaxed), 0);

        cons.skip(12_000 - 9600);
        push_block(&mut prod, &block, &mut primed, 1920, 6720, &shared);
        assert_eq!(cons.occupied_len(), 9600);
        assert_eq!(shared.overflows.load(Relaxed), 12_000);
    }

    fn timestamp(callback_ns: u32, playback_ns: u32) -> cpal::OutputStreamTimestamp {
        cpal::OutputStreamTimestamp {
            callback: cpal::StreamInstant::new(1, callback_ns),
            playback: cpal::StreamInstant::new(1, playback_ns),
        }
    }

    #[test]
    fn device_latency_from_timestamps() {
        let shared = Shared::new();
        store_device_latency(&timestamp(0, 25_000_000), &shared);
        assert_eq!(shared.device_latency_us.load(Relaxed), 25_000);

        store_device_latency(&timestamp(25_000_000, 0), &shared);
        assert_eq!(shared.device_latency_us.load(Relaxed), 25_000);
    }
}
