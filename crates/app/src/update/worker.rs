//! The update work that runs off the UI thread: checking for a release, the verified download and
//! removing old installers.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::client::{self, DownloadError};
use super::release::{self, CHECKSUMS, Update};
use super::verify::{self, Signature};
use super::version::Version;

/// The release JSON is a few KB; this only guards against a runaway response.
const MAX_RELEASE_JSON: u64 = 2 * 1024 * 1024;
const MAX_CHECKSUMS: u64 = 64 * 1024;

pub struct Agents {
    pub api: ureq::Agent,
    pub download: ureq::Agent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    Available(Update),
    UpToDate,
    NotInstallable(String),
    Failed(String),
}

/// Asks `url` for the latest release. Drafts, prereleases, odd tags and versions not newer than
/// `current` are UpToDate; a newer release without the installer or checksums is NotInstallable.
pub fn check(agent: &ureq::Agent, url: &str, current: Version) -> CheckOutcome {
    let outcome = client::fetch_text(agent, url, MAX_RELEASE_JSON)
        .and_then(|json| release::parse_release(&json))
        .map_or_else(CheckOutcome::Failed, |release| match release {
            Some(r) if r.version > current => {
                let version = r.version;
                match r.into_update() {
                    Ok(update) => CheckOutcome::Available(update),
                    Err(e) => CheckOutcome::NotInstallable(format!("VoiceChanger {version}: {e}")),
                }
            }
            _ => CheckOutcome::UpToDate,
        });
    match &outcome {
        CheckOutcome::Failed(e) => tracing::warn!("update check failed: {e}"),
        CheckOutcome::NotInstallable(e) => tracing::warn!("no installable update: {e}"),
        _ => {}
    }
    outcome
}

