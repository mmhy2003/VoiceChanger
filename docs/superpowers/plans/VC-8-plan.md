# VC-8 — M6 Release: implementation plan

## Goal
Ship `Setup-VoiceChanger-X.Y.Z.exe`: a per-user Inno Setup installer with a static-CRT exe, DirectML runtime, MIT base models and notices. Every merge into `main` builds, optionally signs, tags and publishes it. The task ends with a 1.0.0 release PR, then a smoke check of the published release.

## Architecture
The app build gains a static CRT (`.cargo/config.toml`) and a Windows version resource (`crates/app/build.rs`, `winresource`). One PowerShell script, `installer/build.ps1`, checks the inputs, generates `THIRD_PARTY_NOTICES.md` (`installer/notices.ps1` + cargo-about) and runs ISCC on `installer/setup.iss`. CI's smoke build and `release.yml` both call that script, so the smoke build is exactly what ships. `.github/scripts/version.sh` is the one place that reads the version and changelog sections. The GPL ContentVec is replaced by RVC's MIT `hubert_base.pt`, which a dev-only script exports to ONNX and which is hosted on a `base-models-1` prerelease.

## Facts about the repo (checked at `e085d04`)
- Workspace version is `0.1.0` in `[workspace.package]` of `Cargo.toml`. The app crate is `crates/app` (package `voicechanger`, bin `voicechanger`). There is no `build.rs` yet.
- `.cargo/config.toml` holds only `[alias] xtask = "run -p xtask --"`.
- `xtask/src/manifest.rs` defines `Entry {name,url,sha256,licence,dest,extract}`. `parse()` validates entries and requires `extract` for runtimes. Tests live in the same file, including `repo_manifest_is_valid`, which expects 2 models and 2 runtimes.
- `xtask/src/fetch.rs` has `extract_from_zip(zip, inner, dest)` and `fetch_all(root, manifest)`; runtime packages are cached in `vendor/cache/<nupkg>`. Its tests build zips with `zip::ZipWriter` through `write_zip`. `ensure()` deletes a file whose hash doesn't match and returns an error, so after a manifest hash change a local `fetch-models` must be run twice.
- `crates/engine/src/assets.rs` looks in `<exe dir>\vendor` first (`models/hubert.onnx`, `models/rmvpe.onnx`, `runtime/onnxruntime.dll`, `runtime/DirectML.dll`).
- `crates/engine/src/hubert.rs` takes input 0 of rank 2 or 3 and requires the last output dimension to be 768. The test `hubert_one_second_is_50_frames` expects 50 frames for 16000 samples plus 2×40 padding.
- `crates/audio/src/devices.rs` defines `CABLE_MARKERS = ["cable", "voicemeeter", "virtual audio cable"]` (lowercase, substring match).
- `.github/workflows/ci.yml` has jobs `checks` (windows: fetch-models, fmt, clippy, test with `VC_REQUIRE_ASSETS=1`, cargo-deny, release build), `fuzz` (ubuntu) and `release-guard` (PRs into main; inline awk version reader). Its concurrency is `ci-${{ github.ref }}`, cancel-in-progress.
- `.gitignore` holds `/target` and `/vendor`. `LICENSE` reads "Copyright (c) 2026 VoiceChanger contributors". `crates/convert/proto/LICENSE` is the ONNX Apache-2.0 licence.
- `tools/export-templates/` is the pattern for dev-only Python: `README.md`, `export.py`, `requirements.txt`, a venv under `%TEMP%` and `python -I`.
- There are no `v*` tags yet. Commits use conventional prefixes (`feat(app):`, `docs:`, `test(convert):`) and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Quality gate (run before every commit; abbreviated as **QG** below)
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
$env:VC_REQUIRE_ASSETS='1'; cargo test --workspace
cargo deny check
cargo build --release
```
`cargo xtask fetch-models` must have been run first. PowerShell scripts are run with `pwsh -File …` (Windows PowerShell 5.1 `powershell -File …` works too).

## File map
| File | Status | Responsibility |
|---|---|---|
| `.cargo/config.toml` | modify | static CRT rustflags for `x86_64-pc-windows-msvc` |
| `crates/app/Cargo.toml`, `crates/app/build.rs` | modify / create | Windows version resource (FileVersion `X.Y.Z.0`) |
| `xtask/src/manifest.rs`, `xtask/src/fetch.rs` | modify | `licence_extract` field, extracting licence files to `vendor/licences/` |
| `assets/base-models.toml` | modify | `licence_extract` for runtimes; HuBERT base (MIT) entry |
| `.github/scripts/version.sh`, `.github/scripts/test-version.sh`, `.github/scripts/fixtures/{Cargo.fixture.toml,CHANGELOG.fixture.md}` | create | read version / changelog section, plus tests |
| `.gitattributes` | modify | LF for `*.sh` and fixtures |
| `about.toml`, `about.hbs`, `assets/notices-extra.md`, `installer/notices.ps1` | create | `THIRD_PARTY_NOTICES.md` generation |
| `.gitignore` | modify | `/dist`, `/THIRD_PARTY_NOTICES.md` |
| `installer/setup.iss`, `installer/build.ps1` | create | installer definition and shared packaging script |
| `.github/workflows/ci.yml` | modify | `workflow_call`, script tests, installer smoke, develop artifact, guard uses `version.sh` |
| `.github/workflows/release.yml` | create | guard, checks, build, sign, checksums, publish |
| `tools/export-hubert/{README.md,export.py,requirements.txt}` | create | dev-only MIT HuBERT export |
| `README.md`, `AGENTS.md`, `CHANGELOG.md` | modify | docs |

## Steps

### 1. Static CRT and Windows version info for voicechanger.exe
- **Failing check first:** `cargo build --release`, then `(Get-Item target\release\voicechanger.exe).VersionInfo.FileVersion` prints nothing (expected `0.1.0.0`), and `Select-String -Path target\release\voicechanger.exe -Pattern VCRUNTIME140 -SimpleMatch -Quiet` prints `True`.
- **`.cargo/config.toml`:** keep `[alias]` and add:
  ```toml
  # Static C runtime: the installer ships no VC++ redistributable. Applies to every build so CI tests what ships.
  [target.x86_64-pc-windows-msvc]
  rustflags = ["-C", "target-feature=+crt-static"]
  ```
- **`crates/app/Cargo.toml`:** add `build = "build.rs"` (optional, it is the default) and a `[build-dependencies]` table with `winresource = "0.1"`.
- **`crates/app/build.rs`:** see Appendix A.
- **Pass:** the FileVersion is `0.1.0.0`, ProductName is `VoiceChanger`, `Select-String` prints `False`, and the app still starts (`cargo run --release -p voicechanger`, close it). QG passes; `cargo deny check` accepts winresource and its dependencies.
- **Commit:** `build(app): static CRT and Windows version resource`.

### 2. xtask: extract runtime licence files (licence_extract)
- **Tests first** (in `xtask/src/manifest.rs` tests):
  - `parses_licence_extract`: a runtime with `licence_extract = "LICENSE.txt"`, `dest = "runtime/r.dll"` gives `licence_extract == Some("LICENSE.txt")` and `licence_dest() == Some("licences/r.txt")`.
  - `parses_models_and_runtimes`: also asserts `m.runtime[0].licence_extract == None` and `licence_dest() == None`.
  - `rejects_empty_licence_extract`: the error contains `licence_extract`.
  - `model_rejects_licence_extract`: a `[[model]]` with `licence_extract` fails with an error containing `licence_extract`.
  - `repo_manifest_is_valid`: every runtime has `licence_extract`, and their `licence_dest()` values are distinct.
- **Tests first** (in `xtask/src/fetch.rs` tests): generalise `write_zip(name)` to `write_zip(name, &[(&str, &[u8])])`. Add `extracts_runtime_and_licence`: a zip holding `a/b.dll`=`hi` and `LICENSE.txt`=`lic`, with an Entry `extract="a/b.dll"`, `licence_extract="LICENSE.txt"`, `dest="runtime/b.dll"`. `extract_runtime(&zip, &entry, &vendor)` writes `vendor/runtime/b.dll` and `vendor/licences/b.txt`. Add `extract_runtime_missing_licence_errors`: the error names the missing inner path.
- **Run** `cargo test -p xtask`; it fails to compile (no field or function).
- **Implement:**
  - `Entry` gets `#[serde(default)] pub licence_extract: Option<String>`, documented as "Licence file inside a runtime package, copied to `vendor/licences/<dest file stem>.txt`".
  - `impl Entry { pub fn licence_dest(&self) -> Option<String> }` takes the stem of the file name of `dest`.
  - In `parse`, models reject `licence_extract`, and runtimes reject `Some("")`.
  - `pub fn extract_runtime(package: &Path, entry: &Entry, vendor: &Path) -> Result<(), String>` calls `extract_from_zip` for the DLL and, when set, for the licence. `fetch_all` uses it.
