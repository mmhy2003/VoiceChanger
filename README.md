# VoiceChanger

Real-time AI voice changer for Windows, written in Rust. Runs RVC voice models locally on your GPU via ONNX Runtime, with no Python required.

## Status

1.0.0 is the first release. Download the installer from [Releases](https://github.com/mmhy2003/VoiceChanger/releases).

## Requirements

- Windows 10 (22H2) or Windows 11, x64
- Rust stable (the version is pinned by `rust-toolchain.toml`; install with [rustup](https://rustup.rs))
- A virtual audio cable (e.g. VB-Audio Virtual Cable) to use the app once it exists

## Install

1. Download `Setup-VoiceChanger-X.Y.Z.exe` from [GitHub Releases](https://github.com/mmhy2003/VoiceChanger/releases).
2. Run it. It installs for the current user without admin rights; installing for all users is optional in the setup dialog.
3. Install [VB-CABLE](https://vb-audio.com/Cable/). When no virtual cable is found, the last setup page offers to open its download page.

Releases may be unsigned for now, so Windows SmartScreen may warn. Choose **More info** → **Run anyway**, after checking the installer against `SHA256SUMS.txt` from the same release.

The installer uses DirectML only. CUDA needs a CUDA build of `onnxruntime.dll` in place of the DirectML one.

## Building

    cargo build --release
    cargo test --workspace
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo deny check

The binary is `target\release\voicechanger.exe`.

## Base models

    cargo xtask fetch-models

This downloads HuBERT base, RMVPE and the DirectML ONNX Runtime DLLs (about 950 MB on disk) into `vendor/` and checks every file against the SHA-256 pinned in `assets/base-models.toml`. Run it before `cargo test`. Files that are already present and match are skipped.

HuBERT base and RMVPE are MIT (lj1995/VoiceConversionWebUI) and are never committed; [`tools/export-hubert/`](tools/export-hubert/README.md) documents how `hubert.onnx` was made. Set `VC_ASSETS_DIR` to use another location than `vendor/`.

## Offline conversion

    cargo run -p vc-engine --bin vc-convert --release -- <in.wav> <out.wav> --model m.onnx [--sr N] [--pitch N] [--gate-db N] [--provider dml|cuda|cpu] [--device N] [--out-rate N]
    cargo run -p vc-engine --bin vc-convert --release -- <in.wav> <out.wav> --model m.onnx --stream [--block-ms N] [--context-ms N] [--crossfade-ms N]

The `--stream` form runs the real-time streamer over the file and prints the per-block inference p50/p95/max and the p95 as a share of the block length.

- `--sr 32000|40000|48000` is needed when the model has no `sr` metadata.
- Only RVC WebUI ONNX exports with F0 (fp32) are supported. fp16 exports and models without F0 are rejected.
- `--provider dml` (the default) uses DirectML. `--provider cuda` needs a CUDA build of `onnxruntime.dll` in place of the DirectML one. If a provider fails, the engine falls back to the CPU with a warning.

## Model library and settings

Import a voice on the Models tab with **Import…** or by dropping `.onnx` files on the window. Each model is checked and copied into the library. If the model does not say its sample rate, the app asks for it (32, 40 or 48 kHz). A dropped or picked `.pth` or `.zip` is not imported: the app offers to open it in the Convert tab instead. Dropping it on the Convert tab opens it there directly.

VoiceChanger keeps its files in four folders:

| Folder | Contents |
|---|---|
| `%APPDATA%\VoiceChanger\models` | The imported models, their icons and the `library.toml` index |
| `%APPDATA%\VoiceChanger\config` | `settings.toml`: devices, last model, pitch per model and every control |
| `%LOCALAPPDATA%\VoiceChanger\logs` | Daily rotating log files (Settings → General → **Open logs folder**) |
| `%LOCALAPPDATA%\VoiceChanger\updates` | Downloaded installers; old ones are deleted at startup |

The global Start/Stop hotkey is off by default. Set it in Settings → General; the combination needs at least one modifier (Ctrl, Alt or Shift); Esc cancels recording.

If `settings.toml` or `library.toml` cannot be read, it is kept as `settings.toml.bad` or `library.toml.bad` and the app starts with defaults or an empty library. Model files are never deleted.

## Updates and privacy

- Updates are the app's only network use. Every 6 hours it asks `api.github.com` for the latest release of `mmhy2003/VoiceChanger`. It downloads the installer and `SHA256SUMS.txt` from GitHub only for an update. No account, token or personal data is sent, only a `VoiceChanger/X.Y.Z` user agent.
- You can turn this off in Settings → General → "Check for updates automatically"; **Check for updates** still works on demand. Drafts and pre-releases are never offered.
- Downloads go to `%LOCALAPPDATA%\VoiceChanger\updates`. A file whose SHA-256 (or signature, once releases are signed) does not match is deleted.
- Updating stops the voice, runs the installer with `/SILENT /SUPPRESSMSGBOXES /NORESTART /RELAUNCH` (log: `%LOCALAPPDATA%\VoiceChanger\logs\update-install.log`), and reopens the app. An all-users install asks for UAC.

## Converting PyTorch models

The Convert tab turns a PyTorch RVC v2 voice model (a `.pth` file, or a `.zip` holding one) into an ONNX model and adds it to the library. No Python is needed.

1. Press **Choose file…** or drop the file on the Convert tab. The app reads it and shows its version, sample rate, pitch (F0), speakers and size.
2. Set the name, the default pitch and, if you like, an icon.
3. Press **Convert**. Conversion runs in the background with a progress bar; **Cancel** stops it and adds nothing. The voice changer keeps running meanwhile.
4. The converted model is checked on the CPU (a short synthetic input; the output must have the right shape, no NaN or infinite values and not be silent) before it is added. When it is done, choose **Use now**, **Show in Models**, **Export .onnx…** or **Convert another…**.

Supported: RVC v2 models with pitch (F0) at 32, 40 or 48 kHz. Not supported: RVC v1 models, training checkpoints (`G_*.pth`, `D_*.pth`), models without F0 and `.safetensors`. Each case gets a short message; the details go to the log.

`.pth` files are pickles, which can run code when Python loads them. The app never executes file contents: it reads them with its own restricted reader, which accepts only PyTorch's tensor-rebuild functions and `OrderedDict` and refuses everything else.

The converter fills weight-free ONNX templates (`assets/templates/`, embedded in the binary). They are made once with Python by `tools/export-templates/` (see [its README](tools/export-templates/README.md)); only developers changing the templates need Python.

To test the converter against a set of public RVC v2 models (pinned in `assets/test-models.toml`, downloaded into `vendor/test-models/`):

    cargo xtask fetch-test-models
    cargo test -p vc-convert --release --test corpus -- --ignored --nocapture

The ONNX types in `crates/convert/src/onnx/onnx.rs` are generated; regenerate them with `cargo xtask gen-onnx`.

## Releasing

Releases are made by a release PR from `develop` into `main` (see [AGENTS.md](AGENTS.md)). When it is merged, `.github/workflows/release.yml` tags `vX.Y.Z` and publishes the GitHub Release with the installer and `SHA256SUMS.txt`.

There is no manual test step. CI and the release workflow run `installer\test-install.ps1` on a clean Windows runner. It installs per user, upgrades from the latest release (keeping user data), runs `voicechanger.exe --self-test` on the bundled models, checks the VB-CABLE prompt against fake cable endpoints, and uninstalls with and without `/PURGEDATA`. The release is only tagged and published when it passes. Afterwards the workflow downloads the published installer and checks it against `SHA256SUMS.txt`. The runners have no GPU or audio devices, so DirectML and real audio are not covered. The script deletes `%APPDATA%\VoiceChanger`, so it refuses to run where VoiceChanger is installed or has data.

To build the installer locally (needs Inno Setup 6.3+ and cargo-about):

    cargo build --release
    cargo xtask fetch-models
    pwsh -File installer\build.ps1 -Version X.Y.Z -FileVersion X.Y.Z.0

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests target `develop`.

## Licence

MIT. See [LICENSE](LICENSE).
