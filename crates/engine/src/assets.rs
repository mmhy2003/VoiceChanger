//! Lookup of vendored assets (base models, ONNX Runtime DLLs) fetched by `cargo xtask fetch-models`.

use std::path::{Path, PathBuf};

use crate::error::{EngineError, Result};

pub const HUBERT: &str = "models/hubert.onnx";
pub const RMVPE: &str = "models/rmvpe.onnx";
pub const ORT_DLL: &str = "runtime/onnxruntime.dll";
pub const DML_DLL: &str = "runtime/DirectML.dll";

/// Directories searched for assets, in order: only `VC_ASSETS_DIR` when it is set,
/// otherwise `<exe dir>/vendor` and then the workspace `vendor/`.
pub fn candidates() -> Vec<PathBuf> {
    if let Some(dir) = std::env::var_os("VC_ASSETS_DIR") {
        return vec![PathBuf::from(dir)];
    }
    let mut dirs = Vec::new();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("vendor"));
    }
    dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor"));
    dirs
}

/// First `dir.join(rel)` that is a file. Otherwise `MissingAsset` naming the path
/// in the first dir, or `rel` itself when `dirs` is empty.
pub fn resolve(rel: &str, dirs: &[PathBuf]) -> Result<PathBuf> {
    if let Some(found) = dirs.iter().map(|d| d.join(rel)).find(|p| p.is_file()) {
        return Ok(found);
    }
    let path = dirs
        .first()
        .map_or_else(|| PathBuf::from(rel), |d| d.join(rel));
    Err(EngineError::MissingAsset { path })
}

/// Resolves `rel` against [`candidates`].
pub fn asset(rel: &str) -> Result<PathBuf> {
    resolve(rel, &candidates())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assets_or_skip, temp_path};
    use std::fs;

    fn dir_with(name: &str, files: &[&str]) -> PathBuf {
        let dir = temp_path(name);
        let _ = fs::remove_dir_all(&dir);
        for f in files {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, b"x").unwrap();
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn resolve_prefers_first_dir() {
        let a = dir_with("prefers-a", &[HUBERT]);
        let b = dir_with("prefers-b", &[HUBERT]);
        let got = resolve(HUBERT, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(got, a.join(HUBERT));
        let _ = (fs::remove_dir_all(a), fs::remove_dir_all(b));
    }

    #[test]
    fn resolve_falls_through() {
        let a = dir_with("falls-a", &[]);
        let b = dir_with("falls-b", &[RMVPE]);
        let got = resolve(RMVPE, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(got, b.join(RMVPE));
        let _ = (fs::remove_dir_all(a), fs::remove_dir_all(b));
    }

    #[test]
    fn missing_names_file_and_fix() {
        let a = dir_with("missing-a", &[]);
        let err = resolve(HUBERT, std::slice::from_ref(&a)).unwrap_err();
        assert!(matches!(&err, EngineError::MissingAsset { path } if path == &a.join(HUBERT)));
        let msg = err.to_string().replace('\\', "/");
        assert!(msg.contains("models/hubert.onnx"), "{msg}");
        assert!(msg.contains("cargo xtask fetch-models"), "{msg}");
        let _ = fs::remove_dir_all(a);
    }

    #[test]
    fn missing_with_no_dirs_names_rel() {
        let err = resolve(DML_DLL, &[]).unwrap_err();
        assert!(matches!(&err, EngineError::MissingAsset { path } if path == Path::new(DML_DLL)));
    }

    #[test]
    fn no_assets_needed_never_skips() {
        assert!(assets_or_skip(&[]));
    }
}
