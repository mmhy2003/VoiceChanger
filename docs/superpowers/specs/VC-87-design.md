# VC-87 — Auto-check for updates and auto-update

## Goal
VoiceChanger checks GitHub Releases for a newer version every 6 hours (on by default). When one exists, it shows the version and release notes, with **Update now**, **Later** and **Skip this version**. Update now downloads and verifies the installer, installs it silently with a progress window, and reopens the app. Models and settings are kept.

## Decisions
- **On by default** (human's choice), with a setting to turn it off. We drop the PRD's "no network calls" promise for this one case. PRD §2/§6 and the README will say the app calls `api.github.com` and downloads from GitHub, and only for updates. A first-run prompt and off-by-default were both rejected.
- **Every 6 hours** (instruction from Mohamed Youssef; replaces "at most once a day"). At startup the app checks if the last check was 6 h or more ago. While it runs, it checks again every 6 h. The time of the last check is saved, so restarting the app does not reset the timer.
- **Silent install with a progress window** (human's choice): `/SILENT /SUPPRESSMSGBOXES /NORESTART`. Fully invisible `/VERYSILENT` and the normal wizard were both rejected.
- **The installer relaunches the app** (human's choice). A separate `updater.exe` was rejected (one more binary to ship and sign). The `self_update` crate was rejected (it swaps the exe in place, so DLLs, models, shortcuts and the uninstall entry would drift).
- **HTTP: `ureq` 3 with `native-tls`** (Windows SChannel). xtask already uses it, so it is in `Cargo.lock` and passes `cargo deny`. There are no new licences, and it uses the Windows certificate store. Rejected: `reqwest` (pulls in tokio, which this app does not need).
- **Per-user install needs no extra work.** `setup.iss` already defaults to per-user (`PrivilegesRequired=lowest`), so an update to a per-user install needs no admin rights. An install that was done for all users is upgraded the same way: Inno reuses the earlier mode (`UsePreviousPrivileges`, the default) and asks for UAC itself.
- **Automatic download is off by default.** When it is on, the installer downloads in the background, but nothing installs until the user clicks Update now.

## Architecture
New module `crates/app/src/update/`, split into small units:
- `version.rs`: parses strict `X.Y.Z` and `vX.Y.Z` and compares versions. Pure.
- `release.rs`: the subset of the GitHub release JSON we read (tag_name, name, body, published_at, assets[name, browser_download_url, size]). Picks the assets `Setup-VoiceChanger-X.Y.Z.exe` and `SHA256SUMS.txt`. Parses `SHA256SUMS.txt`. Pure.
- `schedule.rs`: decides whether a check is due (`last_check` and the 6 h interval), and whether to notify (newer than the running version, and not the skipped version, unless the check was manual). Pure; takes `now` as a parameter.
- `client.rs`: `ureq` calls. `GET https://api.github.com/repos/mmhy2003/VoiceChanger/releases/latest` (this endpoint already leaves out drafts and pre-releases, including `base-models-1`). Sends `User-Agent: VoiceChanger/<ver>` and `Accept: application/vnd.github+json`, with timeouts. A streamed download hashes with `sha2` as it writes.
- `verify.rs`: compares the SHA-256 and checks the Authenticode signature with `WinVerifyTrust` (`windows-sys`). The rule: if the running `voicechanger.exe` is signed, the installer must be validly signed too. While releases are unsigned, only the SHA-256 is checked.
- `worker.rs`: one background thread (`thread::Builder`, the same pattern as `import.rs` and `files.rs`). The UI sends it commands (Check, Download, Cancel) over mpsc, and it sends back messages (Checking, Available, UpToDate, Failed, Progress, Ready). It calls `ctx.request_repaint()` after each message. Cancel sets an `AtomicBool`. It never touches the audio or inference threads.
- `install.rs`: stops audio (`VcApp::stop`) and saves settings right away. Then it spawns `Setup-…exe /SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH /LOG=<logs>\update-install.log` and closes the window (`ViewportCommand::Close`).

The UI lives in `update/ui.rs`:
- a one-line banner above the tabs on every tab ("VoiceChanger 1.1.0 is available. View… ✕", or "…is ready to install" once it is downloaded);
- an "Update available" window with the version, the current version, the date, the release notes as plain text in a scroll area, the three buttons, then a progress bar with Cancel and the errors;
- new rows in Settings → General (`tabs/settings.rs`): "Check for updates automatically (every 6 hours)", "Download updates in the background" (only usable when the first is on), and a **Check for updates** button with the last result.

`installer/setup.iss`: a new `[Run]` entry `{app}\voicechanger.exe` with `Flags: nowait runasoriginaluser` and `Check: RelaunchRequested`. It has no `skipifsilent` and no `postinstall`, so it also runs in silent mode. `RelaunchRequested` reads `/RELAUNCH` the same way `PurgeDataRequested` does. `CloseApplications=yes` stays as a safety net.

## Data and interfaces
New `[updates]` table in `settings.toml` (with `#[serde(default)]`, so `CONFIG_VERSION` stays 1 and old files load):
```toml
[updates]
auto_check = true
auto_download = false
last_check = 1763100000      # Unix seconds; 0 = never
skipped_version = "1.1.0"    # optional
```
These are mirrored in `Prefs` and saved by the existing debounced save. Downloads go to `%LOCALAPPDATA%\VoiceChanger\updates\`, first as `*.part`, renamed only after they are verified. At startup, files there for versions ≤ the running version are deleted.

## Error handling and edge cases
- Automatic check fails (offline, DNS, TLS, timeout, HTTP 403 rate limit, bad JSON): `tracing::warn!`, nothing shown, and `last_check` is still updated, so a 6 h back-off. A manual check shows "Could not check for updates: <reason>".
- Startup never waits. The first check runs on the worker thread a few seconds after the window opens.
- A newer tag without the expected installer or checksum asset is logged and treated as "no installable update".
- Tags that are not strict `vX.Y.Z` are ignored. An equal or older version gives "You have the latest version".
- Hash mismatch, a missing checksum line, a bad or missing signature (when required), or a size different from the asset's `size`: the file is deleted and the error shown ("…did not match SHA256SUMS.txt and was deleted"). Cancel deletes the `.part` file.
- Update now while the voice is running stops it first. The window text warns that the voice will stop.
- If the installer fails to spawn (for example, antivirus blocks it), an error is shown and the app stays open.
- Only one download runs at a time. If the user clicks Update now again, they see the progress that is already running.
- The rate limit (60 requests/h without a token) is far above one check per 6 h.

## Testing
All automated, in CI:
- Unit tests for version parsing and ordering, release JSON parsing from a fixture, asset choice (missing, wrong names), `SHA256SUMS.txt` parsing, the schedule (due or not around 6 h, a clock that went backwards, skipped version against a manual check), and config round-trip plus defaults when `[updates]` is missing.
- Download tests against a real local HTTP server (`std::net::TcpListener` on 127.0.0.1): a good file is verified and renamed, a hash mismatch deletes it, cancel deletes the `.part` file, a short read fails.
- `verify.rs`: an unsigned temp file is reported as unsigned. The signed-exe rule is tested as a pure function.
- `installer/test-install.ps1`: the upgrade step also runs with `/SILENT /RELAUNCH`, checks that `voicechanger.exe` started from `{app}`, then stops it. A silent install without `/RELAUNCH` still does not start the app.
- The quality gate as usual: fmt, clippy `-D warnings`, workspace tests with `VC_REQUIRE_ASSETS=1`, `cargo deny check`, release build.

## Docs
CHANGELOG `[Unreleased]/Added`. A README "Updates and privacy" note. The KB PRD gets an amendment to §2/§6 (network use for updates only) and to §10/§11 (in-app updates).

## Out of scope
- Delta or patch updates, beta or pre-release channels, and downgrades.
- Updating without a user click (even with automatic download on).
- Release notes shown as rendered Markdown. They are shown as plain text.
- A GitHub token or proxy settings.
- Changes to `release.yml`: the existing asset names and `SHA256SUMS.txt` are enough.
