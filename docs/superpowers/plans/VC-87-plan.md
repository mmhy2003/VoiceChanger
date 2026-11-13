# VC-87 — Auto-check for updates and auto-update: implementation plan

## Goal
VoiceChanger checks GitHub Releases every 6 hours (on by default), shows a newer version with its release notes and **Update now / Later / Skip this version**, then downloads, verifies (SHA-256, plus Authenticode once releases are signed) and silently installs it. The installer reopens the app.

## Architecture
A new `crates/app/src/update/` module holds small pure units (`version`, `release`, `schedule`, `verify`, `state`) and two I/O units (`client` for `ureq` calls, `worker` for the one background thread). The UI thread owns an `UpdateState` state machine. It feeds the state machine with `worker::Message`s, and the state machine returns `Action`s (send a command to the worker, or install). `install.rs` spawns `Setup-…exe /SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH /LOG=…` and closes the window. `installer/setup.iss` gets a `[Run]` entry, gated on `/RELAUNCH`, that restarts the app in silent mode.

## Facts about the repo (checked at `093ba90`)
- Workspace version `1.0.0` in `[workspace.package]` of `Cargo.toml`. App crate `crates/app`, package `voicechanger`; modules are declared in `crates/app/src/main.rs`.
- `crates/app/Cargo.toml` has no HTTP, JSON, hashing or Windows API dependencies. `Cargo.lock` already has `ureq 3.5.2` (xtask: `default-features = false, features = ["native-tls-no-default"]`), `sha2 0.10.9` and `windows-sys 0.61.2`. `serde_json` is not in the lock yet (MIT/Apache-2.0, which `deny.toml` allows).
- `xtask/src/fetch.rs::fetch_all` shows the TLS setup: `TlsConfig::builder().provider(TlsProvider::NativeTls).root_certs(RootCerts::PlatformVerifier)`.
- `crates/app/src/config.rs`: `Config` (`#[serde(default)]`, `CONFIG_VERSION = 1`), `Prefs` (`Prefs::from_config`), `SaveTracker`. `AppState::to_config(&Prefs)` is in `app.rs`. Tests use `crate::testutil::TempDir`.
- `crates/app/src/paths.rs`: `AppDirs { config, models, logs }`, `AppDirs::under`, test `dirs_layout`.
- `app.rs`: `VcApp` has `prefs`, `dirs`, `stop()`, `is_running()`, `write_config(cfg)`. `update()` draws `TopBottomPanel::top("tabs")`, then a `CentralPanel`, then the debounced save. `on_exit` saves and shuts audio down.
- `tabs/settings.rs::general()` draws Settings → General.
- Threads use `std::thread::Builder::new().name("vc-…")`. The app crate has no `unsafe` yet. CI runs clippy with `-D warnings`. Convention: `#[expect(dead_code, reason = …)]`, not `#[allow]`.
- `installer/setup.iss`: `PrivilegesRequired=lowest`, `CloseApplications=yes`. The launch `[Run]` entry is `postinstall nowait skipifsilent`. `PurgeDataRequested` loops over `ParamStr`.
- `installer/test-install.ps1`: `Install($Path,$Name,$Extra)` always passes `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /LOG=…`. Step 1 is `$log = Install $Setup 'upgrade'`. CI and release.yml run it.
- release.yml writes `SHA256SUMS.txt` as `"<lowercase hex>  Setup-VoiceChanger-X.Y.Z.exe\n"` and publishes it with the installer on tag `vX.Y.Z`.

