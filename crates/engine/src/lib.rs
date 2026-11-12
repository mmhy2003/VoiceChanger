//! RVC v2 inference pipeline: ONNX Runtime sessions, DSP, F0 and offline conversion.

pub mod assets;
pub mod error;
#[cfg(test)]
mod testutil;

pub use error::{EngineError, Result};
