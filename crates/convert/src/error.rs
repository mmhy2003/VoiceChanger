//! Conversion errors and the messages shown to the user.

use std::io;

/// Why a conversion failed. `Display` gives the details for the log;
/// [`ConvertError::user_message`] gives the short text for the user.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("not a PyTorch zip archive")]
    NotPyTorch,
    #[error("unsafe or unsupported pickle content: {0}")]
    UnsafeContent(String),
    #[error("RVC v1 model")]
    RvcV1,
    #[error("training checkpoint")]
    TrainingCheckpoint,
    #[error("model without F0 (f0 = 0)")]
    NoF0,
    #[error("the zip holds several inference models")]
    SeveralModels,
    #[error("no matching template: {0}")]
    NoTemplate(String),
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    #[error("not enough disk space")]
    DiskFull,
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Io(io::Error),
}

impl From<io::Error> for ConvertError {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::StorageFull {
            Self::DiskFull
        } else {
            Self::Io(error)
        }
    }
}

impl ConvertError {
    /// The message for the user, from the PRD §5.8 error table.
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NotPyTorch => "This file isn't a PyTorch model.",
            Self::UnsafeContent(_) => {
                "This file contains code the app won't run, so it can't be converted. It may be unsafe."
            }
            Self::RvcV1 => "RVC v1 models aren't supported. Use an RVC v2 model.",
            Self::TrainingCheckpoint => {
                "This is a training checkpoint. Export the model's weights file from RVC and convert that instead."
            }
            Self::NoF0 | Self::NoTemplate(_) => "This model's settings aren't supported yet.",
            Self::SeveralModels => {
                "This zip holds several models; extract the one you want and convert that."
            }
            Self::ValidationFailed(_) => {
                "The converted model didn't pass the checks and wasn't added."
            }
            Self::DiskFull => "Not enough disk space to save the converted model.",
            Self::Cancelled => "The conversion was cancelled.",
            Self::Io(_) => "The file couldn't be read or written. Details are in the log.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn user_message_matches_prd_table() {
        let unsupported = "This model's settings aren't supported yet.";
        let cases = [
            (ConvertError::NotPyTorch, "This file isn't a PyTorch model."),
            (
                ConvertError::UnsafeContent("os.system".into()),
                "This file contains code the app won't run, so it can't be converted. It may be unsafe.",
            ),
            (
                ConvertError::RvcV1,
                "RVC v1 models aren't supported. Use an RVC v2 model.",
            ),
            (
                ConvertError::TrainingCheckpoint,
                "This is a training checkpoint. Export the model's weights file from RVC and convert that instead.",
            ),
            (ConvertError::NoF0, unsupported),
            (ConvertError::NoTemplate("sr=22050".into()), unsupported),
            (
                ConvertError::SeveralModels,
                "This zip holds several models; extract the one you want and convert that.",
            ),
            (
                ConvertError::ValidationFailed("NaN".into()),
                "The converted model didn't pass the checks and wasn't added.",
            ),
            (
                ConvertError::DiskFull,
                "Not enough disk space to save the converted model.",
            ),
            (ConvertError::Cancelled, "The conversion was cancelled."),
            (
                ConvertError::Io(io::Error::other("boom")),
                "The file couldn't be read or written. Details are in the log.",
            ),
        ];
        for (error, message) in cases {
            assert_eq!(error.user_message(), message, "{error:?}");
        }
    }

    #[test]
    fn storage_full_io_is_disk_full() {
        let error = ConvertError::from(io::Error::from(io::ErrorKind::StorageFull));
        assert!(matches!(error, ConvertError::DiskFull), "{error:?}");
    }

    #[test]
    fn other_io_stays_io() {
        let error = ConvertError::from(io::Error::from(io::ErrorKind::NotFound));
        match error {
            ConvertError::Io(inner) => assert_eq!(inner.kind(), io::ErrorKind::NotFound),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn display_keeps_details() {
        assert_eq!(
            ConvertError::NoTemplate("x".into()).to_string(),
            "no matching template: x"
        );
    }
}