- **Manifest:** run `tar -tf vendor\cache\microsoft.ai.directml.1.15.4.nupkg` and `tar -tf vendor\cache\microsoft.ml.onnxruntime.directml.1.24.4.nupkg`. Set `licence_extract` to the exact licence path in each (expected `LICENSE.txt` for DirectML and `LICENSE` for ONNX Runtime; use what the listing shows).
- **Pass:** `cargo test -p xtask` passes. `cargo xtask fetch-models` creates `vendor\licences\onnxruntime.txt` and `vendor\licences\DirectML.txt`. QG passes.
- **Commit:** `feat(xtask): extract runtime licence files`.

### 3. version.sh with tests; release-guard uses it
- **Files:**
  - `.github/scripts/version.sh`, `.github/scripts/test-version.sh` and both fixtures: exact content in Appendix B.
  - `.gitattributes`: add `*.sh text eol=lf` and `.github/scripts/fixtures/* text eol=lf`.
- **Test first:** write `test-version.sh` and the fixtures. `bash .github/scripts/test-version.sh` fails because `version.sh` doesn't exist yet.
- **Implement** `version.sh`. `bash .github/scripts/test-version.sh` then prints only `ok` lines and exits 0.
- **ci.yml:**
  - In `checks`, after checkout, add a step `Script tests` with `shell: bash` and `run: bash .github/scripts/test-version.sh`.
  - In `release-guard`, replace the inline awk with `version=$(bash .github/scripts/version.sh read)`, keeping the X.Y.Z regex and the tag comparison.
  - Replace the grep changelog check with `bash .github/scripts/version.sh changelog "$version" > /dev/null || { echo "::error file=CHANGELOG.md::CHANGELOG.md has no non-empty '## [$version]' section."; exit 1; }`.
- **Commit:** `ci: shared version.sh for version and changelog reads`.

### 4. Third-party notices (cargo-about + notices-extra)
- **Files:**
  - `about.toml`, `about.hbs`, `assets/notices-extra.md` and `installer/notices.ps1`: Appendix C.
  - `.gitignore`: add `/dist` and `/THIRD_PARTY_NOTICES.md`.
- **Failing check first:** `pwsh -File installer\notices.ps1` errors before the files exist.
- **Pass:** `cargo install cargo-about --locked` if it is missing, then `pwsh -File installer\notices.ps1` writes `THIRD_PARTY_NOTICES.md`. Check that:
  - It contains `## Rust crates`, `eframe`, the DirectML licence text and the ONNX Apache text, and no `<!-- licence:` marker.
  - It does not contain the xtask-only crates `prost-build`, `protox` or `ureq` (`Select-String -Path THIRD_PARTY_NOTICES.md -Pattern 'protox|ureq' -Quiet` prints `False`). If they appear, `--manifest-path` scanned the workspace; fix the invocation.
  - A licence rejected by cargo-about is fixed in `about.toml` only when `deny.toml` already allows it.
  - `git status` doesn't show `THIRD_PARTY_NOTICES.md`.
- **Commit:** `build: generate THIRD_PARTY_NOTICES.md with cargo-about`.

### 5. Inno Setup script and installer/build.ps1
- **Files:** `installer/setup.iss` (Appendix D) and `installer/build.ps1` (Appendix E).
- **Failing check first:** temporarily rename `vendor\runtime\DirectML.dll`. `pwsh -File installer\build.ps1 -Version 0.1.0-dev.1 -FileVersion 0.1.0.0` fails with "Missing installer inputs: vendor\runtime\DirectML.dll …". Restore the file.
- **Wrong version check:** `-FileVersion 0.2.0.0` fails with "file version '0.1.0.0', expected '0.2.0.0'".
- **Pass:**
  - `pwsh -File installer\build.ps1 -Version 0.1.0-dev.1 -FileVersion 0.1.0.0` writes `dist\Setup-VoiceChanger-0.1.0-dev.1.exe`, roughly 150–250 MB because of the models.
  - `(Get-Item dist\Setup-VoiceChanger-0.1.0-dev.1.exe).VersionInfo.FileVersion` is `0.1.0.0`.
  - It needs Inno Setup 6.3 or later (`x64compatible`); the script installs it through choco when it is missing.
