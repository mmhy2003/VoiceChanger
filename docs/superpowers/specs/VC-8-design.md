# VC-8 — M6: Release (Inno Setup installer, release workflow, signing, 1.0.0)

## Goal
Users download `Setup-VoiceChanger-X.Y.Z.exe` from GitHub Releases, install it without admin rights, and it just works (DirectML, base models included, no VC++ redistributable). Every merge into `main` builds, optionally signs, tags `vX.Y.Z` and publishes a release automatically. This task ends with a 1.0.0 release PR that you merge, followed by the smoke checklist.

## Decisions
- **ContentVec licence: swap to an MIT source** (your answer). The pinned `vec-768-layer-12.onnx` (MoeSS-SUBModel) is GPL-3.0, which conflicts with the MIT app.
  - A dev-only script exports RVC's own `hubert_base.pt` (lj1995/VoiceConversionWebUI, MIT, the same repo and pinned revision family as RMVPE) to `hubert.onnx`. It keeps the input/output signature the engine uses today.
  - The exported file is hosted as an asset on a dedicated GitHub Release in this repo (tag `base-models-1`, marked prerelease, never "latest"). `assets/base-models.toml` points at it with its SHA-256.
  - The tag doesn't match `v*`, so the version guards ignore it.
  - Creating that release publishes a file, so the implementer asks you before uploading it.
  - Rejected: bundling the GPL file as "mere aggregation" (weak legal footing); relicensing the app to GPL.
- **CUDA: DirectML only in 1.0.0** (your answer). The CUDA provider option and its fallback stay as they are; the README already says it needs a CUDA `onnxruntime.dll`. A CUDA package is a later task.
- **Scope: tooling, then a release PR** (your answer). The tooling is merged into `develop` through the normal PR.
  - Then the agent opens a PR from `develop` into `main` that bumps `version` to `1.0.0` and moves `[Unreleased]` to `## [1.0.0]`.
  - You merge it. The agent then checks the workflow run and the published release, and runs the smoke checklist.
- **Signing: only when the secrets exist** (your answer). The SignPath steps are in `release.yml` but run only when `SIGNPATH_API_TOKEN` is set. Until then releases are unsigned, and the release notes add one line saying so (with a SmartScreen hint).
- **App name and paths:** `VoiceChanger`, matching the existing `%APPDATA%\VoiceChanger` and `%LOCALAPPDATA%\VoiceChanger`. `AppId` is a GUID generated once and never changed.

## Architecture

### 1. App build changes
- `.cargo/config.toml`: `[target.x86_64-pc-windows-msvc] rustflags = ["-C", "target-feature=+crt-static"]`. This applies to every build, so CI tests the same binary configuration that ships. `ort` uses `load-dynamic`, so it is unaffected.
- `crates/app/build.rs` with `winresource` (MIT, passes `cargo deny`): embeds the Windows version info. FileVersion and ProductVersion are `X.Y.Z.0` from `CARGO_PKG_VERSION`, plus ProductName and the copyright.
- No runtime code changes. The installer lays files out as `{app}\voicechanger.exe` and `{app}\vendor\models\{hubert,rmvpe}.onnx` / `{app}\vendor\runtime\{onnxruntime,DirectML}.dll`. `crates/engine/src/assets.rs` already looks in `<exe dir>\vendor` first. Templates are already embedded in the binary.

### 2. MIT HuBERT model
- `tools/export-hubert/`: an `export.py` script plus a README, following the `tools/export-templates/` pattern (a venv, `python -I`, not shipped).
  - It downloads `hubert_base.pt` at a pinned revision and checks its SHA-256.
  - It exports layer-12 768-dim features with the same input/output names and shapes as today's `hubert.onnx`.
  - It checks the ONNX output against PyTorch on reference audio (max abs diff ≤ 1e-3) and prints the SHA-256.
- `assets/base-models.toml`: the `ContentVec` entry becomes `HuBERT base (RVC, MIT)`, with the new URL, the new `sha256` and `licence = "MIT (lj1995/VoiceConversionWebUI)"`.
- `cargo xtask fetch-models` also extracts each runtime's licence file. The `[[runtime]]` entries get an optional `licence_extract` field (for example `LICENSE.txt` from the DirectML nupkg), which goes to `vendor/licences/<name>.txt`.

