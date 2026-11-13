//! Real-time streaming: block sizes, live controls and per-block stats shared with the audio and UI
//! threads. No audio device code lives here.

pub mod bench;
pub mod settings;
pub mod sola;
pub mod stats;
pub mod streamer;

pub use bench::{StreamReport, stream_file};
pub use settings::{Controls, StreamSettings};
pub use sola::Sola;
pub use stats::StreamStats;
pub use streamer::{Convert, Streamer};