## Quality gate (every step, before committing)
Run `cargo xtask fetch-models` once first.
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
$env:VC_REQUIRE_ASSETS='1'; cargo test --workspace
cargo deny check
cargo build --release
```
Commit with the `Co-Authored-By` trailer. Never rewrite history.

## Dead code before wiring
In step 1, declare the module as
```rust
#[cfg_attr(not(test), expect(dead_code, reason = "wired into VcApp in step 11"))]
mod update;
```
Every item must be used by its tests, so the test build has no dead code; if clippy reports some, add a test that uses the item. Step 11 removes the attribute.

## File map
| File | Change | Responsibility |
|---|---|---|
| `crates/app/Cargo.toml` | modify | `serde_json`, `sha2`, `ureq`, `windows-sys` |
| `crates/app/src/main.rs` | modify | `mod update;` |
| `update/mod.rs` | create | module docs, `mod` lines |
| `update/version.rs` | create | strict `X.Y.Z`/`vX.Y.Z` `Version` |
| `update/release.rs` | create | release JSON → `Release` → `Update`; `SHA256SUMS.txt` parsing |
| `update/schedule.rs` | create | `next_check`, `check_due`, `should_notify`, `unix_now` |
| `update/client.rs` | create | ureq agents, `fetch_text`, streamed hashing `download` |
| `update/testserver.rs` | create (test) | one-thread HTTP/1.1 server on 127.0.0.1 |
| `update/verify.rs` | create | `sha256_file`, `Signature`, WinVerifyTrust, `signature_ok` |
| `update/worker.rs` | create | `check`, `download_update`, `cleanup`, `Updater` thread |
| `update/state.rs` | create | `UpdateState`/`Phase`/`Action` |
| `update/install.rs` | create | `installer_args`, `spawn_installer` |
| `update/ui.rs` | create | banner, window, Settings rows, `progress_text` |
| `config.rs` | modify | `UpdatesConfig` in `Config` and `Prefs` |
| `app.rs` | modify | `to_config` copies `updates`; `VcApp` owns `Updater` + `UpdateState` |
| `paths.rs` | modify | `AppDirs.updates` |
| `tabs/settings.rs` | modify | call `settings_rows` |
| `installer/setup.iss` | modify | `/RELAUNCH` `[Run]` entry, `ParamGiven` |
| `installer/test-install.ps1` | modify | `/SILENT /RELAUNCH` upgrade check |
| `CHANGELOG.md`, `README.md` | modify | notes, "Updates and privacy" |

(`update/` = `crates/app/src/update/`.)

## Contracts (names and signatures are fixed)

### C1 `update/version.rs`
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version { pub major: u32, pub minor: u32, pub patch: u32 }
impl Version {
    /// Strict `X.Y.Z` or `vX.Y.Z`: three ASCII-digit parts, no sign, no leading zeros except "0",
    /// no suffix or spaces, no u32 overflow.
    pub fn parse(s: &str) -> Option<Self>;
    /// A release tag: must be `vX.Y.Z`.
    pub fn parse_tag(tag: &str) -> Option<Self>;
    /// From env!("CARGO_PKG_VERSION").
    pub fn current() -> Self;
}
impl std::fmt::Display for Version { /* "X.Y.Z" */ }
```

### C2 `update/release.rs`
```rust
pub const CHECKSUMS: &str = "SHA256SUMS.txt";
pub fn installer_name(v: Version) -> String;      // "Setup-VoiceChanger-1.1.0.exe"
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
pub struct Asset { pub name: String, pub browser_download_url: String, pub size: u64 }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release { pub version: Version, pub title: String, pub notes: String, pub date: String, pub assets: Vec<Asset> }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update { pub version: Version, pub title: String, pub notes: String, pub date: String,
                    pub installer: Asset, pub checksums: Asset }
/// Err for invalid JSON / no tag_name. Ok(None) for draft, prerelease, or a tag that is not strict vX.Y.Z.
/// title = name, or "VoiceChanger X.Y.Z" if null/empty; notes = body with \r\n → \n ("" if null);
/// date = first 10 chars of published_at ("" if null).
pub fn parse_release(json: &str) -> Result<Option<Release>, String>;
impl Release {
    /// Exact names. Err("the release has no Setup-VoiceChanger-1.1.0.exe") / Err("the release has no SHA256SUMS.txt").
    pub fn into_update(self) -> Result<Update, String>;
}
/// Lowercase hex for `file`: lines `<64 hex><ws><name>` or `<hex> *<name>`; hex any case; others ignored.
pub fn parse_sums(text: &str, file: &str) -> Option<String>;
```
The private `ReleaseJson` has `tag_name: String`, `name/body/published_at: Option<String>`, and `#[serde(default)]` `draft: bool`, `prerelease: bool` and `assets: Vec<Asset>`.