/// Downloads the installer into `dir` and returns its final path once its SHA-256 matches
/// `SHA256SUMS.txt` and, when the running app is validly signed, its signature is valid too.
/// A file that fails a check is deleted. An already verified installer in `dir` is reused.
/// `progress` gets (bytes so far, installer size).
pub fn download_update(
    agents: &Agents,
    update: &Update,
    dir: &Path,
    running: Signature,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, DownloadError> {
    let name = &update.installer.name;
    fs::create_dir_all(dir)
        .map_err(|e| DownloadError::Failed(format!("{}: {e}", dir.display())))?;
    let sums = client::fetch_text(
        &agents.download,
        &update.checksums.browser_download_url,
        MAX_CHECKSUMS,
    )
    .map_err(DownloadError::Failed)?;
    let expected = release::parse_sums(&sums, name)
        .ok_or_else(|| DownloadError::Failed(format!("{CHECKSUMS} has no line for {name}")))?;

    let path = dir.join(name);
    if path.exists() {
        if verify::sha256_file(&path).is_ok_and(|hash| hash == expected) {
            return Ok(path);
        }
        let _ = fs::remove_file(&path);
    }

    let part = dir.join(format!("{name}.part"));
    let size = update.installer.size;
    let hash = client::download(
        &agents.download,
        &update.installer.browser_download_url,
        &part,
        size,
        cancel,
        &mut |done| progress(done, size),
    )?;
    let reject = |reason: String| {
        let _ = fs::remove_file(&part);
        DownloadError::Failed(reason)
    };
    if hash != expected {
        return Err(reject(format!(
            "{name} did not match {CHECKSUMS} and was deleted."
        )));
    }
    if let Err(e) = verify::signature_ok(running, verify::signature(&part)) {
        return Err(reject(format!("{name} {e} and was deleted.")));
    }
    fs::rename(&part, &path).map_err(|e| reject(format!("{}: {e}", path.display())))?;
    Ok(path)
}

/// Deletes leftover `.part` files and installers for `current` or older. Other files are kept.
/// A missing `dir` is fine.
pub fn cleanup(dir: &Path, current: Version) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let old_installer = name
            .strip_prefix("Setup-VoiceChanger-")
            .and_then(|rest| rest.strip_suffix(".exe"))
            .and_then(Version::parse)
            .is_some_and(|v| v <= current);
        if (old_installer || name.ends_with(".part"))
            && let Err(e) = fs::remove_file(entry.path())
        {
            tracing::warn!("cannot delete {}: {e}", entry.path().display());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::testutil::TempDir;
    use crate::update::client::{self, hex};
    use crate::update::release::Asset;
    use crate::update::testserver::{Route, TestServer};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn route(path: &'static str, status: u16, body: Vec<u8>) -> Route {
        Route {
            path,
            status,
            body,
            content_length: None,
        }
    }

    fn agents() -> Agents {
        Agents {
            api: client::api_agent(),
            download: client::download_agent(),
        }
    }

    fn release_json(tag: &str, prerelease: bool, assets: &[&str]) -> Vec<u8> {
        let assets: Vec<String> = assets
            .iter()
            .map(|name| {
                format!(
                    r#"{{"name":"{name}","browser_download_url":"https://example.invalid/{name}","size":10}}"#
                )
            })
            .collect();
        format!(
            r#"{{"tag_name":"{tag}","name":null,"body":"notes","published_at":"2026-11-01T00:00:00Z","prerelease":{prerelease},"assets":[{}]}}"#,
            assets.join(",")
        )
        .into_bytes()
    }

    fn check_tag(tag: &str, prerelease: bool, assets: &[&str]) -> CheckOutcome {
        let server = TestServer::start(vec![route(
            "/latest",
            200,
            release_json(tag, prerelease, assets),
        )]);
        check(&agents().api, &server.url("/latest"), v("1.0.0"))
    }

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    fn sums_for(data: &[u8]) -> Vec<u8> {
        format!(
            "{}  Setup-VoiceChanger-1.1.0.exe\n",
            hex(&Sha256::digest(data))
        )
        .into_bytes()
    }

    fn update_for(server: &TestServer, body: &[u8], sums: &[u8]) -> Update {
        Update {
            version: v("1.1.0"),
            title: "VoiceChanger 1.1.0".into(),
            notes: String::new(),
            date: String::new(),
            installer: Asset {
                name: "Setup-VoiceChanger-1.1.0.exe".into(),
                browser_download_url: server.url("/setup"),
                size: body.len() as u64,
            },
            checksums: Asset {
                name: "SHA256SUMS.txt".into(),
                browser_download_url: server.url("/sums"),
                size: sums.len() as u64,
            },
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn run(
        server: &TestServer,
        data: &[u8],
        sums: &[u8],
        dir: &Path,
        running: Signature,
        cancel: bool,
    ) -> (Result<PathBuf, DownloadError>, Vec<(u64, u64)>) {
        let update = update_for(server, data, sums);
        let mut calls = Vec::new();
        let got = download_update(
            &agents(),
            &update,
            dir,
            running,
            &AtomicBool::new(cancel),
            &mut |done, total| calls.push((done, total)),
        );
        (got, calls)
    }

    #[test]
    fn check_newer_available() {
        let got = check_tag(
            "v9.0.0",
            false,
            &["Setup-VoiceChanger-9.0.0.exe", "SHA256SUMS.txt"],
        );
        match got {
            CheckOutcome::Available(u) => {
                assert_eq!(u.version, v("9.0.0"));
                assert_eq!(u.installer.name, "Setup-VoiceChanger-9.0.0.exe");
                assert_eq!(u.checksums.name, "SHA256SUMS.txt");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn check_same_or_older_up_to_date() {
        for (tag, pre) in [("v1.0.0", false), ("v0.9.0", false), ("v9.0.0", true)] {
            let assets = [
                format!("Setup-VoiceChanger-{}.exe", &tag[1..]),
                "SHA256SUMS.txt".to_owned(),
            ];
            let assets: Vec<&str> = assets.iter().map(String::as_str).collect();
            assert_eq!(
                check_tag(tag, pre, &assets),
                CheckOutcome::UpToDate,
                "{tag}"
            );
        }
    }

    #[test]
    fn check_newer_without_assets_not_installable() {
        match check_tag("v9.0.0", false, &["SHA256SUMS.txt"]) {
            CheckOutcome::NotInstallable(msg) => {
                assert!(msg.contains("9.0.0"), "{msg}");
                assert!(msg.contains("Setup-VoiceChanger-9.0.0.exe"), "{msg}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn check_failures() {
        let server = TestServer::start(vec![
            route("/forbidden", 403, Vec::new()),
            route("/oops", 200, b"oops".to_vec()),
        ]);
        for path in ["/forbidden", "/oops"] {
            let got = check(&agents().api, &server.url(path), v("1.0.0"));
            assert!(matches!(got, CheckOutcome::Failed(_)), "{path}: {got:?}");
        }
    }

    #[test]
    fn download_verified_and_renamed() {
        let dir = TempDir::new("worker-ok");
        let data = body(200 * 1024);
        let sums = sums_for(&data);
        let server = TestServer::start(vec![
            route("/setup", 200, data.clone()),
            route("/sums", 200, sums.clone()),
        ]);
        let (got, calls) = run(
            &server,
            &data,
            &sums,
            dir.path(),
            Signature::Unsigned,
            false,
        );
        let path = dir.path().join("Setup-VoiceChanger-1.1.0.exe");
        assert_eq!(got, Ok(path.clone()));
        assert_eq!(std::fs::read(&path).unwrap(), data);
        assert_eq!(files(dir.path()), ["Setup-VoiceChanger-1.1.0.exe"]);
        let len = data.len() as u64;
        assert_eq!(calls.last(), Some(&(len, len)));
    }

    #[test]
    fn download_hash_mismatch_deleted() {
        let dir = TempDir::new("worker-mismatch");
        let data = body(10 * 1024);
        let sums = sums_for(b"something else");
        let server = TestServer::start(vec![
            route("/setup", 200, data.clone()),
            route("/sums", 200, sums.clone()),
        ]);
        let (got, _) = run(
            &server,
            &data,
            &sums,
            dir.path(),
            Signature::Unsigned,
            false,
        );
        match got {
            Err(DownloadError::Failed(e)) => {
                assert!(
                    e.ends_with("did not match SHA256SUMS.txt and was deleted."),
                    "{e}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(files(dir.path()).is_empty());
    }

    #[test]
    fn download_sums_missing_line() {
        let dir = TempDir::new("worker-noline");
        let data = body(1024);
        let sums = format!("{}  other.exe\n", "0".repeat(64)).into_bytes();
        let server = TestServer::start(vec![
            route("/setup", 200, data.clone()),
            route("/sums", 200, sums.clone()),
        ]);
        let (got, _) = run(
            &server,
            &data,
            &sums,
            dir.path(),
            Signature::Unsigned,
            false,
        );
        assert_eq!(
            got,
            Err(DownloadError::Failed(
                "SHA256SUMS.txt has no line for Setup-VoiceChanger-1.1.0.exe".into()
            ))
        );
    }

    #[test]
    fn download_signed_app_rejects_unsigned_installer() {
        let dir = TempDir::new("worker-unsigned");
        let data = body(10 * 1024);
        let sums = sums_for(&data);
        let server = TestServer::start(vec![
            route("/setup", 200, data.clone()),
            route("/sums", 200, sums.clone()),
        ]);
        let (got, _) = run(&server, &data, &sums, dir.path(), Signature::Valid, false);
        match got {
            Err(DownloadError::Failed(e)) => assert!(e.contains("is not validly signed"), "{e}"),
            other => panic!("{other:?}"),
        }
        assert!(files(dir.path()).is_empty());
    }

    #[test]
    fn download_reuses_verified_file() {
        let dir = TempDir::new("worker-reuse");
        let data = body(10 * 1024);
        let sums = sums_for(&data);
        let path = dir.path().join("Setup-VoiceChanger-1.1.0.exe");
        std::fs::write(&path, &data).unwrap();
        // No /setup route: a download attempt would fail with a 404.
        let server = TestServer::start(vec![route("/sums", 200, sums.clone())]);
        let (got, _) = run(
            &server,
            &data,
            &sums,
            dir.path(),
            Signature::Unsigned,
            false,
        );
        assert_eq!(got, Ok(path.clone()));

        std::fs::write(&path, b"wrong").unwrap();
        let server = TestServer::start(vec![
            route("/setup", 200, data.clone()),
            route("/sums", 200, sums.clone()),
        ]);
        let (got, _) = run(
            &server,
            &data,
            &sums,
            dir.path(),
            Signature::Unsigned,
            false,
        );
        assert_eq!(got, Ok(path.clone()));
        assert_eq!(std::fs::read(&path).unwrap(), data);
    }

    #[test]
    fn download_cancel() {
        let dir = TempDir::new("worker-cancel");
        let data = body(10 * 1024);
        let sums = sums_for(&data);
        let server = TestServer::start(vec![
            route("/setup", 200, data.clone()),
            route("/sums", 200, sums.clone()),
        ]);
        let (got, _) = run(&server, &data, &sums, dir.path(), Signature::Unsigned, true);
        assert_eq!(got, Err(DownloadError::Cancelled));
        assert!(
            !dir.path()
                .join("Setup-VoiceChanger-1.1.0.exe.part")
                .exists()
        );
    }

    #[test]
    fn cleanup_removes_old_and_parts() {
        let dir = TempDir::new("worker-cleanup");
        for name in [
            "Setup-VoiceChanger-0.9.0.exe",
            "Setup-VoiceChanger-1.0.0.exe",
            "Setup-VoiceChanger-1.1.0.exe",
            "x.part",
            "notes.txt",
        ] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        cleanup(dir.path(), v("1.0.0"));
        assert_eq!(
            files(dir.path()),
            ["Setup-VoiceChanger-1.1.0.exe", "notes.txt"]
        );
        cleanup(&dir.path().join("missing"), v("1.0.0"));
    }
}
