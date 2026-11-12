# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses `X.Y.Z` versions as described in [AGENTS.md](AGENTS.md).

## [Unreleased]

### Added
- Project setup: Rust workspace (`app`, `audio`, `engine`, `models`, `convert`), MIT licence, contributor guides, agent guides and CI on Windows.
- Audio skeleton: Voice / Models / Convert / Settings tabs, microphone, virtual cable and monitor selection, mic passthrough with live level meters, and a clean stop with a message when a device is unplugged.
- Inference engine: ONNX Runtime (DirectML, CUDA or CPU with automatic fallback) running ContentVec, RMVPE and RVC v2 WebUI ONNX exports, `cargo xtask fetch-models` for pinned base models, and the `vc-convert` CLI for offline WAV conversion.
