# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses `X.Y.Z` versions as described in [AGENTS.md](AGENTS.md).

## [Unreleased]

## [1.0.0] - 2026-11-13

### Added
- Project setup: Rust workspace (`app`, `audio`, `engine`, `models`, `convert`), MIT licence, contributor guides, agent guides and CI on Windows.
- Audio skeleton: Voice / Models / Convert / Settings tabs, microphone, virtual cable and monitor selection, mic passthrough with live level meters, and a clean stop with a message when a device is unplugged.
- Inference engine: ONNX Runtime (DirectML, CUDA or CPU with automatic fallback) running ContentVec, RMVPE and RVC v2 WebUI ONNX exports, `cargo xtask fetch-models` for pinned base models, and the `vc-convert` CLI for offline WAV conversion.
- Real-time voice conversion: pick an RVC v2 ONNX model on the Voice tab, live pitch, input/output gain and noise gate, SOLA crossfaded blocks with Performance settings (provider, block, extra context, crossfade), a status bar with measured latency, inference time and provider, and `vc-convert --stream` for benchmarking.
- Model library and saved settings: import RVC v2 ONNX models by picker or drag-and-drop into `%APPDATA%\VoiceChanger\models`, rename, delete, reorder, default pitch and icons on the Models tab, switch models while running, `.pth` hand-off to the Convert tab, settings remembered in `%APPDATA%\VoiceChanger\config\settings.toml` (devices by endpoint ID, last model, pitch per model, all controls), a global Start/Stop hotkey, start minimised, daily rotating logs in `%LOCALAPPDATA%\VoiceChanger\logs` and an About box.
- Convert tab: turn PyTorch RVC v2 `.pth` models (or a `.zip` holding one) into ONNX inside the app, with no Python. A restricted reader that never runs file contents, embedded weight-free 32/40/48 kHz templates, a CPU check before the model is added, progress and Cancel, then Use now / Show in Models / Export .onnx….
- Windows installer (`Setup-VoiceChanger-X.Y.Z.exe`, per-user, no VC++ runtime needed, VB-CABLE check, keeps your models and settings on upgrade, `unins000.exe /VERYSILENT /PURGEDATA` also removes them) and automatic GitHub Releases with SHA-256 checksums. Every build installs, upgrades and uninstalls the installer on a clean CI runner (`voicechanger.exe --self-test` runs the bundled models) before it can be released.

### Changed
- The content encoder is now RVC's MIT HuBERT base model instead of the GPL-3.0 ContentVec export.