- **Commit:** `feat(installer): Inno Setup script and build.ps1`.

### 6. CI: installer smoke build, develop artifacts, workflow_call
Edit `.github/workflows/ci.yml` as in Appendix F:
- `on.workflow_call: {}`.
- The concurrency group becomes `ci-${{ github.workflow }}-${{ github.ref }}`, so release's call to this workflow can't cancel the CI run on `main`.
- cargo-about is installed with cargo-deny.
- New steps `Installer version`, `Installer (smoke build)` and `Upload develop installer`.

Validate with actionlint: download the `rhysd/actionlint` Windows release zip into a new empty temp folder and run `actionlint .github/workflows/ci.yml`; expect no errors. The real check is the CI run on the PR: the `checks` job builds the installer. The artifact upload runs only on pushes to `develop`.
- **Commit:** `ci: installer smoke build and develop artifacts`.

### 7. release.yml: guard, checks, build, sign, checksums, publish
Create `.github/workflows/release.yml` exactly as in Appendix G.
- Check the SignPath action inputs (`api-token`, `organization-id`, `project-slug`, `signing-policy-slug`, `github-artifact-id`, `wait-for-completion`, `output-artifact-directory`) and the permissions it needs against the `signpath/github-action-submit-signing-request` README (Context7 or GitHub), and adjust if they changed.
- Run `actionlint .github/workflows/release.yml .github/workflows/ci.yml` with no errors.
- It can't run before a merge into `main`. Its scripts are covered by `test-version.sh` and its build by the CI smoke build.
- **Commit:** `ci: release workflow`.

### 8. tools/export-hubert: export RVC's MIT hubert_base.pt to ONNX
- **Files:** `tools/export-hubert/requirements.txt`, `export.py` and `README.md`, per Appendix H.
- **Pin the hash first:** `curl.exe -sI https://huggingface.co/lj1995/VoiceConversionWebUI/resolve/e6d0c1a17da07c33557852f9dfa2bd44cc75737d/hubert_base.pt`. The `x-linked-etag` header is the file's SHA-256; hard-code it as `SHA256`. A 404 means the file isn't at this revision; ask the human for a revision.
- **Run** the export with `--out $env:TEMP\vc-hubert --reference vendor\models\hubert.onnx` (the old ContentVec):
  - The self-checks pass: the PyTorch-vs-ONNX max abs diff is ≤ 1e-3, 16080 samples give 50 frames, and the signature matches the reference.
  - The script prints the size and SHA-256 of `hubert.onnx` and the max abs diff against the old model (for information).
  - Record these numbers in the commit body.
- No Rust changes and nothing in `vendor/` is committed.
- **Commit:** `feat(tools): export the MIT HuBERT base model to ONNX`.

### 9. Host hubert.onnx on base-models-1 and switch the manifest (needs human approval)
- **Ask the human first** (ask_human or task comment) and wait for a yes: "Upload hubert.onnx (SHA-256 …, … MB) as an asset of a new prerelease `base-models-1` on mmhy2003/VoiceChanger?" Do not upload without approval.
- **Upload:** `gh release create base-models-1 --repo mmhy2003/VoiceChanger --target develop --prerelease --latest=false --title "Base models 1" --notes "HuBERT base content encoder exported from lj1995/VoiceConversionWebUI hubert_base.pt (MIT) by tools/export-hubert. Not an app release." $env:TEMP\vc-hubert\hubert.onnx`
- **Manifest:** the first `[[model]]` becomes `name = "HuBERT base (RVC, MIT)"`, `url = "https://github.com/mmhy2003/VoiceChanger/releases/download/base-models-1/hubert.onnx"`, `sha256 = <printed hash>`, `licence = "MIT (lj1995/VoiceConversionWebUI)"` and `dest = "models/hubert.onnx"`. Update the header comment so it no longer implies Hugging Face only.
- **Test:** `cargo xtask fetch-models` (run it twice if it reports that the old file was deleted), then QG with `VC_REQUIRE_ASSETS=1`. The hubert and e2e tests must pass.
- **Ear check:** copy two complete asset sets to `%TEMP%\vc-old` and `%TEMP%\vc-new` (old vs new `models\hubert.onnx`, same rmvpe and runtime). Run `vc-convert` on a speech WAV with the same RVC v2 ONNX model, setting `VC_ASSETS_DIR` to each folder. Post both output paths to the human for listening.
- **Commit:** `feat(assets): MIT HuBERT base model replaces GPL ContentVec`.

### 10. Docs: README install/release, AGENTS.md CI/CD, CHANGELOG
- **README:**
  - New `## Install` after Requirements: download `Setup-VoiceChanger-X.Y.Z.exe` from Releases; per-user install with no admin rights (all-users optional); install VB-CABLE (the finish page offers it); unsigned builds may trigger SmartScreen (More info → Run anyway; check `SHA256SUMS.txt`); DirectML only, while CUDA needs a CUDA `onnxruntime.dll`.
  - `## Base models`: "HuBERT base" instead of ContentVec; replace the GPL line with "HuBERT base and RMVPE are MIT (lj1995/VoiceConversionWebUI); `tools/export-hubert/` documents how `hubert.onnx` was made."
  - New `## Releasing` paragraph: merge `develop` into `main` via a release PR; `release.yml` publishes; `pwsh -File installer\build.ps1 -Version X.Y.Z -FileVersion X.Y.Z.0` builds locally after `cargo build --release` and `cargo xtask fetch-models`, and needs Inno Setup 6.3+ and cargo-about.
- **AGENTS.md:**
  - Build: replace the ISCC line with the fetch-models + build.ps1 line.
  - CI/CD:
    - CI builds the installer on every run and uploads `VoiceChanger-X.Y.Z-dev.<run>` on develop pushes (14 days).
    - The release job signs only when the `SIGNPATH_API_TOKEN` secret exists, with repo variables `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG` and `SIGNPATH_POLICY_SLUG`; one artifact configuration signs the `.exe` inside the uploaded zip.
    - `base-models-*` releases host base models and are never app releases.
  - Mention that `.github/scripts/version.sh` is the version reader.
- **CHANGELOG `[Unreleased]`:**
  - Added: "Windows installer (`Setup-VoiceChanger-X.Y.Z.exe`, per-user, no VC++ runtime needed, VB-CABLE check, keeps your models and settings on upgrade) and automatic GitHub Releases with SHA-256 checksums."
  - New `### Changed`: "The content encoder is now RVC's MIT HuBERT base model instead of the GPL-3.0 ContentVec export."
