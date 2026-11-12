# VC-4 — M2: Inference pipeline (ONNX Runtime + HuBERT + RMVPE + RVC v2, offline WAV test)

## Goal
`crates/engine` (`vc-engine`) can turn mono audio into an RVC v2 voice with ONNX Runtime: 16 kHz resample → noise gate → HuBERT/ContentVec → RMVPE F0 → pitch shift → RVC v2 generator → resample. It runs on DirectML by default, or on CUDA when asked, and falls back to the CPU on its own. A developer can convert a WAV file end to end with a small CLI. The real-time path (streaming, SOLA crossfade, stats, UI) is M3, so the app's behaviour does not change in M2.

## Decisions
- **Model format: the RVC WebUI official ONNX export** (your answer). This is the signature from RVC-Project `infer/modules/onnx/export.py`:
  - inputs: `phone [1,T,768] f32`, `phone_lengths [1] i64`, `pitch [1,T] i64` (coarse, 1–255), `pitchf [1,T] f32` (Hz), `ds [1] i64` (speaker id), `rnd [1,192,T] f32`;
  - output: `audio [1,1,N] f32`.
  
  This resolves PRD §17, and the M5 templates will export to the same signature. fp16 exports and models without F0 are rejected with a clear error.
- **Base models come from an explicit `cargo xtask fetch-models` step** (your answer). It is not in `build.rs` and does not run at app runtime, so `cargo build` stays offline.
- **ONNX Runtime uses `ort` `load-dynamic`** (your answer). The same xtask fetches Microsoft's official DirectML `onnxruntime.dll` and `DirectML.dll` (from the NuGet packages, pinned by version and SHA-256).
  - CUDA support covers choosing the provider and the fallback logic. It only works if a CUDA build of `onnxruntime.dll` is placed next to the exe; shipping one stays an M6/PRD §17 question.
- **Offline test: a CLI plus a local end-to-end test; CI runs the unit tests** (your answer).
  - CI fetches the base models (cached) and runs on the CPU the tests that load and warm up the real HuBERT and RMVPE sessions.
  - The full voice conversion test needs a user-supplied model through `VC_TEST_MODEL`, so it runs only locally.
- **Block-shaped API now.** The pipeline converts one block with its extra context, and the offline driver cuts the file into blocks. M3 reuses the same call from the worker thread instead of rewriting it.
- **The generator sample rate comes from metadata or a flag.** WebUI exports carry no sample rate, so it is read from `metadata_props["sr"]` when present (M5 will write it). Otherwise the CLI needs `--sr 32000|40000|48000`.

## Architecture
New workspace member `xtask/` (`members = ["crates/*", "xtask"]`, with a `.cargo/config.toml` alias `xtask = "run -p xtask --"`).

**`assets/base-models.toml`** (committed) lists two kinds of entries:
- `[[model]]`: `hubert.onnx` (ContentVec/HuBERT base giving 768-dim layer-12 features, as RVC v2 needs) and `rmvpe.onnx`.
- `[[runtime]]`: the `Microsoft.ML.OnnxRuntime.DirectML` and `Microsoft.AI.DirectML` nupkgs, each with the path of its DLL inside the package.

Each entry has `url` (pinned to an immutable revision, e.g. a Hugging Face commit hash), `sha256`, `licence` and `dest`. The implementer picks the exact sources and records their licences. If no suitable pinned public ONNX of ContentVec exists, they stop and ask rather than hosting one ad hoc.

**`xtask fetch-models`**
- Downloads into the gitignored `vendor/` (`vendor/models/*.onnx`, `vendor/runtime/*.dll`) and checks the SHA-256 of every file.
- Skips files that are already present with the right hash. A hash mismatch deletes the file and fails.
- Extracts the DLLs from the nupkgs, which are zips.
- Uses `ureq` (with native TLS where possible, to avoid widening `deny.toml`), `sha2`, `zip`, `toml` and `serde`. It is a dev-only crate and is not shipped.

**`crates/engine` modules**
- `assets.rs`: finds `vendor/` next to the exe first, then the workspace `vendor/`. The `VC_ASSETS_DIR` environment variable overrides both. If a file is missing, the error names it and says to run `cargo xtask fetch-models`.
- `runtime.rs`
  - `init()` calls `ort::init_from(onnxruntime.dll)` once.
  - `Provider { DirectMl { device: i32 }, Cuda { device: i32 }, Cpu }`.
  - `open_session(path, provider) -> (Session, ActiveProvider)` registers the chosen execution provider with `error_on_failure`. If that fails, it logs the reason and builds the session on the CPU. The active provider is returned so M3 can display it.
- `dsp/resample.rs`: one-shot and stateful `rubato` resamplers (input rate ↔ 16 kHz, model rate → output rate).
- `dsp/gate.rs`: a noise gate on 10 ms RMS frames with threshold (default -50 dB), hold and a short attack/release ramp. It outputs the gated audio and a per-frame "open" mask.
- `dsp/mel.rs`: the RMVPE log-mel front end (n_fft 1024, hop 160, 128 mels, 30–8000 Hz), using `rustfft` with the window and filterbank precomputed.
- `f0.rs`
  - `Rmvpe` runs the session on the mel and decodes the salience `[T,360]` to Hz with a local weighted average around the argmax (threshold 0.03, otherwise unvoiced = 0).
  - Also: `shift(f0, semitones)` (−24…+24) and `coarse(f0)` (the RVC mel-scale mapping to 1–255, with 50–1100 Hz bounds).
  - The gate mask forces closed frames to unvoiced.