### C3 `update/schedule.rs`
```rust
pub const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(5);
pub fn unix_now() -> u64;
/// None if auto is off. Else max(FIRST_CHECK_DELAY - since_start, interval wait); interval wait = 0 when
/// last_check == 0 or now < last_check (clock went back), else (last_check + 6 h) - now (saturating).
pub fn next_check(auto_check: bool, last_check: u64, now: u64, since_start: Duration) -> Option<Duration>;
pub fn check_due(auto_check: bool, last_check: u64, now: u64, since_start: Duration) -> bool; // == Some(ZERO)
/// latest > current && (manual || skipped.and_then(Version::parse) != Some(latest)).
pub fn should_notify(latest: Version, current: Version, skipped: Option<&str>, manual: bool) -> bool;
```

### C4 `update/client.rs`
```rust
pub const LATEST_URL: &str = "https://api.github.com/repos/mmhy2003/VoiceChanger/releases/latest";
pub fn api_agent() -> ureq::Agent;      // xtask TLS, user agent "VoiceChanger/<ver>", connect 15 s, global 30 s
pub fn download_agent() -> ureq::Agent; // same TLS/UA, connect 15 s, recv_response 30 s, no global timeout
/// GET with `Accept: application/vnd.github+json`, body limited to `max`. Err("<url>: <error>").
pub fn fetch_text(agent: &ureq::Agent, url: &str, max: u64) -> Result<String, String>;
#[derive(Debug, PartialEq, Eq)]
pub enum DownloadError { Cancelled, Failed(String) }
/// Streams into `part` in 64 KiB chunks, hashing (sha2::Sha256) while writing. It checks `cancel` before each read,
/// and calls progress(total_so_far) after each chunk. A byte count != size → Failed("received N bytes, expected M").
/// Returns lowercase hex. Deletes `part` on every Err, including Cancelled.
pub fn download(agent: &ureq::Agent, url: &str, part: &Path, size: u64, cancel: &AtomicBool,
                progress: &mut dyn FnMut(u64)) -> Result<String, DownloadError>;
pub fn hex(bytes: &[u8]) -> String;
```
Check the ureq 3.5 builder names (`config_builder().tls_config().user_agent().timeout_connect(Some(..)).timeout_global(..).timeout_recv_response(..)`) and the body API (`with_config().limit(max).read_to_string()`, `into_reader()`) on docs.rs or Context7. The installer is hundreds of MB, so the download reader must have no size limit.

### C5 `update/testserver.rs` (`#[cfg(test)] mod testserver;`)
```rust
pub struct Route { pub path: &'static str, pub status: u16, pub body: Vec<u8>,
                   /// None = body.len(); larger = short read.
                   pub content_length: Option<usize> }
pub struct TestServer { base: String, requests: Arc<Mutex<Vec<String>>> }
impl TestServer {
    /// Binds 127.0.0.1:0, one detached thread, one connection at a time: it reads the head up to \r\n\r\n,
    /// records it, matches "GET <path> HTTP/1.1" (unknown → 404, empty body), writes
    /// "HTTP/1.1 <status> X\r\nContent-Length: <n>\r\nConnection: close\r\n\r\n" + body, then closes.
    pub fn start(routes: Vec<Route>) -> Self;
    pub fn url(&self, path: &str) -> String;
    pub fn requests(&self) -> Vec<String>;
}
```

