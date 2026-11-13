# VoiceChanger

Real-time AI voice changer for Windows, written in Rust. Runs RVC voice models locally on your GPU via ONNX Runtime, with no Python required.

## Status

Pre-alpha. Milestones M0 to M3 are done and M4 (model library, saved settings and hotkey) is in progress. Nothing is released.

## Requirements

- Windows 10 (22H2) or Windows 11, x64
- Rust stable (the version is pinned by `rust-toolchain.toml`; install with [rustup](https://rustup.rs))
- A virtual audio cable (e.g. VB-Audio Virtual Cable) to use the app once it exists

## Building

    cargo build --release
    cargo test --workspace
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo deny check

The binary is `target\release\voicechanger.exe`.

## Base models

    cargo xtask fetch-models

This downloads ContentVec, RMVPE and the DirectML ONNX Runtime DLLs (about 950 MB on disk) into `vendor/` and checks every file against the SHA-256 pinned in `assets/base-models.toml`. Run it before `cargo test`. Files that are already present and match are skipped.

ContentVec is GPL-3.0 and is never committed. Set `VC_ASSETS_DIR` to use another location than `vendor/`.

## Offline conversion

    cargo run -p vc-engine --bin vc-convert --release -- <in.wav> <out.wav> --model m.onnx [--sr N] [--pitch N] [--gate-db N] [--provider dml|cuda|cpu] [--device N] [--out-rate N]
    cargo run -p vc-engine --bin vc-convert --release -- <in.wav> <out.wav> --model m.onnx --stream [--block-ms N] [--context-ms N] [--crossfade-ms N]

The `--stream` form runs the real-time streamer over the file and prints the per-block inference p50/p95/max and the p95 as a share of the block length.

- `--sr 32000|40000|48000` is needed when the model has no `sr` metadata.
- Only RVC WebUI ONNX exports with F0 (fp32) are supported. fp16 exports and models without F0 are rejected.
- `--provider dml` (the default) uses DirectML. `--provider cuda` needs a CUDA build of `onnxruntime.dll` in place of the DirectML one. If a provider fails, the engine falls back to the CPU with a warning.

## Model library and settings

Import a voice on the Models tab with **Import…** or by dropping `.onnx` files on the window. Each model is checked and copied into the library. If the model does not say its sample rate, the app asks for it (32, 40 or 48 kHz). A dropped or picked `.pth` or `.zip` is not imported: the app offers to open it in the Convert tab instead.

VoiceChanger keeps its files in three folders:

| Folder | Contents |
|---|---|
| `%APPDATA%\VoiceChanger\models` | The imported models, their icons and the `library.toml` index |
| `%APPDATA%\VoiceChanger\config` | `settings.toml`: devices, last model, pitch per model and every control |
| `%LOCALAPPDATA%\VoiceChanger\logs` | Daily rotating log files (Settings → General → **Open logs folder**) |

The global Start/Stop hotkey is off by default. Set it in Settings → General; the combination needs at least one modifier (Ctrl, Alt or Shift); Esc cancels recording.

If `settings.toml` or `library.toml` cannot be read, it is kept as `settings.toml.bad` or `library.toml.bad` and the app starts with defaults or an empty library. Model files are never deleted.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests target `develop`.

## Licence

MIT. See [LICENSE](LICENSE).
