//! Engine error type.

use std::path::PathBuf;

/// Errors from the inference engine.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("missing {}: run `cargo xtask fetch-models`", path.display())]
    MissingAsset { path: PathBuf },
    #[error("ONNX Runtime: {0}")]
    RuntimeInit(String),
    #[error("unsupported model: {reason}")]
    UnsupportedModel { reason: String },
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("WAV: {0}")]
    Wav(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
