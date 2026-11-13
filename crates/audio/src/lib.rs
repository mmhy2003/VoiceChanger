//! Audio device handling, ring buffers and resampling.

mod com;
pub mod control;
pub mod devices;
pub mod meter;
pub mod processor;
pub mod ring;
pub mod session;
pub mod shared;
pub mod worker;

pub use control::AudioControl;
pub use devices::{
    DeviceInfo, DeviceList, DeviceRef, Selection, Warning, feedback_warnings, is_virtual_cable,
    list_devices, start_blocker,
};
pub use meter::{Level, db_to_gain, gain_to_db};
pub use processor::{Passthrough, Processor};
pub use session::{AudioSession, SessionInfo, StartError, StartFailure};
pub use shared::{DEFAULT_MONITOR_DB, Endpoint, Shared, StreamStatus};
