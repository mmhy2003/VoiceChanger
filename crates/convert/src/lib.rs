//! Conversion of PyTorch `.pth` RVC v2 models to ONNX.

pub mod checkpoint;
mod error;
pub mod onnx;
pub mod pickle;
pub mod pth;
pub mod template;
#[cfg(test)]
mod testutil;

pub use error::ConvertError;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Name and version written into converted models' metadata.
pub const CONVERTER: &str = concat!("vc-convert ", env!("CARGO_PKG_VERSION"));

/// A cancel flag shared between the UI and a conversion worker.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once [`Cancel::cancel`] has been called on any clone.
    pub fn check(&self) -> Result<(), ConvertError> {
        if self.is_cancelled() {
            Err(ConvertError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The stages of a conversion, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Fill,
    Check,
    Save,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Read, Stage::Fill, Stage::Check, Stage::Save];

    /// Position of the stage in [`Stage::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Progress of a conversion: the current stage and how far it is (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub stage: Stage,
    pub fraction: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_is_shared_between_clones() {
        let cancel = Cancel::new();
        let clone = cancel.clone();
        assert!(!clone.is_cancelled());
        assert!(clone.check().is_ok());
        cancel.cancel();
        assert!(clone.is_cancelled());
        assert!(matches!(clone.check(), Err(ConvertError::Cancelled)));
    }

    #[test]
    fn stage_index_order() {
        let indices: Vec<usize> = Stage::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(indices, [0, 1, 2, 3]);
        assert_eq!(
            Stage::ALL,
            [Stage::Read, Stage::Fill, Stage::Check, Stage::Save]
        );
    }
}
