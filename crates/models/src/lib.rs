//! Model library and per-model metadata.

pub mod fsutil;
pub mod library;
#[cfg(test)]
mod testutil;

pub use fsutil::{back_up_bad, write_atomic};
pub use library::{
    INDEX_FILE, Library, LibraryError, ModelEntry, StagedModel, copy_model, discard_staged, slug,
};