### C6 `update/verify.rs`
```rust
pub fn sha256_file(path: &Path) -> io::Result<String>;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signature { Valid, Unsigned, Invalid(i32) }
/// 0 → Valid; TRUST_E_NOSIGNATURE / TRUST_E_SUBJECT_FORM_UNKNOWN / TRUST_E_PROVIDER_UNKNOWN → Unsigned; else Invalid.
/// The PE subject-interface package recognises the file by its content, so a `.part` name is fine.
pub fn signature(path: &Path) -> Signature;
/// running Valid ⇒ installer must be Valid: Err("is not validly signed (unsigned)") or
/// Err("is not validly signed (error 0x800B0109)"). Otherwise Ok (only the SHA-256 is checked).
pub fn signature_ok(running: Signature, installer: Signature) -> Result<(), String>;
```
WinVerifyTrust (`windows-sys` 0.61, features `Win32_Foundation`, `Win32_Security_WinTrust`, `Win32_Security_Cryptography`; `TRUST_E_*` are HRESULT consts in `Win32::Foundation`):
```rust
let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
let mut file = WINTRUST_FILE_INFO { cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
    pcwszFilePath: wide.as_ptr(), hFile: std::ptr::null_mut(), pgKnownSubject: std::ptr::null_mut() };
// SAFETY: WINTRUST_DATA is a plain C struct; all-zero is a valid starting value.
let mut data: WINTRUST_DATA = unsafe { std::mem::zeroed() };
data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
data.dwUIChoice = WTD_UI_NONE;
data.fdwRevocationChecks = WTD_REVOKE_NONE;
data.dwUnionChoice = WTD_CHOICE_FILE;
data.Anonymous = WINTRUST_DATA_0 { pFile: &mut file };
data.dwStateAction = WTD_STATEACTION_VERIFY;
let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
// SAFETY: data, file and wide outlive both calls; CLOSE frees the state VERIFY allocated.
let status = unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
data.dwStateAction = WTD_STATEACTION_CLOSE;
unsafe { WinVerifyTrust(std::ptr::null_mut(), &mut action, (&raw mut data).cast()) };
```

### C7 `update/worker.rs`
```rust
pub struct Agents { pub api: ureq::Agent, pub download: ureq::Agent }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome { Available(Update), UpToDate, NotInstallable(String), Failed(String) }
/// fetch_text(2 MiB) → parse_release; None or version <= current → UpToDate; newer → into_update:
/// Ok → Available, Err(r) → NotInstallable("VoiceChanger X.Y.Z: <r>"). Errors → Failed. warn! on Failed/NotInstallable.
pub fn check(agent: &ureq::Agent, url: &str, current: Version) -> CheckOutcome;
/// 1 create dir; 2 fetch checksums (64 KiB) with agents.download; 3 parse_sums, None →
/// Failed("SHA256SUMS.txt has no line for <name>"); 4 if dir/<name> exists and its hash matches → Ok(it), else delete it;
/// 5 client::download to dir/<name>.part; 6 mismatch → delete, Failed("<name> did not match SHA256SUMS.txt and was deleted.");
/// 7 signature_ok(running, signature(part)) Err(e) → delete, Failed("<name> <e> and was deleted."); 8 rename → Ok(dir/<name>).
/// progress(done, update.installer.size).
pub fn download_update(agents: &Agents, update: &Update, dir: &Path, running: Signature,
                       cancel: &AtomicBool, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf, DownloadError>;
/// Deletes *.part and Setup-VoiceChanger-X.Y.Z.exe with version <= current; keeps other files; missing dir is fine.
pub fn cleanup(dir: &Path, current: Version);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command { Check { manual: bool }, Download(Update) }
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message { Checked { manual: bool, outcome: CheckOutcome }, Progress { done: u64, total: u64 },
                   Ready(PathBuf), DownloadFailed(String), Cancelled }
pub struct WorkerConfig { pub latest_url: String, pub dir: PathBuf, pub current: Version, pub running_exe: Option<PathBuf> }
pub type Wake = Box<dyn Fn() + Send>;
pub struct Updater { tx: Sender<Command>, rx: Receiver<Message>, cancel: Arc<AtomicBool> }
impl Updater {
    pub fn spawn(config: WorkerConfig, wake: Wake) -> Self;   // thread "vc-update"
    pub fn send(&self, command: Command);   // Download clears `cancel` first
    pub fn cancel(&self);
    pub fn poll(&self) -> Vec<Message>;
}
```

