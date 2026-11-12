# VoiceChanger

Real-time AI voice changer for Windows, written in Rust. Runs RVC voice models locally on your GPU via ONNX Runtime, with no Python required.

## Status

Pre-alpha. Milestones M0 (project setup) and M1 (audio skeleton) are done. M2 (the inference engine with offline conversion) is in progress. Nothing is released.

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

- `--sr 32000|40000|48000` is needed when the model has no `sr` metadata.
- Only RVC WebUI ONNX exports with F0 (fp32) are supported. fp16 exports and models without F0 are rejected.
- `--provider dml` (the default) uses DirectML. `--provider cuda` needs a CUDA build of `onnxruntime.dll` in place of the DirectML one. If a provider fails, the engine falls back to the CPU with a warning.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests target `develop`.

## Licence

MIT. See [LICENSE](LICENSE).