- **Commit:** `docs: install, releasing and HuBERT base`.

### 11. After the tooling PR merges: Sandbox check of the develop artifact (human)
No repository changes. Once the tooling branch is merged into `develop` and CI on `develop` is green:
1. Find the artifact with `gh run list --workflow CI --branch develop`, then `gh run download <id> -n VoiceChanger-0.1.0-dev.<run>`.
2. Post this checklist on the task for the human to run in Windows Sandbox:
   - install per-user with no UAC prompt;
   - the finish page shows the VB-CABLE checkbox when no cable is present, and after VB-CABLE is installed a re-install doesn't show it;
   - Explorer → Properties of `voicechanger.exe` shows file version `0.1.0.0`;
   - the app starts and finds its models;
   - uninstall with "No" keeps `%APPDATA%\VoiceChanger`, and uninstall with "Yes" removes it.
3. Wait for the human's confirmation. Fix any failures in a new feature PR into `develop` first.

### 12. Release PR: 1.0.0 from develop into main
Only after step 11 is confirmed.
1. On an up-to-date `develop`, set `version = "1.0.0"` in `Cargo.toml` and run `cargo build` (this updates the workspace crate versions in `Cargo.lock`).
2. In `CHANGELOG.md`, rename `## [Unreleased]` to `## [1.0.0] - <today, YYYY-MM-DD>` and put a new empty `## [Unreleased]` above it.
3. Replace the README Status paragraph with "1.0.0 is the first release. Download the installer from [Releases](https://github.com/mmhy2003/VoiceChanger/releases)."
4. Run QG and `bash .github/scripts/version.sh changelog 1.0.0`, which prints the notes.
5. Commit `release: 1.0.0` and push to `develop` (allowed by CONTRIBUTING and authorised by the task).
6. `gh pr create --base main --head develop --title "Release 1.0.0" --body …`, then confirm that `release-guard` and `checks` pass.
7. Do not merge: tell the human the PR is ready.

### 13. After the human merges: verify the release and run the smoke checklist
1. `gh run list --workflow Release --branch main`, then `gh run watch <id>`; every job must be green.
2. `gh release view v1.0.0`: it holds `Setup-VoiceChanger-1.0.0.exe` and `SHA256SUMS.txt`, its notes are the 1.0.0 section plus the unsigned line, and it is marked Latest.
3. Download both into a new temp folder and run `Get-FileHash`; the hash must match `SHA256SUMS.txt`.
4. Post the smoke checklist for the human (Sandbox):
   - a clean install of 1.0.0;
   - an upgrade from the develop artifact to 1.0.0 that keeps the imported models and settings and shows 1.0.0 in About;
   - uninstall.
5. Report the results on the task.
6. Merge `main` back into `develop`: `git fetch origin`, then `git push origin origin/main:develop`, only if that is a fast-forward. Otherwise open a PR from `main` into `develop`.

## Final verification
- `bash .github/scripts/test-version.sh` → all `ok`.
- QG passes, with `VC_REQUIRE_ASSETS=1` and the new hubert.
- `pwsh -File installer\build.ps1 -Version 0.1.0-dev.1 -FileVersion 0.1.0.0` → `dist\Setup-VoiceChanger-0.1.0-dev.1.exe`.
- `actionlint .github/workflows/ci.yml .github/workflows/release.yml` → clean.
- The CI run on the tooling PR: `checks` builds the installer. After the merge, the `develop` push uploads `VoiceChanger-0.1.0-dev.<run>`.
- After the release PR merges: Release run green, `v1.0.0` tag and release present, checksum matches, smoke checklist passed.

---

## Appendix A — `crates/app/build.rs`
```rust
//! Embeds the Windows version resource: FileVersion/ProductVersion `X.Y.Z.0` from the workspace version.

fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by cargo");
    let mut parts = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let major = parts.next().unwrap_or(0);
    let minor = parts.next().unwrap_or(0);
    let patch = parts.next().unwrap_or(0);
    let numeric = (major << 48) | (minor << 32) | (patch << 16);
    let text = format!("{major}.{minor}.{patch}.0");
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "VoiceChanger")
        .set("FileDescription", "VoiceChanger")
        .set("CompanyName", "VoiceChanger contributors")
        .set("LegalCopyright", "Copyright (c) 2026 VoiceChanger contributors")
        .set("OriginalFilename", "voicechanger.exe")
        .set("FileVersion", &text)
        .set("ProductVersion", &text)
        .set_version_info(winresource::VersionInfo::FILEVERSION, numeric)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
    res.compile().expect("embedding the Windows version resource");
}
```

