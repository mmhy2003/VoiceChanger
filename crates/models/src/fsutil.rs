//! Small filesystem helpers: atomic writes and `.bad` backups.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Writes `bytes` to `path` through `path.tmp` and a rename, creating the parent folder.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = with_suffix(path, ".tmp");
    let written = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Renames `path` to `path.bad` (replacing an older backup) and returns the backup path.
pub fn back_up_bad(path: &Path) -> io::Result<PathBuf> {
    let bad = with_suffix(path, ".bad");
    fs::rename(path, &bad)?;
    Ok(bad)
}

/// `path` with `suffix` appended to its file name.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn write_atomic_replaces() {
        let dir = TempDir::new("write-atomic");
        let path = dir.path().join("sub").join("a.toml");
        write_atomic(&path, b"one").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one");
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        assert!(!dir.path().join("sub").join("a.toml.tmp").exists());
    }

    #[test]
    fn back_up_bad_replaces_old_backup() {
        let dir = TempDir::new("back-up-bad");
        let path = dir.path().join("a.toml");
        std::fs::write(&path, "first").unwrap();
        let bad = back_up_bad(&path).unwrap();
        assert_eq!(bad, dir.path().join("a.toml.bad"));
        std::fs::write(&path, "second").unwrap();
        back_up_bad(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), "second");
    }
}
