//! Session state shared by the UI, the worker and the audio callbacks.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};

use crate::meter::{LevelMeter, db_to_gain};

/// One of the session's audio endpoints.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint {
    /// The capture device (microphone).
    Input,
    /// The virtual cable playback device.
    Cable,
    /// The optional monitor playback device.
    Monitor,
}

/// State of the running session, as reported by the audio callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamStatus {
    /// All streams are running.
    Running,
    /// A device was unplugged or disabled.
    DeviceLost {
        /// The endpoint that was lost.
        which: Endpoint,
    },
    /// A stream failed for another reason.
    Failed {
        /// The endpoint whose stream failed.
        which: Endpoint,
    },
}

impl StreamStatus {
    // u8 codes: 0 Running; 1/2/3 DeviceLost Input/Cable/Monitor; 4/5/6 Failed Input/Cable/Monitor.
    fn to_code(self) -> u8 {
        let endpoint = |which| match which {
            Endpoint::Input => 0,
            Endpoint::Cable => 1,
            Endpoint::Monitor => 2,
        };
        match self {
            Self::Running => 0,
            Self::DeviceLost { which } => 1 + endpoint(which),
            Self::Failed { which } => 4 + endpoint(which),
        }
    }

    /// Unknown codes map to `Running`.
    fn from_code(code: u8) -> Self {
        const ENDPOINTS: [Endpoint; 3] = [Endpoint::Input, Endpoint::Cable, Endpoint::Monitor];
        match code {
            1..=3 => Self::DeviceLost {
                which: ENDPOINTS[usize::from(code - 1)],
            },
            4..=6 => Self::Failed {
                which: ENDPOINTS[usize::from(code - 4)],
            },
            _ => Self::Running,
        }
    }
}

/// Default monitor volume in dB.
pub const DEFAULT_MONITOR_DB: f32 = -6.0;

/// Shared by the UI, the worker and the audio callbacks (behind an `Arc`). Lock-free.
#[derive(Debug)]
pub struct Shared {
    /// Level of the mic signal.
    pub input_meter: LevelMeter,
    /// Level of the signal sent to the cable.
    pub output_meter: LevelMeter,
    monitor_gain: AtomicU32, // linear gain, f32 bits
    status: AtomicU8,
    /// Samples dropped because a ring was full.
    pub overflows: AtomicU64,
    /// Output frames filled with silence because a ring was empty.
    pub underruns: AtomicU64,
}

impl Shared {
    /// Status `Running`, meters and counters zero, monitor gain at `DEFAULT_MONITOR_DB`.
    pub fn new() -> Self {
        Self {
            input_meter: LevelMeter::default(),
            output_meter: LevelMeter::default(),
            monitor_gain: AtomicU32::new(db_to_gain(DEFAULT_MONITOR_DB).to_bits()),
            status: AtomicU8::new(StreamStatus::Running.to_code()),
            overflows: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
        }
    }

    /// Linear monitor gain.
    pub fn monitor_gain(&self) -> f32 {
        f32::from_bits(self.monitor_gain.load(Relaxed))
    }

    /// Sets the linear monitor gain; takes effect on the next processed block.
    pub fn set_monitor_gain(&self, gain: f32) {
        self.monitor_gain.store(gain.to_bits(), Relaxed);
    }

    /// Current session status.
    pub fn status(&self) -> StreamStatus {
        StreamStatus::from_code(self.status.load(Relaxed))
    }

    /// Records an error. The first error wins; later reports are ignored.
    pub fn report(&self, status: StreamStatus) {
        let _ = self
            .status
            .compare_exchange(0, status.to_code(), Relaxed, Relaxed);
    }

    /// Before each session: status `Running`, meters zero, counters zero. Monitor gain is kept.
    pub fn reset(&self) {
        self.status.store(StreamStatus::Running.to_code(), Relaxed);
        self.input_meter.reset();
        self.output_meter.reset();
        self.overflows.store(0, Relaxed);
        self.underruns.store(0, Relaxed);
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meter::Level;
    use std::sync::atomic::Ordering::Relaxed;

    const ALL: [StreamStatus; 7] = [
        StreamStatus::Running,
        StreamStatus::DeviceLost {
            which: Endpoint::Input,
        },
        StreamStatus::DeviceLost {
            which: Endpoint::Cable,
        },
        StreamStatus::DeviceLost {
            which: Endpoint::Monitor,
        },
        StreamStatus::Failed {
            which: Endpoint::Input,
        },
        StreamStatus::Failed {
            which: Endpoint::Cable,
        },
        StreamStatus::Failed {
            which: Endpoint::Monitor,
        },
    ];

    #[test]
    fn status_codes_round_trip() {
        for (i, status) in ALL.into_iter().enumerate() {
            assert_eq!(status.to_code(), i as u8);
            assert_eq!(StreamStatus::from_code(status.to_code()), status);
        }
        assert_eq!(StreamStatus::from_code(200), StreamStatus::Running);
    }

    #[test]
    fn first_error_wins() {
        let shared = Shared::new();
        shared.report(StreamStatus::DeviceLost {
            which: Endpoint::Input,
        });
        shared.report(StreamStatus::Failed {
            which: Endpoint::Cable,
        });
        assert_eq!(
            shared.status(),
            StreamStatus::DeviceLost {
                which: Endpoint::Input
            }
        );
    }

    #[test]
    fn reset_clears_status_meters_counters_keeps_gain() {
        let shared = Shared::new();
        let level = Level {
            peak: 0.5,
            rms: 0.25,
        };
        shared.input_meter.store(level);
        shared.output_meter.store(level);
        shared.overflows.store(3, Relaxed);
        shared.underruns.store(4, Relaxed);
        shared.set_monitor_gain(0.3);
        shared.report(StreamStatus::Failed {
            which: Endpoint::Monitor,
        });

        shared.reset();

        assert_eq!(shared.status(), StreamStatus::Running);
        assert_eq!(shared.input_meter.load(), Level::default());
        assert_eq!(shared.output_meter.load(), Level::default());
        assert_eq!(shared.overflows.load(Relaxed), 0);
        assert_eq!(shared.underruns.load(Relaxed), 0);
        assert_eq!(shared.monitor_gain(), 0.3);
    }

    #[test]
    fn default_monitor_gain_is_minus_6_db() {
        let shared = Shared::default();
        assert_eq!(shared.monitor_gain(), db_to_gain(DEFAULT_MONITOR_DB));
        assert!((shared.monitor_gain() - 0.501).abs() < 1e-3);
        assert_eq!(shared.status(), StreamStatus::Running);
    }
}