### C8 `update/state.rs`
```rust
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase { #[default] Idle, Checking { manual: bool }, Available(Update),
                 Downloading { update: Update, done: u64, total: u64, install: bool },
                 Ready { update: Update, path: PathBuf }, Installing }
#[derive(Debug, PartialEq, Eq)]
pub enum Action { Send(Command), Install(PathBuf) }
#[derive(Debug, Default)]
pub struct UpdateState { pub phase: Phase, pub window_open: bool, pub banner_hidden: bool,
                         pub status: Option<String>,   // last check result, Settings → General
                         pub error: Option<String> }   // shown in the window
impl UpdateState {
    pub fn busy(&self) -> bool;               // Checking | Downloading | Installing
    pub fn can_check(&self) -> bool;          // Idle | Available
    pub fn checking(&mut self, manual: bool); // if can_check → Checking{manual}; manual clears status
    pub fn apply(&mut self, msg: Message, prefs: &UpdatesConfig, current: Version) -> Option<Action>;
    pub fn update_now(&mut self) -> Option<Action>;
    pub fn later(&mut self);
    pub fn skip(&mut self, prefs: &mut UpdatesConfig);
    pub fn cancel_download(&mut self) -> bool;
    pub fn update(&self) -> Option<&Update>;
    pub fn banner_text(&self) -> Option<String>;
}
```
`apply` rules:
- `Checked` in Downloading, Ready or Installing is ignored (None).
- `Available(u)` that `should_notify` accepts: phase Available, banner_hidden false, window_open = manual, error None, status "VoiceChanger X.Y.Z is available.". If `prefs.auto_download`, the phase becomes Downloading{install:false, done 0, total installer.size} and the return is `Send(Download(u))`.
- `Available(u)` that is skipped: Idle, status "VoiceChanger X.Y.Z is available (skipped).".
- `UpToDate`: Idle, "You have the latest version (<current>).". `NotInstallable(r)`: Idle, "No installable update: <r>.". `Failed(r)`: Idle; when manual, "Could not check for updates: <r>"; when automatic, the status is unchanged.
- `Progress`: updates done/total. `Ready(p)`: Downloading → Ready, and returns `Install(p)` when install is set. `DownloadFailed(e)`: → Available, error Some(e), window_open |= install. `Cancelled`: → Available.

`update_now`: Available → Downloading{install:true} + `Send(Download)`. Downloading → install = true, None. Ready → `Install(path)`. It always clears error and opens the window.
`skip`: skipped_version = Some(version), Idle, window closed, status "VoiceChanger X.Y.Z will be skipped.". `later`: window closed. `cancel_download`: Downloading → Available, true; otherwise false.
`banner_text`: None if hidden. Available or Downloading → "VoiceChanger X.Y.Z is available.". Ready → "VoiceChanger X.Y.Z is ready to install.".

### C9 `update/install.rs`
```rust
pub fn installer_args(log: &Path) -> Vec<OsString>; // /SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH /LOG=<log>
pub fn spawn_installer(exe: &Path, log: &Path) -> io::Result<()>; // creates log's parent, spawns, never waits
```

### C10 `config.rs`
```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig { pub auto_check: bool, pub auto_download: bool,
                           pub last_check: u64 /* unix s, 0 = never */, pub skipped_version: Option<String> }
// Default: true, false, 0, None. Config.updates ([updates]) and Prefs.updates; CONFIG_VERSION stays 1.
```

## Steps
1. Add the update module and strict version parsing
2. Save update settings in settings.toml
3. Parse GitHub releases and SHA256SUMS.txt
4. Decide when to check and when to notify
5. HTTP client and streamed hashing download
6. SHA-256 file hash and Authenticode check
7. Check, verified download and cleanup
8. Background updater thread
9. Update state machine
10. Silent install with relaunch
11. Update UI and wiring into the app
12. Docs: changelog, README, PRD amendment

The full detail of each step is in its subtask. The contracts above are the reference.

## Final verification
1. The quality gate is green locally.
2. `cargo test -p voicechanger update::` and `cargo test -p voicechanger config::` pass.
3. On the PR into `develop`, CI's installer test is green: the `/SILENT /RELAUNCH` upgrade starts `{app}\voicechanger.exe`, and a silent install without `/RELAUNCH` does not. No manual checklist.
