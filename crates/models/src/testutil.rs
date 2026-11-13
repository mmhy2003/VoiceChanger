//! Test helpers: a temporary folder removed on drop.

use std::path::{Path, PathBuf};

/// `temp_dir()/vc-models-test-{pid}-{name}`, created empty and removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vc-models-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