### 3. Third-party notices
- `about.toml` and `about.hbs`: cargo-about generates the Rust crate part of `THIRD_PARTY_NOTICES.md`. Accepted licences mirror the `deny.toml` allow list.
- `assets/notices-extra.md` (committed) covers what cargo-about can't see: ONNX Runtime (MIT), DirectML (Microsoft redistributable licence, full text from `vendor/licences`), HuBERT base and RMVPE (MIT, with source URLs), and the ONNX `.proto` (Apache-2.0).
- The build script concatenates the two parts. `THIRD_PARTY_NOTICES.md` is generated at build time and gitignored.

### 4. `installer/setup.iss`
- **Version and output:** `/DAppVersion=<display version>` and `/DFileVersion=X.Y.Z.0`; `VersionInfoVersion` must be numeric, so it gets its own define. Output is `OutputBaseFilename=Setup-VoiceChanger-{#AppVersion}` into `dist\`.
- **Install mode:** `PrivilegesRequired=lowest` with `PrivilegesRequiredOverridesAllowed=dialog commandline`, so the default is a per-user install under `{autopf}\VoiceChanger` (`%LOCALAPPDATA%\Programs`), and all-users is optional.
- **Setup flags:** `ArchitecturesAllowed`/`ArchitecturesInstallIn64BitMode=x64compatible`, `MinVersion=10.0.19045`, `CloseApplications=yes`, `UninstallDisplayIcon={app}\voicechanger.exe`, `WizardStyle=modern`, `Compression=lzma2/ultra64` with `SolidCompression=yes`.
- **[Files]:** the exe, `vendor\models\*.onnx`, `vendor\runtime\*.dll`, `LICENSE`, `THIRD_PARTY_NOTICES.md` and `CHANGELOG.md`.
- **[Icons]:** a Start menu shortcut, plus a desktop shortcut as an optional task (unchecked by default).
- **Virtual cable check (Pascal `[Code]`):** reads the `FriendlyName` of each Render endpoint under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render` (readable without admin). It matches case-insensitively on `CABLE`, `VoiceMeeter` and `Virtual Audio Cable`, the same rule as the app's `is_virtual_cable`.
  - If none is found, the finish page shows a checked box: "Open the VB-CABLE download page", a `[Run] postinstall shellexec` with a `Check:` function.
  - The finish page always offers "Launch VoiceChanger".
- **Upgrades:** the fixed `AppId` makes the same-version path upgrade in place. Setup never touches `%APPDATA%`.
- **Uninstall:** at `usPostUninstall`, if `%APPDATA%\VoiceChanger` or `%LOCALAPPDATA%\VoiceChanger` exists, it asks "Also delete your voice models and settings?" with **No** as the default. A silent uninstall always keeps the data.

### 5. Shared packaging script: `installer/build.ps1`
- Parameters: `-Version <display>` and `-FileVersion X.Y.Z.0`. It expects `target\release\voicechanger.exe` and `vendor\` to exist.
- It installs `cargo-about` if missing, generates the notices, finds `ISCC.exe` (installing Inno Setup via `choco` if it is absent on the runner) and runs ISCC.
- CI and the release use the same script, so the smoke build is exactly what ships.

### 6. `ci.yml`
- Add `workflow_call` so the release can reuse it.
- **`checks` job:** after "Release build", run `installer/build.ps1`. The display version is `X.Y.Z-dev.<run_number>` everywhere except the release; the file version is `X.Y.Z.0`.
- **On push to `develop` only:** `actions/upload-artifact` uploads `Setup-VoiceChanger-X.Y.Z-dev.<run>.exe` under the name `VoiceChanger-X.Y.Z-dev.<run>`, with `retention-days: 14`.
- **Version reading:** the `awk` that reads `[workspace.package] version` moves to `.github/scripts/version.sh` (`version.sh read`, `version.sh changelog X.Y.Z`). It is shared by `release-guard`, the dev label and `release.yml`. `release-guard` keeps its current checks: source branch, version above the latest tag, changelog section.

### 7. `.github/workflows/release.yml`
- **Trigger and settings:** `on: push: [main]` and `workflow_dispatch`, `concurrency: { group: release, cancel-in-progress: false }`, top-level `permissions: contents: read`.
- **`prepare` job:**
  - Reads `X.Y.Z` and fails if it isn't plain `X.Y.Z`.
  - Fails if tag `vX.Y.Z` exists (`git ls-remote --tags`).
  - Extracts the `## [X.Y.Z]` section of `CHANGELOG.md` (it fails if the section is missing) into the release notes.