- `hubert.rs`: `ContentEncoder` takes 16 kHz audio `[1,N]` and returns features `[T50,768]`. They are upsampled ×2 by repetition to the 100 Hz frame rate of the F0.
- `generator.rs`: `Generator::load(path, provider, sr_override)`.
  - Checks the input and output names, dtypes and ranks against the WebUI signature. It rejects fp16, missing `pitch`/`pitchf` and a 256-dim `phone` (RVC v1).
  - Resolves the sample rate and runs the generator with a seeded `rnd`.
- `pipeline.rs`: `Pipeline::new(PipelineConfig) -> Result<Pipeline>` loads the three sessions and **warms them up** with one silent block of the configured size.
  - `convert(&mut self, block_16k: &[f32], context: usize, params: &Params) -> Result<Vec<f32>>` returns model-rate audio for the block without its context.
  - `Params { pitch_semitones, gate_db, speaker_id }`.
  - It has no threads of its own. M3 calls it from the inference worker.
- `offline.rs`: `convert_file(input: &Path, output: &Path, opts)`.
  - Reads the WAV with `hound` (any format) and downmixes it to mono.
  - Resamples to 16 kHz and converts in blocks (default 10 s with 0.5 s of context each side).
  - Resamples to the input's rate, or `--out-rate`, and writes 32-bit float WAV.
- `src/bin/vc-convert.rs`: `vc-convert <in.wav> <out.wav> --model m.onnx [--sr N] [--pitch N] [--gate-db N] [--provider dml|cuda|cpu] [--device N]`. It prints the active provider, the warm-up time and the real-time factor.

No changes to `crates/app`, `crates/audio` or `crates/models` in M2.

## Data and interfaces
- New dependencies of `vc-engine`: `ort` 2.x (`load-dynamic`, `directml`, `cuda`, `ndarray`), `ndarray`, `rubato`, `rustfft`, `hound`, `thiserror`, `rand` (seeded), and `tracing`, which is only logged if a subscriber is installed. All must pass `cargo deny check`; any licence added to the allow-list gets a reason in the PR.
- `EngineError` (thiserror) has these variants:
  - `MissingAsset { path }`;
  - `RuntimeInit`;
  - `UnsupportedModel { reason }` (for example "fp16 export", "RVC v1 (256-dim)", "no F0 inputs", "unknown sample rate: pass --sr");
  - `Inference`;
  - `Wav`.
- `.gitignore` gains `/vendor`.
- CI (`ci.yml`) adds:
  - `actions/cache` on `vendor/`, keyed by the hash of `base-models.toml`;
  - `cargo xtask fetch-models` before Test;
  - `VC_REQUIRE_ASSETS=1`, so asset-dependent tests fail instead of skipping in CI.
- `README.md` gains a "Base models" section (`cargo xtask fetch-models`) and an "Offline conversion" section with the `vc-convert` command. `CHANGELOG.md` → Unreleased/Added gets one line for the inference engine and the `vc-convert` CLI.

## Error handling and edge cases
- If the chosen provider fails to start (no DX12 GPU, missing CUDA DLLs), the session is rebuilt on the CPU, the reason is logged and `ActiveProvider::Cpu` is reported. If the CPU fails too, the error is returned.
- An unsupported generator signature is rejected at load with a specific reason. Nothing is run.
- Inputs shorter than one HuBERT frame are padded. Silence (gate closed for the whole block) still runs, so timing stays predictable.
- A sample rate that is unknown and not supplied gives an error naming `--sr`. A rate other than 32/40/48 kHz is rejected.
- The output is checked for NaN or infinite values (they cause an error). Clipping is left as is; gains come in M3.
- Missing assets give one clear message with the fix (`cargo xtask fetch-models`). A hash mismatch in xtask deletes the file and fails.

## Testing
- **Pure unit tests (always run):**
  - the gate: threshold, mask and the absence of discontinuities;
  - mel: shape and a reference value for a 440 Hz sine;
  - RMVPE decoding on a synthetic salience;
  - `shift`/`coarse` (known values, 0 Hz stays 0, values clamp to 1–255);
  - feature ×2 upsampling;
  - block slicing and trimming of context in `offline.rs`;
  - resample lengths;
  - generator signature validation against tiny ONNX graphs built in the test (a valid one, fp16, v1 at 256-dim, no pitch);
  - the provider fallback order, with a fake session builder;
  - xtask hash verification and entry parsing.
- **Asset tests** (run when `vendor/` exists; required in CI through `VC_REQUIRE_ASSETS`):
  - load and warm up HuBERT on the CPU and check that 1 s of audio gives 50×768 frames;
  - RMVPE on a 220 Hz sine gives F0 ≈ 220 Hz ± 2% on voiced frames.
- **End-to-end** (`#[ignore]`, local only, with `VC_TEST_MODEL` and optionally `VC_TEST_MODEL_SR`): convert a generated 3 s voiced test signal, then check the output length (±1 block hop), that it has no NaN or infinite values and that it is not silent. A manual listening check with `vc-convert` on a real voice WAV goes in the PR description.
- The full gate: fmt, clippy `-D warnings`, `cargo test --workspace`, `cargo deny check`.

## Out of scope
- Real-time streaming, the SOLA crossfade, latency/inference stats and wiring into the audio worker (M3).
- UI for the provider or GPU choice, the pitch control or model selection (M3/M4).
- The model library, import and metadata storage (M4). The `.pth` conversion and templates (M5).
- Shipping a CUDA runtime build, the installer and bundling the DLLs (M6).
- Models without F0, fp16 exports, feature-index retrieval (`.index`) and multi-speaker UI (`speaker_id` exists only as a parameter).
