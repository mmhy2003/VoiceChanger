//! HTTP for updates: the GitHub API call and the streamed, hashed installer download.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};

/// The newest published, non-draft, non-prerelease release of the app.
pub const LATEST_URL: &str = "https://api.github.com/repos/mmhy2003/VoiceChanger/releases/latest";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const API_TIMEOUT: Duration = Duration::from_secs(30);
const CHUNK: usize = 64 * 1024;

/// For small API and text requests: the whole call must finish within 30 s.
pub fn api_agent() -> ureq::Agent {
    base_config()
        .timeout_global(Some(API_TIMEOUT))
        .build()
        .into()
}

/// For the installer: no overall limit, since it is hundreds of MB, but the server must start answering
/// within 30 s.
pub fn download_agent() -> ureq::Agent {
    base_config()
        .timeout_recv_response(Some(API_TIMEOUT))
        .build()
        .into()
}

fn base_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                // The Windows certificate store, so no bundled root list is needed.
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .user_agent(format!("VoiceChanger/{}", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(CONNECT_TIMEOUT))
}

/// GETs `url` as text, at most `max` bytes. HTTP error statuses are errors.
pub fn fetch_text(agent: &ureq::Agent, url: &str, max: u64) -> Result<String, String> {
    agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .and_then(|mut resp| resp.body_mut().with_config().limit(max).read_to_string())
        .map_err(|e| format!("{url}: {e}"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum DownloadError {
    Cancelled,
    Failed(String),
}

/// Streams `url` into `part` in 64 KiB chunks, hashing while writing, and returns the SHA-256 as
/// lowercase hex. `cancel` is checked before each read; `progress` gets the byte count after each chunk.
/// Receiving other than `size` bytes is a failure. `part` is deleted on every error, including Cancelled.
pub fn download(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    size: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<String, DownloadError> {
    let result = stream(agent, url, part, size, cancel, progress);
    if result.is_err() {
        // Missing (the request failed before the file was created) is fine.
        let _ = fs::remove_file(part);
    }
    result
}

fn stream(
    agent: &ureq::Agent,
    url: &str,
    part: &Path,
    size: u64,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(u64),
) -> Result<String, DownloadError> {
    let failed = |e: &dyn std::fmt::Display| DownloadError::Failed(format!("{url}: {e}"));
    let resp = agent.get(url).call().map_err(|e| failed(&e))?;
    // No body limit: the reader is bounded by the size check below, not by memory.
    let mut reader = resp.into_body().into_reader();
    let write_failed = |e: io::Error| DownloadError::Failed(format!("{}: {e}", part.display()));
    let mut out = File::create(part).map_err(write_failed)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(failed(&e)),
        };
        out.write_all(&buf[..n]).map_err(write_failed)?;
        hasher.update(&buf[..n]);
        total += n as u64;
        progress(total);
    }
    out.flush().map_err(write_failed)?;
    if total != size {
        return Err(DownloadError::Failed(format!(
            "received {total} bytes, expected {size}"
        )));
    }
    Ok(hex(&hasher.finalize()))
}

/// Lowercase hex, two digits per byte.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::sync::atomic::Ordering;

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::testutil::TempDir;
    use crate::update::testserver::{Route, TestServer};

    fn route(path: &'static str, status: u16, body: Vec<u8>) -> Route {
        Route {
            path,
            status,
            body,
            content_length: None,
        }
    }

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn latest_url_is_the_github_api() {
        assert!(LATEST_URL.starts_with("https://api.github.com/repos/"));
        assert!(LATEST_URL.ends_with("/releases/latest"));
    }

    #[test]
    fn fetch_text_ok_sends_headers() {
        let server = TestServer::start(vec![route("/latest", 200, b"hello".to_vec())]);
        let text = fetch_text(&api_agent(), &server.url("/latest"), 1000);
        assert_eq!(text, Ok("hello".to_owned()));
        let head = server.requests()[0].to_lowercase();
        assert!(head.contains("user-agent: voicechanger/"), "{head}");
        assert!(
            head.contains("accept: application/vnd.github+json"),
            "{head}"
        );
    }

    #[test]
    fn fetch_text_http_error() {
        let server = TestServer::start(vec![route("/forbidden", 403, Vec::new())]);
        let err = fetch_text(&api_agent(), &server.url("/forbidden"), 1000).unwrap_err();
        assert!(err.contains("403"), "{err}");
        let err = fetch_text(&api_agent(), &server.url("/missing"), 1000).unwrap_err();
        assert!(err.contains("404"), "{err}");
    }

    #[test]
    fn fetch_text_limit() {
        let server = TestServer::start(vec![route("/big", 200, vec![b'a'; 2000])]);
        assert!(fetch_text(&api_agent(), &server.url("/big"), 1000).is_err());
    }

    #[test]
    fn fetch_text_connection_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("http://127.0.0.1:{port}/latest");
        let err = fetch_text(&api_agent(), &url, 1000).unwrap_err();
        assert!(err.starts_with(&url), "{err}");
    }

    #[test]
    fn download_good_file_hashes() {
        let dir = TempDir::new("client-good");
        let data = body(300 * 1024);
        let server = TestServer::start(vec![route("/setup.exe", 200, data.clone())]);
        let part = dir.path().join("setup.exe.part");
        let mut calls = Vec::new();
        let got = download(
            &download_agent(),
            &server.url("/setup.exe"),
            &part,
            data.len() as u64,
            &AtomicBool::new(false),
            &mut |done| calls.push(done),
        );
        assert_eq!(got, Ok(hex(&Sha256::digest(&data))));
        assert_eq!(std::fs::read(&part).unwrap(), data);
        assert!(calls.len() > 1, "{calls:?}");
        assert_eq!(calls.last(), Some(&(300 * 1024)));
    }

    #[test]
    fn download_short_read_fails_and_deletes() {
        let dir = TempDir::new("client-short");
        let data = body(100 * 1024);
        let server = TestServer::start(vec![Route {
            content_length: Some(data.len() + 1000),
            ..route("/setup.exe", 200, data.clone())
        }]);
        let part = dir.path().join("setup.exe.part");
        let got = download(
            &download_agent(),
            &server.url("/setup.exe"),
            &part,
            (data.len() + 1000) as u64,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(matches!(got, Err(DownloadError::Failed(_))), "{got:?}");
        assert!(!part.exists());
    }

    #[test]
    fn download_size_mismatch() {
        let dir = TempDir::new("client-size");
        let data = body(10 * 1024);
        let server = TestServer::start(vec![route("/setup.exe", 200, data.clone())]);
        let part = dir.path().join("setup.exe.part");
        let got = download(
            &download_agent(),
            &server.url("/setup.exe"),
            &part,
            data.len() as u64 + 1,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert_eq!(
            got,
            Err(DownloadError::Failed(format!(
                "received {} bytes, expected {}",
                data.len(),
                data.len() + 1
            )))
        );
        assert!(!part.exists());
    }

    #[test]
    fn download_cancel_deletes_part() {
        let dir = TempDir::new("client-cancel");
        let data = body(1024 * 1024);
        let server = TestServer::start(vec![route("/setup.exe", 200, data.clone())]);
        let part = dir.path().join("setup.exe.part");
        let cancel = AtomicBool::new(false);
        let got = download(
            &download_agent(),
            &server.url("/setup.exe"),
            &part,
            data.len() as u64,
            &cancel,
            &mut |_| cancel.store(true, Ordering::Relaxed),
        );
        assert_eq!(got, Err(DownloadError::Cancelled));
        assert!(!part.exists());
    }

    #[test]
    fn download_http_error() {
        let dir = TempDir::new("client-404");
        let server = TestServer::start(Vec::new());
        let part = dir.path().join("setup.exe.part");
        let got = download(
            &download_agent(),
            &server.url("/setup.exe"),
            &part,
            10,
            &AtomicBool::new(false),
            &mut |_| {},
        );
        assert!(
            matches!(&got, Err(DownloadError::Failed(e)) if e.contains("404")),
            "{got:?}"
        );
        assert!(!part.exists());
    }

    #[test]
    fn hex_lowercase() {
        assert_eq!(hex(&[0xAB, 0x01]), "ab01");
    }
}