- **`checks` job:** `uses: ./.github/workflows/ci.yml`. This runs fmt, clippy, tests and `cargo deny`, and on this path the fuzz job too.
- **`build` job** (needs both):
  1. fetch-models (cached, SHA-256 checked), then `cargo build --release`.
  2. **Sign exe** if the secret is set: upload the exe as an artifact, then `signpath/github-action-submit-signing-request` waits for completion and downloads the signed copy back to `target\release`.
  3. `build.ps1 -Version X.Y.Z -FileVersion X.Y.Z.0`.
  4. **Sign installer** the same way.
  5. Write `SHA256SUMS.txt` (`sha256  Setup-VoiceChanger-X.Y.Z.exe`) and upload `dist\` as an artifact.
  - Signing config: `SIGNPATH_API_TOKEN` is a secret; `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` and `SIGNPATH_POLICY_SLUG` are repo variables. They are only available on `main`, and fork PRs never get them.
- **`publish` job** (`permissions: contents: write`): `gh release create vX.Y.Z --target $GITHUB_SHA --title "VoiceChanger X.Y.Z" --notes-file notes.md` with the installer and `SHA256SUMS.txt`. This also creates the tag. When unsigned, the notes get the extra line.
- **Re-running:** a failed run publishes nothing and creates no tag, so `workflow_dispatch` can re-run it safely. A run whose tag already exists stops at the guard.

### 8. Docs
- **README:** an Install section (download, per-user install, VB-CABLE, unsigned builds and SmartScreen) and a Releasing note. Replace the "ContentVec is GPL-3.0" line.
- **AGENTS.md:** the CI/CD lines (installer smoke, dev artifacts, signing variables) and `cargo xtask fetch-models` before building the installer.
- **CHANGELOG `[Unreleased]`:** an Added line for the installer and releases, and a Changed line for the MIT HuBERT base model.

## Error handling and edge cases
- A missing `vendor` file or ISCC failure fails the build with ISCC's message. `build.ps1` checks the inputs first and names any missing file.
- A dev display version like `0.1.0-dev.42` is never used as `VersionInfoVersion`.
- If the app is running during an upgrade or uninstall, Setup closes it through Restart Manager (`CloseApplications`).
- Admin and all-users installs read the installing user's `%APPDATA%`. Other users' data is never deleted.
- If the SignPath request fails or times out while the secret is set, the release fails. It never falls back to unsigned.
- Install size stays under 150 MB without models; DLLs are about 20 MB, the exe is small.

## Testing
- **xtask unit tests (written first):** parsing `licence_extract` and extracting it from a test zip, with the rest of the manifest parsing unchanged.
- **Script tests:** `version.sh` read and changelog extraction, run in CI against `Cargo.toml`/`CHANGELOG.md` and small fixture files.
- **HuBERT swap:**
  - The export script's ONNX-vs-PyTorch check.
  - The existing engine asset tests (`VC_REQUIRE_ASSETS=1`) with the new file.
  - A `vc-convert` run on reference WAVs, compared by ear with the old model.
- **Installer:** every CI run builds the installer (smoke).
- **Manual check on Windows Sandbox before the release PR:**
  - Install the develop artifact per-user with no admin prompt.
  - Run with VB-CABLE absent and present, and check the finish-page checkbox.
  - Check the file version in Explorer.
  - Uninstall with "No" and with "Yes".
- **After you merge the release PR:**
  - Check the workflow run, the `v1.0.0` tag, the release assets and that `SHA256SUMS.txt` matches.
  - Smoke checklist: a clean install of 1.0.0; an upgrade from the develop artifact to 1.0.0 that keeps the models and settings; uninstall.
  - Report the results on the task.

## Out of scope
- CUDA runtime packaging.
- Setting up a SignPath account.
- An app icon or branding images (default Inno and egui visuals).
- An auto-updater.
- winget/Chocolatey manifests.
- Correcting the PRD latency figure.
