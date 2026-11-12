//! Downloads, verifies and extracts the pinned files of `assets/base-models.toml`.

use std::fs;
use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::manifest::Manifest;

/// The SHA-256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `Ok(false)` if the file is missing, `Ok(true)` if it matches `want`.
/// A mismatching file is deleted and reported as an error.
pub fn verify_or_delete(path: &Path, want: &str) -> Result<bool, String> {
    if !path.exists() {
        return Ok(false);
    }
    let got = sha256_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if got == want {
        return Ok(true);
    }
    fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Err(format!(
        "hash mismatch for {} (expected {want}, got {got}); file deleted",
        path.display()
    ))
}

/// Copies the file `inner` out of the zip archive `zip` to `dest`.
pub fn extract_from_zip(zip: &Path, inner: &str, dest: &Path) -> Result<(), String> {
    let file = fs::File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut entry = archive
        .by_name(inner)
        .map_err(|e| format!("{}: {inner}: {e}", zip.display()))?;
    create_parent(dest)?;
    let mut out = fs::File::create(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(())
}

/// Streams `url` to `dest` + ".part", then renames it to `dest`.
pub fn download(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<(), String> {
    create_parent(dest)?;
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = Path::new(&part);
    let resp = agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut out = fs::File::create(part).map_err(|e| format!("{}: {e}", part.display()))?;
    io::copy(&mut resp.into_body().into_reader(), &mut out).map_err(|e| format!("{url}: {e}"))?;
    drop(out);
    fs::rename(part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Fetches every model into `root/vendor/<dest>` and extracts every runtime
/// DLL from its verified package in `root/vendor/cache`.
pub fn fetch_all(root: &Path, manifest: &Manifest) -> Result<(), String> {
    let vendor = root.join("vendor");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                // The Windows certificate store, so no bundled root list is needed.
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into();
    for entry in &manifest.model {
        ensure(
            &agent,
            &entry.name,
            &entry.url,
            &entry.sha256,
            &vendor.join(&entry.dest),
        )?;
    }
    for entry in &manifest.runtime {
        let file_name = entry.url.rsplit('/').next().unwrap_or_default();
        let package = vendor.join("cache").join(file_name);
        ensure(&agent, &entry.name, &entry.url, &entry.sha256, &package)?;
        let inner = entry.extract.as_deref().unwrap_or_default();
        extract_from_zip(&package, inner, &vendor.join(&entry.dest))?;
    }
    Ok(())
}

/// Downloads `path` unless it already verifies, then verifies the download.
fn ensure(
    agent: &ureq::Agent,
    name: &str,
    url: &str,
    sha256: &str,
    path: &Path,
) -> Result<(), String> {
    if verify_or_delete(path, sha256)? {
        println!("ok  {name}");
        return Ok(());
    }
    println!("get {name}");
    download(agent, url, path)?;
    verify_or_delete(path, sha256)?;
    Ok(())
}

fn create_parent(path: &Path) -> Result<(), String> {
    match path.parent() {
        Some(dir) => fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display())),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("vc-xtask-{}-{name}", std::process::id()))
    }

    fn write_temp(name: &str, contents: &[u8]) -> PathBuf {
        let path = temp(name);
        fs::write(&path, contents).unwrap();
        path
    }

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_of_abc() {
        let path = write_temp("sha-abc", b"abc");
        assert_eq!(sha256_file(&path).unwrap(), ABC);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn verify_missing_is_false() {
        assert_eq!(verify_or_delete(&temp("missing"), ABC), Ok(false));
    }

    #[test]
    fn verify_match_is_true() {
        let path = write_temp("match", b"abc");
        assert_eq!(verify_or_delete(&path, ABC), Ok(true));
        assert!(path.exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn verify_mismatch_deletes() {
        let path = write_temp("mismatch", b"abd");
        let err = verify_or_delete(&path, ABC).unwrap_err();
        assert!(err.contains(ABC) && err.contains("deleted"), "{err}");
        assert!(!path.exists());
    }

    fn write_zip(name: &str) -> PathBuf {
        let path = temp(name);
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("a/b.dll", options).unwrap();
        zip.write_all(b"hi").unwrap();
        zip.finish().unwrap();
        path
    }

    #[test]
    fn extracts_inner_file() {
        let zip = write_zip("extract.zip");
        let dest = temp("extract-out").join("sub/b.dll");
        extract_from_zip(&zip, "a/b.dll", &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"hi");
        fs::remove_file(zip).unwrap();
        fs::remove_dir_all(temp("extract-out")).unwrap();
    }

    #[test]
    fn extract_missing_inner_errors() {
        let zip = write_zip("missing.zip");
        let dest = temp("missing-out.dll");
        let err = extract_from_zip(&zip, "a/c.dll", &dest).unwrap_err();
        assert!(err.contains("a/c.dll"), "{err}");
        assert!(!dest.exists());
        fs::remove_file(zip).unwrap();
    }
}
