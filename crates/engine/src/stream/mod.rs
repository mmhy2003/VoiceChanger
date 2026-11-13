//! Real-time streaming: block sizes, live controls and per-block stats shared with the audio and UI
//! threads. No audio device code lives here.

pub mod settings;
pub mod stats;

pub use settings::{Controls, StreamSettings};
pub use stats::StreamStats;