## Appendix B — version.sh, tests, fixtures
`.github/scripts/version.sh`:
```bash
#!/usr/bin/env bash
# Version and changelog reads shared by ci.yml (release-guard, dev label) and release.yml.
#   version.sh read [Cargo.toml]               prints [workspace.package] version
#   version.sh changelog X.Y.Z [CHANGELOG.md]  prints the body of "## [X.Y.Z]"; fails if missing or empty
set -euo pipefail

case "${1:-}" in
  read)
    file=${2:-Cargo.toml}
    version=$(awk '
      { sub(/\r$/, "") }
      /^\[workspace\.package\]/ { in_pkg = 1; next }
      /^\[/ { in_pkg = 0 }
      in_pkg && /^version[[:space:]]*=/ {
        sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit
      }' "$file")
    if [ -z "$version" ]; then
      echo "version.sh: no [workspace.package] version in $file" >&2
      exit 1
    fi
    printf '%s\n' "$version"
    ;;
  changelog)
    version=${2:?usage: version.sh changelog X.Y.Z [CHANGELOG.md]}
    file=${3:-CHANGELOG.md}
    body=$(awk -v want="## [$version]" '
      { sub(/\r$/, "") }
      index($0, want) == 1 { found = 1; next }
      found && /^## \[/ { exit }
      found { print }' "$file" | sed '/./,$!d')
    if [ -z "${body//[[:space:]]/}" ]; then
      echo "version.sh: $file has no non-empty '## [$version]' section" >&2
      exit 1
    fi
    printf '%s\n' "$body"
    ;;
  *)
    echo "usage: version.sh read [Cargo.toml] | changelog X.Y.Z [CHANGELOG.md]" >&2
    exit 2
    ;;
esac
```
`.github/scripts/test-version.sh`:
```bash
#!/usr/bin/env bash
# Tests for version.sh. Run: bash .github/scripts/test-version.sh
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
v="$here/version.sh"
fx="$here/fixtures"
fail=0

check() { # name expected actual
  if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: expected [$2], got [$3]"; fail=1; fi
}
status() { "$@" > /dev/null 2>&1 && echo 0 || echo $?; }

check "read fixture skips [package]" "1.2.3" "$(bash "$v" read "$fx/Cargo.fixture.toml")"
check "read repo is X.Y.Z" "1" "$(bash "$v" read "$here/../../Cargo.toml" | grep -cE '^[0-9]+\.[0-9]+\.[0-9]+$')"
check "changelog section" "$(printf '### Added\n- Two.')" "$(bash "$v" changelog 1.2.3 "$fx/CHANGELOG.fixture.md")"
check "changelog last section" "$(printf '### Fixed\n- Three.')" "$(bash "$v" changelog 1.2.1 "$fx/CHANGELOG.fixture.md")"
check "changelog missing fails" "1" "$(status bash "$v" changelog 9.9.9 "$fx/CHANGELOG.fixture.md")"
check "changelog empty fails" "1" "$(status bash "$v" changelog 1.2.2 "$fx/CHANGELOG.fixture.md")"
check "changelog prefix not matched" "1" "$(status bash "$v" changelog 1.2 "$fx/CHANGELOG.fixture.md")"
check "repo changelog has Unreleased" "0" "$(status bash "$v" changelog Unreleased "$here/../../CHANGELOG.md")"
check "no command is usage error" "2" "$(status bash "$v")"
exit $fail
```
`.github/scripts/fixtures/Cargo.fixture.toml`:
```toml
[package]
name = "x"
version = "9.9.9"

[workspace.package]
edition = "2024"
version = "1.2.3"
license = "MIT"
```
`.github/scripts/fixtures/CHANGELOG.fixture.md`:
```md
# Changelog

## [Unreleased]

### Added
- One.

## [1.2.3] - 2026-01-02

### Added
- Two.

## [1.2.2]

## [1.2.1]

### Fixed
- Three.
```

## Appendix C — notices
`about.toml`:
```toml
# cargo-about: licences accepted in THIRD_PARTY_NOTICES.md. Keep in step with deny.toml [licenses].
accepted = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "Unicode-3.0",
    "BSL-1.0",
    "OFL-1.1",
    "Ubuntu-font-1.0",
]
targets = ["x86_64-pc-windows-msvc"]
ignore-build-dependencies = true
ignore-dev-dependencies = true

# Same exception as deny.toml: dirs -> dirs-sys -> option-ext, used unmodified.
[option-ext]
accepted = ["MPL-2.0"]
```
`about.hbs` (Markdown output; triple braces so licence text isn't HTML-escaped):
~~~hbs
## Rust crates

VoiceChanger is built from the Rust crates below. Each licence is listed once, followed by the crates that use it.

{{#each licenses}}
### {{name}}

Used by:
{{#each used_by}}
- {{crate.name}} {{crate.version}}{{#if crate.repository}} ({{crate.repository}}){{/if}}
{{/each}}

```text
{{{text}}}
```

{{/each}}
~~~
`assets/notices-extra.md`:
```md
# Third-party notices

VoiceChanger is MIT-licensed (see LICENSE). It ships with the third-party components below.

## ONNX Runtime 1.24.4 (vendor/runtime/onnxruntime.dll)

Microsoft ONNX Runtime, MIT licence. https://github.com/microsoft/onnxruntime

<!-- licence: onnxruntime -->

## DirectML 1.15.4 (vendor/runtime/DirectML.dll)

Microsoft DirectML, redistributed under the Microsoft DirectML licence. https://www.nuget.org/packages/Microsoft.AI.DirectML

<!-- licence: DirectML -->

## HuBERT base (vendor/models/hubert.onnx)

Content encoder `hubert_base.pt` from lj1995/VoiceConversionWebUI, MIT licence, exported to ONNX by tools/export-hubert. https://huggingface.co/lj1995/VoiceConversionWebUI

## RMVPE (vendor/models/rmvpe.onnx)

Pitch estimator from lj1995/VoiceConversionWebUI, MIT licence. https://huggingface.co/lj1995/VoiceConversionWebUI

## ONNX protocol definition (onnx.proto)

Used to read and write ONNX models in the Convert tab. From the ONNX project, Apache-2.0. https://github.com/onnx/onnx

<!-- licence: onnx-proto -->
```
`installer/notices.ps1`:
```powershell
<#
.SYNOPSIS
Writes THIRD_PARTY_NOTICES.md at the repo root: assets\notices-extra.md with each
"<!-- licence: NAME -->" marker replaced by that licence's text, then the Rust crate
licences from cargo-about. Called by installer\build.ps1.
#>
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$licences = [ordered]@{
    'onnxruntime' = 'vendor\licences\onnxruntime.txt'
    'DirectML'    = 'vendor\licences\DirectML.txt'
    'onnx-proto'  = 'crates\convert\proto\LICENSE'
}

if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    cargo install cargo-about --locked
    if ($LASTEXITCODE -ne 0) { throw 'cargo install cargo-about failed' }
}
$crates = Join-Path $root 'target\notices-crates.md'
cargo about generate --manifest-path (Join-Path $root 'crates\app\Cargo.toml') `
    --config (Join-Path $root 'about.toml') --output-file $crates (Join-Path $root 'about.hbs')
if ($LASTEXITCODE -ne 0) { throw 'cargo about generate failed' }

$text = Get-Content -Raw -Encoding utf8 (Join-Path $root 'assets\notices-extra.md')
foreach ($name in $licences.Keys) {
    $path = Join-Path $root $licences[$name]
    if (-not (Test-Path $path -PathType Leaf)) {
        throw "Missing licence file $($licences[$name]) (run cargo xtask fetch-models)"
    }
    $marker = "<!-- licence: $name -->"
    if (-not $text.Contains($marker)) { throw "assets\notices-extra.md has no '$marker'" }
    $body = (Get-Content -Raw -Encoding utf8 $path).TrimEnd()
    $text = $text.Replace($marker, "~~~text`n$body`n~~~")
}
if ($text -match '<!-- licence: [^>]*-->') { throw "Unknown licence marker in assets\notices-extra.md: $($Matches[0])" }

$out = Join-Path $root 'THIRD_PARTY_NOTICES.md'
$all = $text.TrimEnd() + "`n`n" + (Get-Content -Raw -Encoding utf8 $crates)
[IO.File]::WriteAllText($out, $all, (New-Object Text.UTF8Encoding $false))
Write-Host "wrote $out"
```
Check the cargo-about flags (`generate`, `--manifest-path`, `--config`, `--output-file`) against `cargo about generate --help` of the installed version, and adapt only the flag names if they differ.

## Appendix D — `installer/setup.iss`
```iss
; VoiceChanger installer. Build with installer\build.ps1, which runs
;   ISCC /DAppVersion=<display version> /DFileVersion=X.Y.Z.0 installer\setup.iss
; AppId must never change: it is what makes upgrades install in place.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef FileVersion
  #error Pass /DFileVersion=X.Y.Z.0
#endif
#define AppName "VoiceChanger"
#define AppExe "voicechanger.exe"
#define Root AddBackslash(SourcePath) + ".."

[Setup]
AppId={{7C3B9E52-4A1D-4F6E-9B8A-2D5E1C0F7A34}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=VoiceChanger contributors
AppCopyright=Copyright (c) 2026 VoiceChanger contributors
AppPublisherURL=https://github.com/mmhy2003/VoiceChanger
AppSupportURL=https://github.com/mmhy2003/VoiceChanger/issues
AppUpdatesURL=https://github.com/mmhy2003/VoiceChanger/releases
VersionInfoVersion={#FileVersion}
VersionInfoProductVersion={#FileVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog commandline
UsedUserAreasWarning=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19045
CloseApplications=yes
RestartApplications=no
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
OutputDir={#Root}\dist
OutputBaseFilename=Setup-{#AppName}-{#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Root}\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\vendor\models\hubert.onnx"; DestDir: "{app}\vendor\models"; Flags: ignoreversion
Source: "{#Root}\vendor\models\rmvpe.onnx"; DestDir: "{app}\vendor\models"; Flags: ignoreversion
Source: "{#Root}\vendor\runtime\onnxruntime.dll"; DestDir: "{app}\vendor\runtime"; Flags: ignoreversion
Source: "{#Root}\vendor\runtime\DirectML.dll"; DestDir: "{app}\vendor\runtime"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "{#Root}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Run]
Filename: "https://vb-audio.com/Cable/"; Description: "Open the VB-CABLE download page"; Flags: shellexec postinstall nowait skipifsilent; Check: NoVirtualCable
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent

[Code]
const
  RenderKey = 'SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render';
  { PKEY_Device_DeviceDesc ("CABLE Input") and PKEY_DeviceInterface_FriendlyName ("VB-Audio Virtual Cable"). }
  DeviceDescValue = '{a45c254e-df1c-4efd-8020-67d146a850e0},2';
  InterfaceNameValue = '{b3f8fa53-0004-438e-9003-51a46e139bfc},6';
  DeviceStateActive = 1;

var
  CableChecked: Boolean;
  CableFound: Boolean;

{ Same rule as is_virtual_cable in crates/audio/src/devices.rs. }
function IsVirtualCableName(Name: String): Boolean;
var
  Lower: String;
begin
  Lower := Lowercase(Name);
  Result := (Pos('cable', Lower) > 0) or (Pos('voicemeeter', Lower) > 0)
    or (Pos('virtual audio cable', Lower) > 0);
end;

{ True if an active render endpoint looks like a virtual cable. HKLM is readable without admin. }
function VirtualCableInstalled: Boolean;
var
  Endpoints: TArrayOfString;
  I: Integer;
  State: Cardinal;
  Props, Desc, Iface: String;
begin
  Result := False;
  if not RegGetSubkeyNames(HKLM, RenderKey, Endpoints) then
    Exit;
  for I := 0 to GetArrayLength(Endpoints) - 1 do
  begin
    if RegQueryDWordValue(HKLM, RenderKey + '\' + Endpoints[I], 'DeviceState', State)
      and (State = DeviceStateActive) then
    begin
      Props := RenderKey + '\' + Endpoints[I] + '\Properties';
      Desc := '';
      Iface := '';
      RegQueryStringValue(HKLM, Props, DeviceDescValue, Desc);
      RegQueryStringValue(HKLM, Props, InterfaceNameValue, Iface);
      if IsVirtualCableName(Desc + ' (' + Iface + ')') then
      begin
        Result := True;
        Exit;
      end;
    end;
  end;
end;

function NoVirtualCable: Boolean;
begin
  if not CableChecked then
  begin
    CableFound := VirtualCableInstalled;
    CableChecked := True;
  end;
  Result := not CableFound;
end;

{ Setup never touches user data; the uninstaller asks (default No) and a silent uninstall keeps it. }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Roaming, Local: String;
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  Roaming := ExpandConstant('{userappdata}\{#AppName}');
  Local := ExpandConstant('{localappdata}\{#AppName}');
  if not (DirExists(Roaming) or DirExists(Local)) then
    Exit;
  if UninstallSilent then
    Exit;
  if MsgBox('Also delete your voice models and settings?' + #13#10#13#10 + Roaming + #13#10 + Local,
    mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
  begin
    DelTree(Roaming, True, True, True);
    DelTree(Local, True, True, True);
  end;
end;
```

## Appendix E — `installer/build.ps1`
```powershell
<#
.SYNOPSIS
Builds dist\Setup-VoiceChanger-<Version>.exe. CI's smoke build and release.yml both run it.
.EXAMPLE
pwsh -File installer\build.ps1 -Version 0.1.0-dev.42 -FileVersion 0.1.0.0
Needs target\release\voicechanger.exe (cargo build --release) and vendor\ (cargo xtask fetch-models).
#>
param(
    [Parameter(Mandatory = $true)] [ValidatePattern('^\d+\.\d+\.\d+(-dev\.\d+)?$')] [string] $Version,
    [Parameter(Mandatory = $true)] [ValidatePattern('^\d+\.\d+\.\d+\.0$')] [string] $FileVersion
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\release\voicechanger.exe'

$inputs = 'target\release\voicechanger.exe', 'vendor\models\hubert.onnx', 'vendor\models\rmvpe.onnx',
    'vendor\runtime\onnxruntime.dll', 'vendor\runtime\DirectML.dll', 'LICENSE', 'CHANGELOG.md'
$missing = @($inputs | Where-Object { -not (Test-Path (Join-Path $root $_) -PathType Leaf) })
if ($missing.Count -gt 0) {
    throw "Missing installer inputs: $($missing -join ', '). Run cargo build --release and cargo xtask fetch-models."
}
$exeVersion = (Get-Item $exe).VersionInfo.FileVersion
if ($exeVersion -ne $FileVersion) {
    throw "voicechanger.exe has file version '$exeVersion', expected '$FileVersion'."
}
$image = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
if ($image.IndexOf('VCRUNTIME140', [StringComparison]::OrdinalIgnoreCase) -ge 0) {
    throw 'voicechanger.exe imports VCRUNTIME140.dll; build with the static CRT (.cargo/config.toml).'
}

& (Join-Path $PSScriptRoot 'notices.ps1')

function Find-Iscc {
    $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    @(
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe')
    ) | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
}

$iscc = Find-Iscc
if (-not $iscc) {
    if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
        throw 'Inno Setup 6.3+ not found. Install it from https://jrsoftware.org/isdl.php or with winget install JRSoftware.InnoSetup.'
    }
    choco install innosetup -y --no-progress
    if ($LASTEXITCODE -ne 0) { throw 'choco install innosetup failed' }
    $iscc = Find-Iscc
    if (-not $iscc) { throw 'ISCC.exe not found after installing Inno Setup' }
}

& $iscc /Qp "/DAppVersion=$Version" "/DFileVersion=$FileVersion" (Join-Path $PSScriptRoot 'setup.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed with exit code $LASTEXITCODE" }
$setup = Join-Path $root "dist\Setup-VoiceChanger-$Version.exe"
if (-not (Test-Path $setup -PathType Leaf)) { throw "ISCC did not write $setup" }
Write-Host "built $setup"
```

## Appendix F — `ci.yml` changes
```yaml
on:
  push:
    branches: [develop, main]
  pull_request:
    branches: [develop, main]
  workflow_call: {}

concurrency:
  # github.workflow is the caller's name under workflow_call, so release.yml never cancels CI on main.
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
```
In `checks`:
- Add `Script tests` (step 3) after checkout.
- Change the cargo-deny install to `tool: cargo-deny,cargo-about`.
- Append after `Release build`:
```yaml
      - name: Installer version
        id: installer
        shell: bash
        run: |
          set -euo pipefail
          version=$(bash .github/scripts/version.sh read)
          echo "display=$version-dev.${{ github.run_number }}" >> "$GITHUB_OUTPUT"
          echo "file=$version.0" >> "$GITHUB_OUTPUT"
      - name: Installer (smoke build)
        shell: pwsh
        run: ./installer/build.ps1 -Version '${{ steps.installer.outputs.display }}' -FileVersion '${{ steps.installer.outputs.file }}'
      - name: Upload develop installer
        if: github.event_name == 'push' && github.ref == 'refs/heads/develop'
        uses: actions/upload-artifact@v4
        with:
          name: VoiceChanger-${{ steps.installer.outputs.display }}
          path: dist/Setup-VoiceChanger-${{ steps.installer.outputs.display }}.exe
          retention-days: 14
          if-no-files-found: error
```

## Appendix G — `.github/workflows/release.yml`
```yaml
name: Release

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: release
  cancel-in-progress: false

env:
  CARGO_TERM_COLOR: always

jobs:
  prepare:
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    outputs:
      version: ${{ steps.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - name: Version, tag and changelog guard
        id: version
        run: |
          set -euo pipefail
          version=$(bash .github/scripts/version.sh read)
          if ! [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "::error file=Cargo.toml::[workspace.package] version '$version' is not plain X.Y.Z."
            exit 1
          fi
          if git ls-remote --exit-code --tags origin "refs/tags/v$version" > /dev/null; then
            echo "::error file=Cargo.toml::Tag v$version already exists. Bump the version in a release PR."
            exit 1
          fi
          bash .github/scripts/version.sh changelog "$version" > /dev/null
          echo "version=$version" >> "$GITHUB_OUTPUT"

  checks:
    uses: ./.github/workflows/ci.yml

  build:
    needs: [prepare, checks]
    runs-on: windows-latest
    permissions:
      contents: read
      actions: read
    outputs:
      signed: ${{ steps.signing.outputs.enabled }}
    env:
      VERSION: ${{ needs.prepare.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Cache base models
        uses: actions/cache@v4
        with:
          path: vendor
          key: vendor-${{ hashFiles('assets/base-models.toml') }}
      - name: Fetch base models
        run: cargo xtask fetch-models
      - name: Release build
        run: cargo build --release
      - uses: taiki-e/install-action@v2
        with:
          tool: cargo-about
      - name: Signing enabled?
        id: signing
        shell: bash
        env:
          SIGNPATH_API_TOKEN: ${{ secrets.SIGNPATH_API_TOKEN }}
        run: |
          if [ -n "$SIGNPATH_API_TOKEN" ]; then echo "enabled=true"; else echo "enabled=false"; fi >> "$GITHUB_OUTPUT"
      - name: Upload exe for signing
        id: exe-unsigned
        if: steps.signing.outputs.enabled == 'true'
        uses: actions/upload-artifact@v4
        with:
          name: voicechanger-exe-unsigned
          path: target/release/voicechanger.exe
          if-no-files-found: error
      - name: Sign exe
        if: steps.signing.outputs.enabled == 'true'
        uses: signpath/github-action-submit-signing-request@v1
        with:
          api-token: ${{ secrets.SIGNPATH_API_TOKEN }}
          organization-id: ${{ vars.SIGNPATH_ORGANIZATION_ID }}
          project-slug: ${{ vars.SIGNPATH_PROJECT_SLUG }}
          signing-policy-slug: ${{ vars.SIGNPATH_POLICY_SLUG }}
          github-artifact-id: ${{ steps.exe-unsigned.outputs.artifact-id }}
          wait-for-completion: true
          output-artifact-directory: target/release
      - name: Installer
        shell: pwsh
        run: ./installer/build.ps1 -Version $env:VERSION -FileVersion "$env:VERSION.0"
      - name: Upload installer for signing
        id: setup-unsigned
        if: steps.signing.outputs.enabled == 'true'
        uses: actions/upload-artifact@v4
        with:
          name: setup-unsigned
          path: dist/Setup-VoiceChanger-${{ env.VERSION }}.exe
          if-no-files-found: error
      - name: Sign installer
        if: steps.signing.outputs.enabled == 'true'
        uses: signpath/github-action-submit-signing-request@v1
        with:
          api-token: ${{ secrets.SIGNPATH_API_TOKEN }}
          organization-id: ${{ vars.SIGNPATH_ORGANIZATION_ID }}
          project-slug: ${{ vars.SIGNPATH_PROJECT_SLUG }}
          signing-policy-slug: ${{ vars.SIGNPATH_POLICY_SLUG }}
          github-artifact-id: ${{ steps.setup-unsigned.outputs.artifact-id }}
          wait-for-completion: true
          output-artifact-directory: dist
      - name: Checksums
        shell: pwsh
        run: |
          $name = "Setup-VoiceChanger-$env:VERSION.exe"
          $hash = (Get-FileHash -Algorithm SHA256 (Join-Path $PWD "dist/$name")).Hash.ToLowerInvariant()
          [IO.File]::WriteAllText((Join-Path $PWD 'dist/SHA256SUMS.txt'), "$hash  $name`n")
      - uses: actions/upload-artifact@v4
        with:
          name: release
          path: |
            dist/Setup-VoiceChanger-${{ env.VERSION }}.exe
            dist/SHA256SUMS.txt
          if-no-files-found: error

  publish:
    needs: [prepare, build]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    env:
      VERSION: ${{ needs.prepare.outputs.version }}
      SIGNED: ${{ needs.build.outputs.signed }}
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with:
          name: release
          path: dist
      - name: Verify checksums
        working-directory: dist
        run: sha256sum -c SHA256SUMS.txt
      - name: Release notes
        env:
          UNSIGNED_NOTE: 'This build is not code-signed yet. Windows SmartScreen may show "Windows protected your PC": choose More info, then Run anyway. Check the download against SHA256SUMS.txt.'
        run: |
          set -euo pipefail
          bash .github/scripts/version.sh changelog "$VERSION" > notes.md
          if [ "$SIGNED" != "true" ]; then
            printf '\n%s\n' "$UNSIGNED_NOTE" >> notes.md
          fi
      - name: Tag and publish
        env:
          GH_TOKEN: ${{ github.token }}
        run: >-
          gh release create "v$VERSION" --target "$GITHUB_SHA"
          --title "VoiceChanger $VERSION" --notes-file notes.md
          "dist/Setup-VoiceChanger-$VERSION.exe" dist/SHA256SUMS.txt
```
How this meets the design:
- A failed run publishes nothing and creates no tag, so `workflow_dispatch` can re-run it.
- A run whose tag already exists stops in `prepare`.
- When the secret is set, a failed or timed-out signing fails the job; there is no fallback to unsigned.

## Appendix H — tools/export-hubert
`requirements.txt` (dev only, like export-templates):
```
# Dev-only: exports hubert.onnx from RVC's MIT hubert_base.pt. Never needed at runtime or in CI.
#   pip install -r requirements.txt --extra-index-url https://download.pytorch.org/whl/cpu
torch==2.4.1+cpu
torchaudio==2.4.1+cpu
onnx==1.16.2
onnxruntime==1.19.2
numpy==1.26.4
```
`export.py` contract: `python -I export.py --out <dir> [--reference <old hubert.onnx>] [--cache <dir>]`.

1. **Constants:**
   - `REVISION = "e6d0c1a17da07c33557852f9dfa2bd44cc75737d"`
   - `URL = f"https://huggingface.co/lj1995/VoiceConversionWebUI/resolve/{REVISION}/hubert_base.pt"`
   - `SHA256` = the x-linked-etag value from step 8
   - `INPUT_NAME = "source"` (shape `[1, 1, N]` float32) and `OUTPUT_NAME = "embed"` (shape `[1, T, 768]`)
   - `OPSET = 17` and `TOLERANCE = 1e-3`
2. **Download:** download to `--cache` (default `%TEMP%\vc-hubert-cache`) unless the file is already there with the right hash. Verify the SHA-256 and exit 1 on a mismatch.
3. **Restricted load (never runs pickle contents):**
   - A `RestrictedUnpickler(pickle.Unpickler)` whose `find_class` returns the real object only for `("collections","OrderedDict")`, `("torch._utils","_rebuild_tensor_v2")`, `("torch._utils","_rebuild_parameter")` and `("torch", "<X>Storage")`. Anything else (fairseq args, omegaconf, enums) gets an inert `_Stub` class whose `__init__` and `__setstate__` accept and ignore everything.
   - Expose it through a `types.ModuleType("restricted_pickle")` that has `Unpickler` and `load`.
   - Call `torch.load(path, map_location="cpu", weights_only=False, pickle_module=restricted)` and take `ckpt["model"]`.
4. **Map the weights:**
   - `model = torchaudio.models.hubert_base()`; `converted = torchaudio.models.wav2vec2.utils.import_fairseq._convert_state_dict(state)`. Drop keys listed in `UNUSED: dict[str, str]` (key → reason, e.g. `label_embs_concat`, `final_proj.weight`, `final_proj.bias`, `mask_emb`) before conversion if `_convert_state_dict` rejects them.
   - `missing, unexpected = model.load_state_dict({k: v.float() ...}, strict=False)`. Exit 1 if `missing` is non-empty or if any unexpected key is not in `UNUSED`, the same self-check style as export-templates.
5. **Wrapper:** `Encoder(nn.Module).forward(source)` returns `self.hubert.extract_features(source.squeeze(1), num_layers=12)[0][-1]`. This is layer 12, 768-dim, the same features RVC v2 takes from fairseq `extract_features(output_layer=12)`.
6. **Export:** `torch.onnx.export(Encoder(model).eval(), (torch.zeros(1,1,16080),), out/"hubert.onnx", input_names=[INPUT_NAME], output_names=[OUTPUT_NAME], dynamic_axes={INPUT_NAME:{2:"samples"}, OUTPUT_NAME:{1:"frames"}}, opset_version=OPSET)`.
7. **Checks with onnxruntime CPU:**
   - Inputs are a seeded synthetic voiced signal (3 s at 16 kHz: harmonics of a 120–220 Hz glide plus noise at 0.01, seed 1234), 16080 samples (expect exactly 50 frames) and 400 samples (expect ≥ 1 frame).
   - Max abs diff between ONNX and eager PyTorch must be ≤ `TOLERANCE`; otherwise exit 1.
   - With `--reference`, the old model's input/output count, rank and last dimension must match (exit 1 if not). Print the max abs diff against the old model's output for information.
8. **Print** the size in MB and the SHA-256 of `hubert.onnx`.

`README.md` follows `tools/export-templates/README.md`: purpose, pinned revision and hash, licence (MIT, lj1995/VoiceConversionWebUI), venv setup (`py -3.11 -m venv $env:TEMP\vc-hubert-venv`, pip install with the CPU index), the run command, that the hosted file on release `base-models-1` and its hash in `assets/base-models.toml` are authoritative, and how to publish a new `base-models-N` release (ask the maintainer first, prerelease, `--latest=false`).
