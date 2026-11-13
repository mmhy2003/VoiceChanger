# VC-4 — M2: Inference pipeline — implementation plan

## Goal
`vc-engine` converts mono audio into an RVC v2 voice with ONNX Runtime: resample to 16 kHz → noise gate → ContentVec → RMVPE F0 → pitch shift → RVC v2 generator → resample. It runs on DirectML, CUDA or the CPU, and falls back to the CPU automatically. `vc-convert` converts a WAV file end to end.

## Architecture
- A new dev-only `xtask` crate implements `cargo xtask fetch-models`. It reads `assets/base-models.toml` and downloads into the gitignored `vendor/`:
  - the ContentVec and RMVPE ONNX models;
  - the official DirectML `onnxruntime.dll` and `DirectML.dll`, extracted from their NuGet packages.
  
  Every file is checked against its SHA-256.
- `vc-engine` loads ONNX Runtime with `ort` `load-dynamic`. Its building blocks are `assets`, `runtime`, `dsp::{resample, gate, mel}`, `f0`, `hubert` and `generator`.
- `Pipeline::convert(block, context, params)` is the block call that M3 reuses. `offline::convert_file` slices a WAV into blocks and drives that call. `src/bin/vc-convert.rs` is a thin CLI over it.
- `app`, `audio` and `models` are not touched.

## Facts checked (repo at `c73fa76`, web on 2026-11-13)
**Repo**
- Workspace `members = ["crates/*"]`, edition 2024, resolver 3, `[workspace.lints.clippy] all = "warn"`, and CI runs clippy with `-D warnings`.
- `crates/engine` (`vc-engine`) has no dependencies, and `src/lib.rs` is a single doc comment.
- `crates/audio` already uses `rubato = "0.16"` (the `FftFixedIn` and `process_into_buffer` API) and `thiserror = "2"`. `rustfft 6.4` is already in `Cargo.lock` through rubato.
- `.gitignore` contains only `/target`.
- CI (`.github/workflows/ci.yml`, `windows-latest`): fmt → clippy → test → cargo-deny → release build.
- The `deny.toml` licence allow-list is MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3, ISC, Zlib, Unicode-3.0, BSL-1.0, OFL-1.1 and Ubuntu-font-1.0.
- Commits are conventional (`feat(engine): …`) and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The PR targets `develop`.

**`ort` 2.0.0-rc.15** (newest; it targets ONNX Runtime 1.31; every `api-NN` feature includes the levels below it)
- `ort::init_from(path) -> Result<EnvironmentBuilder, LoadDynamicError>` and `.build() -> Result<Environment>`. Neither sets a global, so keep the `Environment`.
- `Session::builder(&env) -> Result<SessionBuilder>`.
- `.with_execution_providers([..]) -> BuilderResult` (that is, `Result<SessionBuilder, ort::Error<SessionBuilder>>`).
- `.commit_from_file(path)` and `.commit_from_memory(bytes)`.
- Execution providers: `ort::ep::DirectML::default().with_device_id(i32).build().error_on_failure()` and `ort::ep::CUDA::default().with_device_id(i32).build().error_on_failure()`.
- `Session::run(&mut self, ort::inputs!{"name" => tensor, ..})`.
- `ort::value::Tensor::from_array((shape, Vec<T>))` builds an input. On an output, `try_extract_tensor::<f32>() -> Result<(&Shape, &[f32])>` reads it.
- `session.inputs()` and `session.outputs()` return `&[Outlet]`. `outlet.name()` gives the name and `outlet.dtype() -> &ValueType` gives the type: `Tensor { ty: TensorElementType, shape, .. }`, with `tensor_type()` and `tensor_shape()`; a dynamic dimension is `-1`.
- `session.metadata()?.custom("sr") -> Option<String>`.

**Downloads** (Hugging Face API, NuGet)
- ContentVec: `NaruseMioShirakana/MoeSS-SUBModel` at commit `3ff2e37e161ebcb3c4711e7ec870472342260842`, file `vec-768-layer-12.onnx`, 377,655,729 bytes, sha256 `b3886e7dff1495cda514f94f4680a7b1261e05d6929f5c764cdb17934b413c2a`.
  - **Licence GPL-3.0**; the human chose this source.
  - Its input is the 16 kHz waveform `[1,1,N]` and its output is `[1,T,768]`. This is how the RVC WebUI `onnx_inference.py` uses it.
- RMVPE: `lj1995/VoiceConversionWebUI` at commit `e6d0c1a17da07c33557852f9dfa2bd44cc75737d`, file `rmvpe.onnx`, 361,688,443 bytes, sha256 `5370e71ac80af8b4b7c793d27efd51fd8bf962de3a7ede0766dac0befa3660fd`, licence MIT.
  - Input 0 is the log-mel `[1,128,T32]`, where T32 is the frame count padded to a multiple of 32. Output 0 is `[1,T32,360]`.
- NuGet versions: the newest `Microsoft.ML.OnnxRuntime.DirectML` is **1.24.4** and the newest `Microsoft.AI.DirectML` is **1.15.4**. DirectML builds stop at ORT 1.24, so `ort` is built with `default-features = false` and **`api-24`**. Step 4 checks this against the real DLL.

## Deviations from the design (small, with reasons)
- **No direct `ndarray` dependency.** `ort`'s tuple API `(shape, Vec<T>)` and `try_extract_tensor` cover every tensor here. That means one crate fewer for `cargo deny`, and no risk of a version mismatch with `ort`'s ndarray. This follows the precedent of VC-3 dropping an unused `anyhow`.
- **Block alignment.** `Pipeline::convert` needs `context` to be a multiple of 160 samples, and it pads the block to a multiple of 320 samples.
  - ContentVec gives 1 frame per 320 samples and the F0 gives 1 per 160. With this alignment, the ×2 features, the F0 and the gate mask all have exactly `T = len/160` frames.
  - The offline driver uses 10 s blocks and 0.5 s of context, which already satisfy this.
- **50 frames per second from ContentVec.** `ContentEncoder` pads 40 zeros on each side of its input, so 1 s (16,000 samples) gives exactly 50 frames, as the design's test expects. Without the padding the conv stack gives 49.

## File map
| File | Responsibility |
|---|---|
| `Cargo.toml` | `members = ["crates/*", "xtask"]` |
| `.cargo/config.toml` | `[alias] xtask = "run -p xtask --"` |
| `.gitignore` | add `/vendor` |
| `assets/base-models.toml` | pinned URLs, SHA-256, licence and dest for the 2 models and 2 nupkgs (R1) |
| `xtask/Cargo.toml`, `xtask/src/main.rs` | the `fetch-models` command and its dispatch |
| `xtask/src/manifest.rs` | `Manifest`/`Entry` parsing and validation |
| `xtask/src/fetch.rs` | `sha256_file`, `verify_or_delete`, `download`, `extract_from_zip`, `fetch_all` |
| `crates/engine/Cargo.toml` | dependencies (R2) |
| `crates/engine/src/lib.rs` | modules and re-exports |
| `crates/engine/src/error.rs` | `EngineError` |
| `crates/engine/src/assets.rs` | `vendor/` lookup, `VC_ASSETS_DIR`, asset constants |
| `crates/engine/src/testutil.rs` (`#[cfg(test)]`) | `assets_or_skip`, `onnx` mini-writer (R5), `temp_path` |
| `crates/engine/src/runtime.rs` | `environment()`, `Provider`, `ActiveProvider`, `with_fallback`, `open_session` |
| `crates/engine/src/dsp/mod.rs` | `pub mod gate; pub mod mel; pub mod resample;` |
| `crates/engine/src/dsp/resample.rs` | `StreamResampler`, `resample` |
| `crates/engine/src/dsp/gate.rs` | `Gate`, `frame_mask`, `envelope` |
| `crates/engine/src/dsp/mel.rs` | `MelExtractor` (RMVPE front end) |
| `crates/engine/src/f0.rs` | `decode_salience`, `shift`, `coarse`, `apply_mask`, `Rmvpe` |
| `crates/engine/src/hubert.rs` | `ContentEncoder`, `upsample2` |
| `crates/engine/src/generator.rs` | `IoSpec`, `ElemType`, `validate_signature`, `resolve_sr`, `Generator` |
| `crates/engine/src/pipeline.rs` | `PipelineConfig`, `Params`, `Pipeline` (warm-up, `convert`) |
| `crates/engine/src/offline.rs` | `ConvertOptions`, `ConvertReport`, `plan_blocks`, WAV I/O, `convert_file` |
| `crates/engine/src/bin/vc-convert.rs` | the CLI: `parse_args` and `main` |
| `crates/engine/tests/e2e.rs` | `#[ignore]` end-to-end test using `VC_TEST_MODEL` |
| `.github/workflows/ci.yml` | vendor cache, fetch-models, `VC_REQUIRE_ASSETS=1` |
| `deny.toml` | allow-list additions only if needed, each with a reason |
| `README.md`, `CHANGELOG.md` | docs |

---

## Reference

### R1 — `assets/base-models.toml`
```toml
# Base models and ONNX Runtime binaries fetched by `cargo xtask fetch-models`.
# Files land in vendor/ (gitignored). Never commit them. URLs are pinned to
# immutable revisions; every download is checked against its SHA-256.

[[model]]
name = "ContentVec (vec-768-layer-12)"
url = "https://huggingface.co/NaruseMioShirakana/MoeSS-SUBModel/resolve/3ff2e37e161ebcb3c4711e7ec870472342260842/vec-768-layer-12.onnx"
sha256 = "b3886e7dff1495cda514f94f4680a7b1261e05d6929f5c764cdb17934b413c2a"
licence = "GPL-3.0 (NaruseMioShirakana/MoeSS-SUBModel). Fetched for builds only; review before the M6 installer bundles it."
dest = "models/hubert.onnx"

[[model]]
name = "RMVPE"
url = "https://huggingface.co/lj1995/VoiceConversionWebUI/resolve/e6d0c1a17da07c33557852f9dfa2bd44cc75737d/rmvpe.onnx"
sha256 = "5370e71ac80af8b4b7c793d27efd51fd8bf962de3a7ede0766dac0befa3660fd"
licence = "MIT (lj1995/VoiceConversionWebUI)"
dest = "models/rmvpe.onnx"

[[runtime]]
name = "ONNX Runtime DirectML 1.24.4"
url = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime.directml/1.24.4/microsoft.ml.onnxruntime.directml.1.24.4.nupkg"
sha256 = "<computed in step 1>"
licence = "MIT (Microsoft ONNX Runtime)"
extract = "runtimes/win-x64/native/onnxruntime.dll"
dest = "runtime/onnxruntime.dll"

[[runtime]]
name = "DirectML 1.15.4"
url = "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/1.15.4/microsoft.ai.directml.1.15.4.nupkg"
sha256 = "<computed in step 1>"
licence = "Microsoft DirectML licence (redistributable, see LICENSE.txt in the package)"
extract = "bin/x64-win/DirectML.dll"
dest = "runtime/DirectML.dll"
```
Step 1 replaces both `<computed in step 1>` values with real hashes. Nothing is committed with a placeholder.

### R2 — dependencies
`crates/engine/Cargo.toml`:
```toml
[dependencies]
ort = { version = "=2.0.0-rc.15", default-features = false, features = ["load-dynamic", "directml", "cuda", "tracing", "api-24"] }
rubato = "0.16"
rustfft = "6"
hound = "3"
rand = "0.9"
thiserror = "2"
tracing = "0.1"
```
`xtask/Cargo.toml` (`publish = false`, workspace version, edition, licence and lints):
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.9"
sha2 = "0.10"
zip = { version = "2", default-features = false, features = ["deflate"] }
ureq = { version = "3", default-features = false, features = ["native-tls"] }
```
- If `cargo deny check` flags a crate, first try trimming its features. Only if that fails, widen `deny.toml`, adding a `# reason` comment and noting it for the PR description.
- If `toml = "0.9"` does not resolve, use the newest `toml` minor.

### R3 — `EngineError` (`crates/engine/src/error.rs`)
```rust
use std::path::PathBuf;

/// Errors from the inference engine.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("missing {}: run `cargo xtask fetch-models`", path.display())]
    MissingAsset { path: PathBuf },
    #[error("ONNX Runtime: {0}")]
    RuntimeInit(String),
    #[error("unsupported model: {reason}")]
    UnsupportedModel { reason: String },
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("WAV: {0}")]
    Wav(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;
```

### R4 — runtime contract (`crates/engine/src/runtime.rs`)
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider { DirectMl { device: i32 }, Cuda { device: i32 }, Cpu }
impl Default for Provider { fn default() -> Self { Provider::DirectMl { device: 0 } } }
/// Provider a session actually runs on.
pub type ActiveProvider = Provider;
impl std::fmt::Display for Provider { /* "DirectML (GPU 0)", "CUDA (GPU 0)", "CPU" */ }

/// Tries `provider`, then the CPU. Returns the first success and where it runs.
pub fn with_fallback<S>(provider: Provider, mut build: impl FnMut(Provider) -> std::result::Result<S, String>)
    -> std::result::Result<(S, ActiveProvider), String>;
// Order: [provider] if provider == Cpu, else [provider, Cpu]. On a non-CPU failure:
// tracing::warn!(%provider, %reason, "execution provider failed, using CPU").
// If every attempt fails: Err(last reason).

/// Process-wide ONNX Runtime environment, loaded once from vendor/runtime/onnxruntime.dll.
pub fn environment() -> Result<&'static ort::environment::Environment>;
// static ENV: OnceLock<std::result::Result<Environment, String>>.
// Init: if assets DirectML.dll exists, preload it (the `preload-dylibs` helper in ort, see
// docs.rs `ort::util::preload_dylib`), because Windows does not search
// onnxruntime.dll's folder for its dependencies. Then ort::init_from(ort_dll)?.with_name("vc-engine").build().
// Errors -> EngineError::RuntimeInit(reason). Missing DLL -> EngineError::MissingAsset.

pub fn open_session(path: &Path, provider: Provider) -> Result<(ort::session::Session, ActiveProvider)>;
pub fn open_session_from_memory(bytes: &[u8], provider: Provider) -> Result<(Session, ActiveProvider)>; // tests only need Cpu
// Both: with_fallback(provider, |p| build(env, p, source)); final Err(r) -> RuntimeInit(format!("could not open {path}: {r}")).
// build(): Session::builder(env); for DirectMl also .with_memory_pattern(false) and
// .with_parallel_execution(false) if those methods exist in rc.15 (DirectML requires it);
// then .with_execution_providers([ep::DirectML::default().with_device_id(d).build().error_on_failure()])
// (or ep::CUDA…); Cpu registers none. Map every ort error with e.to_string().
```
If `Environment` is not `Sync`, keep it inside `OnceLock` behind a `Mutex` instead. Note any such adjustment in the commit body.

### R5 — test ONNX writer (`crates/engine/src/testutil.rs`, inside `pub mod onnx`)
```rust
//! Minimal ONNX protobuf writer: one Identity node, typed inputs and outputs, metadata.
fn varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 { out.push(b); break; }
        out.push(b | 0x80);
    }
}
fn int_field(field: u64, v: i64, out: &mut Vec<u8>) { varint(field << 3, out); varint(v as u64, out); }
fn bytes_field(field: u64, b: &[u8], out: &mut Vec<u8>) { varint((field << 3) | 2, out); varint(b.len() as u64, out); out.extend_from_slice(b); }

pub const F32: i64 = 1;
pub const I64: i64 = 7;
pub const F16: i64 = 10;
pub enum Dim { Fixed(i64), Sym(&'static str) }
pub struct Io { pub name: &'static str, pub elem: i64, pub dims: Vec<Dim> }

fn value_info(io: &Io) -> Vec<u8> {
    let mut shape = Vec::new();
    for d in &io.dims {
        let mut dim = Vec::new();
        match d { Dim::Fixed(v) => int_field(1, *v, &mut dim), Dim::Sym(s) => bytes_field(2, s.as_bytes(), &mut dim) }
        bytes_field(1, &dim, &mut shape);
    }
    let mut tensor = Vec::new();
    int_field(1, io.elem, &mut tensor);
    bytes_field(2, &shape, &mut tensor);
    let mut ty = Vec::new();
    bytes_field(1, &tensor, &mut ty);
    let mut vi = Vec::new();
    bytes_field(1, io.name.as_bytes(), &mut vi);
    bytes_field(2, &ty, &mut vi);
    vi
}

/// ModelProto (IR 8, opset 17) whose output is `Identity(identity_from)`.
pub fn model(inputs: &[Io], output: &Io, identity_from: &str, metadata: &[(&str, &str)]) -> Vec<u8> {
    let mut node = Vec::new();
    bytes_field(1, identity_from.as_bytes(), &mut node);
    bytes_field(2, output.name.as_bytes(), &mut node);
    bytes_field(4, b"Identity", &mut node);
    let mut graph = Vec::new();
    bytes_field(1, &node, &mut graph);
    bytes_field(2, b"test", &mut graph);
    for i in inputs { bytes_field(11, &value_info(i), &mut graph); }
    bytes_field(12, &value_info(output), &mut graph);
    let mut opset = Vec::new();
    bytes_field(1, b"", &mut opset);
    int_field(2, 17, &mut opset);
    let mut m = Vec::new();
    int_field(1, 8, &mut m);
    bytes_field(7, &graph, &mut m);
    bytes_field(8, &opset, &mut m);
    for (k, v) in metadata {
        let mut e = Vec::new();
        bytes_field(1, k.as_bytes(), &mut e);
        bytes_field(2, v.as_bytes(), &mut e);
        bytes_field(14, &e, &mut m);
    }
    m
}

/// WebUI-shaped generator inputs. `phone_dim` = 768 (v2) or 256 (v1); `pitch` = false drops pitch/pitchf.
pub fn rvc_inputs(phone_dim: i64, float: i64, pitch: bool) -> Vec<Io>;
// phone [1,"T",phone_dim] float; phone_lengths [1] I64; pitch [1,"T"] I64; pitchf [1,"T"] float;
// ds [1] I64; rnd [1,192,"T"] float.
// Output io: audio [1,1,"N"] float, identity_from = "rnd" (shapes are only declared; the graph is loaded, never run).
```
`testutil` also has:
- `pub fn assets_or_skip(rel: &[&str]) -> bool`. It returns true when every asset resolves. Otherwise it panics if `VC_REQUIRE_ASSETS == "1"` (with a message naming `cargo xtask fetch-models`), or prints `skipping: missing …` and returns false.
- `pub fn temp_path(name: &str) -> PathBuf`: `std::env::temp_dir()/vc-engine-test-<pid>-<name>`.

### R6 — signal maths
**Mel** (it matches RVC `rmvpe.py`'s `MelSpectrogram(128, 16000, 1024, 160, fmin 30, fmax 8000)`)
- Window: periodic Hann, `w[n] = 0.5 − 0.5·cos(2πn/1024)`.
- Framing: centred STFT. Pad 512 on each side by reflection, or with zeros when `len ≤ 512`. That gives `frames = len/160 + 1`.
- Per frame: `mag[k] = sqrt(re²+im²)` for k in 0..=512.
- Filterbank: librosa `mel(sr=16000, n_fft=1024, n_mels=128, fmin=30, fmax=8000, htk=True, norm="slaney")`.
  - HTK mel: `m = 2595·log10(1+f/700)`.
  - 130 points are spaced evenly in mel from fmin to fmax and converted to Hz as `hz[i]`.
  - FFT bin frequency: `k·16000/1024`.
  - Weight: `max(0, min((f−hz[i])/(hz[i+1]−hz[i]), (hz[i+2]−f)/(hz[i+2]−hz[i+1])))`, multiplied by `2/(hz[i+2]−hz[i])`.
- Output `log(max(mel·mag, 1e-5))`, laid out `[128][frames]`, row-major as `Vec<f32>` with `n_mels = 128`.

**RMVPE**
- Pad the mel frames with zeros up to a multiple of 32, run the session, and take output 0 trimmed to `[frames, 360]`.

**Salience decode** (per frame, `s[0..360]`)
- `c = argmax`. If `s[c] ≤ 0.03`, the frame is unvoiced: f0 = 0.
- Otherwise, over `j = max(0,c−4) .. min(360,c+5)`, take `cents = Σ s[j]·(20j + 1997.3794084376191) / Σ s[j]`.
- `f0 = 10 · 2^(cents/1200)`.

**Pitch**
- `shift(f0, st)`: clamp `st` to −24..=24, then `f0 · 2^(st/12)`, and 0 stays 0.
- `coarse(f0)`:
  - `mel = 1127·ln(1+f0/700)`, with `mel_min = 1127·ln(1+50/700)` and `mel_max = 1127·ln(1+1100/700)`.
  - If `mel > 0`: `mel = (mel−mel_min)·254/(mel_max−mel_min) + 1`.
  - Clamp to 1..=255, then round to `i64`. So `coarse(0.0) == 1`, `coarse(50.0) == 1`, `coarse(1100.0) == 255`, and `coarse(5000.0) == 255`.

**Gate** (frame = 160 samples)
- RMS dB per frame is `20·log10(rms + 1e-9)`; a frame is open when it is ≥ `threshold_db`.
- Hold: a frame stays open for `hold_frames` (default 5) after the last open frame.
- Gain: a per-sample linear ramp toward 1 or 0 with step `1/ramp` (default ramp 80 samples). The start value is the first frame's state.
- The output is `x·gain`, plus a mask of `ceil(len/160)` frames.

**Frame alignment in `convert`**
- `n` = the block length padded to a multiple of 320; `T = n/160`.
- ContentVec frames are `n/320`; ×2 gives `T`.
- F0 has `T + 1` frames; take the first `T`.
- The mask has `T` frames.

**Generator output**
- `hop = sr/100` samples per frame, so the generator gives `T·hop` samples.
- With `cf = context/160`, the result is `out[cf·hop ..]`, truncated to `core·sr/16000` samples, where `core = original_len − 2·context`. If it is short, pad with zeros.

### R7 — generator validation rules (`validate_signature(inputs: &[IoSpec], outputs: &[IoSpec]) -> Result<()>`)
`IoSpec { name: String, elem: ElemType, dims: Vec<i64> }` and `ElemType { F32, F16, I64, Other }`. The rules are checked in this order, and the first failure returns `UnsupportedModel { reason }`:
1. Any input or output is `F16`: `"fp16 export"`.
2. No `pitch` or no `pitchf` input: `"no F0 inputs (models without pitch guidance are not supported)"`.
3. No `phone` input: `"not an RVC WebUI ONNX export (missing input `phone`)"`.
4. The last dimension of `phone` is 256: `"RVC v1 (256-dim)"`. Any other value except 768 or −1: `"phone has {d} features, expected 768"`.
5. Each required I/O must have the right type and rank:
   - `phone` F32/3, `phone_lengths` I64/1, `pitch` I64/2, `pitchf` F32/2, `ds` I64/1, `rnd` F32/3;
   - output `audio` F32/3.
   
   Missing: `"missing input `{n}`"` (or `"missing output `audio`"`). Wrong: `"`{n}` is {elem:?} rank {r}, expected {elem:?} rank {r}"`.

`resolve_sr(metadata: Option<&str>, override_sr: Option<u32>) -> Result<u32>`:
- The override wins; otherwise the metadata is parsed as `u32`.
- If neither is available: `"unknown sample rate: pass --sr"`.
- A value outside {32000, 40000, 48000}: `"unsupported sample rate {n} (expected 32000, 40000 or 48000)"`.
- Unparsable metadata is treated as absent.

### R8 — engine public API (`lib.rs` re-exports)
```rust
pub mod assets; pub mod dsp; pub mod error; pub mod f0; pub mod generator; pub mod hubert;
pub mod offline; pub mod pipeline; pub mod runtime;
#[cfg(test)] mod testutil;
pub use error::{EngineError, Result};
pub use offline::{ConvertOptions, ConvertReport, convert_file};
pub use pipeline::{Params, Pipeline, PipelineConfig};
pub use runtime::{ActiveProvider, Provider};
```
Add each `pub mod` in the step that creates its module, so every step compiles.

---

## Steps

### 1. Pin the downloads: xtask crate skeleton, manifest parsing and `assets/base-models.toml`
- **Workspace:**
  - In `Cargo.toml`, set `members = ["crates/*", "xtask"]`.
  - Create `.cargo/config.toml` with `[alias]` and `xtask = "run -p xtask --"`.
  - Add `/vendor` to `.gitignore`.
- **Crate:** create `xtask/Cargo.toml` with `name = "xtask"`, `publish = false`, the workspace version, edition, licence, repository and authors, `[lints] workspace = true`, and the R2 deps (only `serde` and `toml` are needed in this step; add `sha2`, `zip` and `ureq` in step 2).
- **Nupkg hashes:** download the two nupkgs into a new empty temp folder with PowerShell. Get each hash with `Get-FileHash -Algorithm SHA256 <file>`. Open each package as a zip (`Expand-Archive` into its own empty folder) to confirm the `extract` paths in R1 exist. Then write `assets/base-models.toml` exactly as in R1, with the real lowercase hashes.
- **TDD:** `xtask/src/manifest.rs` defines:
  ```rust
  #[derive(Debug, Deserialize)] pub struct Manifest { #[serde(default)] pub model: Vec<Entry>, #[serde(default)] pub runtime: Vec<Entry> }
  #[derive(Debug, Deserialize)] pub struct Entry { pub name: String, pub url: String, pub sha256: String, pub licence: String, pub dest: String, #[serde(default)] pub extract: Option<String> }
  pub fn parse(text: &str) -> Result<Manifest, String> // toml + validate
  ```
  Validation rejects:
  - `url` not starting with `https://`;
  - `sha256` that is not 64 lowercase hex characters;
  - a `dest` that is empty, absolute, or contains `..`;
  - a `runtime` entry without `extract`.
  
  Write these tests first:
  - `parses_models_and_runtimes`, on an inline 1 model + 1 runtime TOML;
  - `rejects_http_url`;
  - `rejects_bad_sha`;
  - `rejects_parent_dest`;
  - `runtime_needs_extract`;
  - `repo_manifest_is_valid`, which parses `include_str!("../../assets/base-models.toml")` and expects 2 models and 2 runtimes.
  
  `main.rs` has `mod manifest; fn main()`. The `fetch-models` subcommand is stubbed with `eprintln!("not implemented")` + exit 2; any other argument prints usage and exits 2.
- **Run:** `cargo test -p xtask`. It fails first (module missing), then passes.
- **Gate:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
- **Commit:** `build(xtask): add xtask crate and pinned base-models manifest`.

### 2. Implement `cargo xtask fetch-models` (download, verify, extract)
Add `sha2`, `zip` and `ureq` (R2). Create `xtask/src/fetch.rs`:
```rust
pub fn sha256_file(path: &Path) -> io::Result<String>        // lowercase hex
pub fn verify_or_delete(path: &Path, want: &str) -> Result<bool, String> // Ok(false) if missing; Ok(true) if matches; mismatch => delete file, Err("hash mismatch for … (expected …, got …); file deleted")
pub fn extract_from_zip(zip: &Path, inner: &str, dest: &Path) -> Result<(), String>
pub fn download(agent: &ureq::Agent, url: &str, dest: &Path) -> Result<(), String> // stream to dest + ".part", rename on success
pub fn fetch_all(root: &Path, manifest: &Manifest) -> Result<(), String>
```
- **Agent:** `ureq::Agent::config_builder().tls_config(ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build()).build().into()`. Read with `resp.into_body().into_reader()` (no size limit) and `io::copy`.
- **Models:**
  - `vendor/<dest>`: if `verify_or_delete` gives true, print `ok  <name>`.
  - If false: download it, print `get <name>`, and verify it. A mismatch fails, and the file has already been deleted.
- **Runtimes:**
  - the nupkg goes to `vendor/cache/<file name of url>` with the same verify/download rule;
  - then always `extract_from_zip(cache, extract, vendor/<dest>)`, so DLLs come only from a verified package.
- `create_dir_all` the parents. `root` = `Path::new(env!("CARGO_MANIFEST_DIR")).parent()`. `main` reads `root/assets/base-models.toml`. On error it prints the message and exits 1.

**TDD** (in `fetch.rs`, using temp files in `std::env::temp_dir()` with unique names):
- `sha256_of_abc`: the file contents `abc` give `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`;
- `verify_missing_is_false`;
- `verify_match_is_true`;
- `verify_mismatch_deletes`: Err, and the file is gone;
- `extracts_inner_file`: build a zip with `zip::ZipWriter` and `SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)` holding `a/b.dll` = `hi`, extract it, and read back `hi`;
- `extract_missing_inner_errors`.

**Run** `cargo test -p xtask`: it fails, then passes. Then run `cargo xtask fetch-models` for real: about 750 MB, and all 4 files should end up under `vendor/`. A second run prints only `ok` lines.

Run `cargo deny check`. If it fails, see R2.

Commit: `feat(xtask): fetch and verify base models and ONNX Runtime DLLs`.

### 3. Engine foundations: `EngineError`, asset lookup and test helpers
- Add `thiserror` and `tracing` to `crates/engine/Cargo.toml`.
- Create `error.rs` (R3) and `assets.rs`:
  ```rust
  pub const HUBERT: &str = "models/hubert.onnx";
  pub const RMVPE: &str = "models/rmvpe.onnx";
  pub const ORT_DLL: &str = "runtime/onnxruntime.dll";
  pub const DML_DLL: &str = "runtime/DirectML.dll";
  pub fn candidates() -> Vec<PathBuf> // VC_ASSETS_DIR set → [it]; else [<exe dir>/vendor, <CARGO_MANIFEST_DIR>/../../vendor]
  pub fn resolve(rel: &str, dirs: &[PathBuf]) -> Result<PathBuf> // first dir where dir.join(rel).is_file(); else MissingAsset { path: first dir join rel, or rel itself if dirs is empty }
  pub fn asset(rel: &str) -> Result<PathBuf> { resolve(rel, &candidates()) }
  ```
- Create `testutil.rs` with `assets_or_skip` and `temp_path` from R5. The `onnx` module comes in step 10.
- `lib.rs` has a doc comment (`//! RVC v2 inference pipeline: ONNX Runtime sessions, DSP, F0 and offline conversion.`), `pub mod assets; pub mod error; #[cfg(test)] mod testutil;` and `pub use error::{EngineError, Result};`.

TDD in `assets.rs` (a temp directory per test):
- `resolve_prefers_first_dir`: two dirs both hold the file, and the first is returned;
- `resolve_falls_through`: only the second has it;
- `missing_names_file_and_fix`: `err.to_string()` contains `models/hubert.onnx` and `cargo xtask fetch-models`.

Do not set environment variables in tests: they are process-global. That is why `resolve` takes `dirs`.

Run: `cargo test -p vc-engine` fails, then passes. Then the gate. Commit: `feat(engine): add EngineError and vendor asset lookup`.

### 4. ONNX Runtime loading, providers and CPU fallback
Add `ort` (R2) and implement `runtime.rs` per R4. Add `pub mod runtime;` and `pub use runtime::{ActiveProvider, Provider};`.

TDD for the pure parts:
- `cpu_requested_tries_once`: the closure records the providers it receives, and the result is `[Cpu]` with active `Cpu`;
- `dml_success_stays_on_dml`;
- `dml_failure_falls_back_to_cpu`: the attempts are `[DirectMl{0}, Cpu]` and active is `Cpu`;
- `cuda_failure_falls_back_to_cpu`;
- `all_fail_returns_last_reason`;
- `display_names`: `"DirectML (GPU 0)"`, `"CUDA (GPU 1)"`, `"CPU"`.

Asset test `runtime_loads_and_opens_rmvpe_on_cpu`:
- `if !assets_or_skip(&[ORT_DLL, RMVPE]) { return }`;
- `open_session(&asset(RMVPE)?, Provider::Cpu)` gives `Ok` with active `Cpu`;
- the session's first input has 3 dimensions.

Run `cargo test -p vc-engine runtime`. It fails, then passes with `vendor/` present.

**api-24 check:** if `init_from` or the session fails because the DLL's API level is too low, lower the feature to the highest `api-NN` that works with ORT 1.24.4, and write that in the commit body. If a DirectML-capable GPU is present, also run a local ignored test `dml_smoke` (`#[ignore]`) that opens RMVPE on `DirectMl{0}` and prints the active provider.

Gate, `cargo deny check`. Commit: `feat(engine): load ONNX Runtime dynamically with DirectML/CUDA and CPU fallback`.

### 5. Resampling (`dsp/resample.rs`)
- Add `rubato = "0.16"`. Create `dsp/mod.rs` with `pub mod resample;` and add `pub mod dsp;` to `lib.rs`.
- `StreamResampler`:
  - `new(from: u32, to: u32) -> Result<Self>` wraps `rubato::FftFixedIn::<f32>::new(from, to, 1024, 2, 1)`, mapping errors to `Inference`;
  - `push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()>` buffers input and processes each full `input_frames_next()` chunk with `process_into_buffer`;
  - `flush(&mut self, out: &mut Vec<f32>) -> Result<()>` processes the rest with `process_partial`;
  - `output_delay(&self) -> usize`.
- `resample(input: &[f32], from: u32, to: u32) -> Result<Vec<f32>>`:
  - `from == to` returns a copy;
  - otherwise `expected = (len·to + from/2)/from`. Push, flush, and keep pushing zero chunks until there are at least `delay + expected` samples. Drop `delay` samples and truncate to `expected`.

TDD:
- `same_rate_is_copy`;
- `lengths_48k_to_16k`: 48,000 samples in, 16,000 out;
- `lengths_16k_to_40k`: 16,000 in, 40,000 out;
- `lengths_44k1_to_16k`: 44,100 in, 16,000 out;
- `odd_length`: 1,001 samples at 16 kHz → 48 kHz gives 3,003;
- `sine_amplitude_kept`: a 0.5-amplitude 440 Hz sine at 48 kHz → 16 kHz has peak 0.5 ± 0.02 in the middle half;
- `stream_matches_oneshot_length`.

Run `cargo test -p vc-engine resample`: it fails, then passes. Gate. Commit: `feat(engine): add rubato resamplers`.

### 6. Noise gate (`dsp/gate.rs`)
Add `pub mod gate;` to `dsp/mod.rs`.
```rust
pub const FRAME: usize = 160; // 10 ms at 16 kHz
#[derive(Clone, Copy, Debug)] pub struct Gate { pub threshold_db: f32, pub hold_frames: usize, pub ramp: usize }
impl Default for Gate { -50.0, 5, 80 }
pub fn frame_mask(x: &[f32], threshold_db: f32, hold_frames: usize) -> Vec<bool> // ceil(len/160) frames, hold applied
pub fn envelope(mask: &[bool], len: usize, ramp: usize) -> Vec<f32>              // per-sample gain, R6
impl Gate { pub fn apply(&self, x: &[f32]) -> (Vec<f32>, Vec<bool>) }
```
TDD:
- `loud_frames_open_quiet_closed`: 10 frames at 0.1 amplitude (−20 dB), then 10 silent frames, with hold 0, gives a mask of 10× true and 10× false;
- `hold_extends_open`: with hold 3, frames 10–12 are open and 13 is closed;
- `threshold_respected`: a −60 dB signal is closed at −50 and open at −70;
- `envelope_has_no_jumps`: for an alternating mask, the max |gain[i+1]−gain[i]| ≤ 1/80 + 1e-6, and every gain is in 0..=1;
- `silent_input_is_zero`;
- `partial_last_frame_counted`: 170 samples give 2 frames.

Run `cargo test -p vc-engine gate`: it fails, then passes. Gate. Commit: `feat(engine): add noise gate with hold and ramped gain`.

### 7. RMVPE log-mel front end (`dsp/mel.rs`)
- Add `rustfft = "6"` and `pub mod mel;`.
- `MelExtractor::new()` precomputes the Hann window, the `FftPlanner` forward plan for 1024 and the filterbank `[128][513]` (R6).
- `pub fn log_mel(&mut self, audio: &[f32]) -> (Vec<f32>, usize)` returns a row-major `[128][frames]` and `frames`.
- Constants: `N_FFT = 1024`, `HOP = 160`, `N_MELS = 128`, `FMIN = 30.0`, `FMAX = 8000.0`.

TDD:
- `frame_count`: 16,000 samples give 101 frames, and 1,600 give 11;
- `silence_is_log_floor`: every value is `ln(1e-5)` (−11.512925) ± 1e-4;
- `filterbank_is_slaney_triangles`: every row has a non-zero sum, and each row's peak bin frequency rises with the row index;
- `sine_440_peaks_in_right_band`: in a 0.5-amplitude 440 Hz sine, the argmax mel row of the middle frame is a band whose `[hz[i], hz[i+2]]` contains 440, and its value is > 0;
- `short_input_works`: 100 samples give 1 frame.

Run `cargo test -p vc-engine mel`: it fails, then passes. Gate. Commit: `feat(engine): add RMVPE log-mel spectrogram`.

### 8. F0: salience decode, pitch shift, coarse pitch and the `Rmvpe` session (`f0.rs`)
`pub mod f0;`
```rust
pub const THRESHOLD: f32 = 0.03;
pub fn decode_salience(sal: &[f32], frames: usize, threshold: f32) -> Vec<f32> // sal row-major [frames][360], R6
pub fn shift(f0: &mut [f32], semitones: f32)
pub fn coarse(f0: f32) -> i64
pub fn apply_mask(f0: &mut [f32], mask: &[bool]) // closed => 0; frames beyond mask untouched
pub struct Rmvpe { session: Session, mel: MelExtractor, active: ActiveProvider }
impl Rmvpe {
  pub fn load(path: &Path, provider: Provider) -> Result<Self>
  pub fn active_provider(&self) -> ActiveProvider
  pub fn f0(&mut self, audio_16k: &[f32]) -> Result<Vec<f32>> // len/160+1 values in Hz
}
```
`Rmvpe::f0`:
- compute the log-mel, pad the frames to a multiple of 32 with zeros, and build `Tensor::from_array(([1, 128, t32], data))`;
- run with `ort::inputs![tensor]` (positional) and take output 0 with `try_extract_tensor::<f32>()`;
- keep the first `frames·360` values and decode them;
- map ort errors to `Inference`.

TDD (pure):
- `decode_peak_at_bin_100`: a one-hot at bin 100 gives `10·2^((2000+1997.379)/1200)` ± 0.01;
- `decode_below_threshold_is_zero`;
- `decode_weighted_between_bins`: equal 0.5 at bins 100 and 101 gives cents at bin 100.5;
- `shift_octave_doubles`, `shift_zero_stays_zero` and `shift_clamped_to_24`;
- `coarse_known_values`: 0→1, 50→1, 1100→255, 5000→255, and 220 → the value from R6, computed in the test;
- `mask_forces_unvoiced`.

Asset test `rmvpe_220hz_sine`:
- `assets_or_skip(&[ORT_DLL, RMVPE])`;
- load on `Cpu` and feed 1 s of a 0.3-amplitude 220 Hz sine;
- at least 80% of the frames in 10..90 are voiced, and their median is within 2% of 220.

Run `cargo test -p vc-engine f0`: it fails, then passes. Gate. Commit: `feat(engine): add RMVPE F0 extraction, pitch shift and coarse pitch`.

### 9. ContentVec feature encoder (`hubert.rs`)
`pub mod hubert;`
```rust
pub const DIM: usize = 768;
pub fn upsample2(feats: &[f32], frames: usize) -> Vec<f32> // each 768-row repeated twice → 2*frames rows
pub struct ContentEncoder { session: Session, rank: usize, active: ActiveProvider }
impl ContentEncoder {
  pub fn load(path: &Path, provider: Provider) -> Result<Self> // rank = session.inputs()[0] tensor shape len (2 or 3; else UnsupportedModel)
  pub fn active_provider(&self) -> ActiveProvider
  pub fn encode(&mut self, audio_16k: &[f32]) -> Result<(Vec<f32>, usize)> // row-major [T][768], T
}
```
`encode`:
- pad 40 zeros on each side, and pad further with zeros to at least 400 samples, so short inputs are handled;
- the shape is `[1,1,n]` for rank 3 and `[1,n]` for rank 2;
- run positionally and take output 0. Its last dimension must be 768, otherwise `UnsupportedModel { reason: "content encoder gives {d} features, expected 768" }`. `T = data.len()/768`.

TDD:
- `upsample2_repeats_rows`: 2 rows give 4, in the order r0, r0, r1, r1;
- `upsample2_empty`.

Asset tests (`assets_or_skip(&[ORT_DLL, HUBERT])`, on the CPU):
- `hubert_one_second_is_50_frames`: 16,000 samples of a 0.1-amplitude sine give T = 50 and len = 50·768, all finite;
- `hubert_short_input_padded`: 100 samples give T ≥ 1.

Run `cargo test -p vc-engine hubert`: it fails, then passes. Gate. Commit: `feat(engine): add ContentVec encoder with 100 Hz upsampling`.

### 10. RVC v2 generator: signature validation, sample rate and inference (`generator.rs`)
`pub mod generator;` Add `rand = "0.9"`, and add `pub mod onnx` (R5) to `testutil.rs`.
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum ElemType { F32, F16, I64, Other }
#[derive(Clone, Debug)] pub struct IoSpec { pub name: String, pub elem: ElemType, pub dims: Vec<i64> }
pub fn validate_signature(inputs: &[IoSpec], outputs: &[IoSpec]) -> Result<()> // R7
pub fn resolve_sr(metadata: Option<&str>, override_sr: Option<u32>) -> Result<u32> // R7
pub struct GenInput<'a> { pub phone: &'a [f32], pub frames: usize, pub pitch: &'a [i64], pub pitchf: &'a [f32], pub speaker_id: i64 }
pub struct Generator { session: Session, sr: u32, rng: rand::rngs::StdRng, active: ActiveProvider }
impl Generator {
  pub fn load(path: &Path, provider: Provider, sr_override: Option<u32>) -> Result<Self>
  pub fn load_from_memory(bytes: &[u8], provider: Provider, sr_override: Option<u32>) -> Result<Self>
  pub fn sample_rate(&self) -> u32
  pub fn active_provider(&self) -> ActiveProvider
  pub fn run(&mut self, input: &GenInput) -> Result<Vec<f32>> // [1,1,N] flattened; NaN/inf => Inference("model produced non-finite audio")
}
```
`load`:
- open the session, convert `inputs()`/`outputs()` to `IoSpec` (via `dtype().tensor_type()` and `tensor_shape()`), and validate;
- then `resolve_sr(session.metadata()?.custom("sr").as_deref(), sr_override)`;
- `rng = StdRng::seed_from_u64(0)`.

`run`:
- feed `phone [1,T,768]`, `phone_lengths [T]`, `pitch [1,T]`, `pitchf [1,T]`, `ds [speaker_id]` and `rnd [1,192,T]`;
- `rnd` is standard normal by Box–Muller from `rng.random::<f32>()`, with `u1` clamped to ≥ 1e-7;
- read the `"audio"` output.

TDD (pure):
- `valid_signature_ok`, built from `IoSpec` lists;
- `fp16_rejected`;
- `v1_rejected`;
- `no_pitch_rejected`;
- `missing_rnd_rejected`;
- `wrong_rank_rejected`;
- `sr_override_wins`;
- `sr_from_metadata`;
- `sr_missing_names_flag`: the message contains `--sr`;
- `sr_44100_rejected`;
- `box_muller_mean_and_var`: over 10k samples, the mean is within ±0.05 and the variance is 1 ± 0.05.

Asset tests (`assets_or_skip(&[ORT_DLL])`; the graphs come from `testutil::onnx::model(&rvc_inputs(..), ..)` and are loaded with `load_from_memory(.., Provider::Cpu, ..)`):
- `tiny_valid_loads` with metadata `sr = 40000` gives `sample_rate() == 40000`;
- `tiny_fp16_rejected`;
- `tiny_v1_rejected`;
- `tiny_no_pitch_rejected`;
- `tiny_without_sr_needs_flag`.

All rejections are matched as `EngineError::UnsupportedModel`.

Run `cargo test -p vc-engine generator`: it fails, then passes. Gate. Commit: `feat(engine): add RVC v2 generator with WebUI signature validation`.

### 11. Pipeline with warm-up and the block `convert` call (`pipeline.rs`)
`pub mod pipeline;` and `pub use pipeline::{Params, Pipeline, PipelineConfig};`.
```rust
#[derive(Clone, Debug)] pub struct PipelineConfig { pub model: PathBuf, pub sr_override: Option<u32>, pub provider: Provider, pub warmup_len: usize, pub warmup_context: usize }
#[derive(Clone, Copy, Debug)] pub struct Params { pub pitch_semitones: f32, pub gate_db: f32, pub speaker_id: i64 }
impl Default for Params { 0.0, -50.0, 0 }
pub struct Pipeline { hubert: ContentEncoder, rmvpe: Rmvpe, generator: Generator, warmup: Duration }
impl Pipeline {
  pub fn new(cfg: &PipelineConfig) -> Result<Self> // assets::asset(HUBERT/RMVPE), load 3 sessions, then time convert(&vec![0.0; warmup_len], warmup_context, &Params::default())
  pub fn sample_rate(&self) -> u32
  pub fn active_provider(&self) -> ActiveProvider // the generator's; hubert/rmvpe fallbacks were already logged
  pub fn warmup_time(&self) -> Duration
  pub fn convert(&mut self, block_16k: &[f32], context: usize, params: &Params) -> Result<Vec<f32>>
}
pub fn frames_for(len: usize) -> usize           // padded_len(len)/160
pub fn padded_len(len: usize) -> usize           // round up to a multiple of 320, min 320
pub fn trim_output(out: &[f32], context: usize, core: usize, sr: u32) -> Vec<f32> // R6
```
`convert`:
1. Return `Inference` if `context % 160 != 0` or `block.len() <= 2*context`.
2. Pad to `padded_len`, then run the gate (`Gate { threshold_db: params.gate_db, ..Default }`).
3. `hubert.encode` → `upsample2`, then truncate or pad (repeat the last row) to T rows.
4. `rmvpe.f0`, truncate to T, `apply_mask`, `shift`, then `coarse` per frame.
5. `generator.run`, then `trim_output`.

A silent block still runs every session, so timing stays predictable.

TDD (pure):
- `padded_len_rounds_up`: 1 → 320, 320 → 320, 321 → 640;
- `frames_for_matches`: 168,000 → 1,050;
- `trim_output_drops_context`: sr 40,000, context 8,000, core 160,000, and `out` of length 1,050·400 with values = index, gives the first value `50·400` and length 400,000;
- `trim_output_pads_short`;
- `trim_output_44k_style_rounding`: sr 40,000 with an odd core gives the length `core·40000/16000`;
- `convert_rejects_misaligned_context`: test it through a free fn `check_block(len, context) -> Result<()>` that `convert` calls first.

Run `cargo test -p vc-engine pipeline`: it fails, then passes. Gate. Commit: `feat(engine): add block pipeline with session warm-up`.

### 12. Offline WAV conversion (`offline.rs`)
`pub mod offline;` and the re-exports from R8. Add `hound = "3"`.
```rust
#[derive(Clone, Debug)] pub struct ConvertOptions { pub model: PathBuf, pub sr_override: Option<u32>, pub provider: Provider, pub params: Params, pub out_rate: Option<u32>, pub block_secs: f32, pub context_secs: f32 }
// ConvertOptions::new(model) sets: sr_override None, provider default, params default, out_rate None, block 10.0, context 0.5
#[derive(Clone, Debug)] pub struct ConvertReport { pub active_provider: ActiveProvider, pub warmup: Duration, pub audio_secs: f64, pub convert_secs: f64 }
impl ConvertReport { pub fn real_time_factor(&self) -> f64 { convert_secs / audio_secs } }
pub fn align(samples: usize, step: usize) -> usize                // round to nearest multiple of step, min step
pub fn plan_blocks(total: usize, block: usize, context: usize) -> Vec<Range<usize>> // ranges in the padded signal
pub fn read_wav_mono(path: &Path) -> Result<(Vec<f32>, u32)>
pub fn write_wav_f32(path: &Path, samples: &[f32], rate: u32) -> Result<()>
pub fn convert_file(input: &Path, output: &Path, opts: &ConvertOptions) -> Result<ConvertReport>
```
`plan_blocks`:
- `n = max(1, ceil(total/block))`;
- range `i` is `i·block .. i·block + block + 2·context`;
- the padded signal is `context` zeros, then the signal, then zeros up to the length `n·block + 2·context`.

`read_wav_mono`:
- reads float 32, or int 8/16/24/32 scaled by `1/2^(bits-1)`;
- averages the channels;
- maps hound errors to `Wav`.

`convert_file`:
1. Read the WAV and resample it to 16 kHz.
2. `block = align(block_secs·16000, 320)` and `context = align(context_secs·16000, 320)`.
3. `Pipeline::new` with `warmup_len = block + 2·context`.
4. Time the loop over `plan_blocks`, concatenate the outputs and truncate them to `total·sr/16000`.
5. Resample to `out_rate.unwrap_or(input_rate)` and write it.
6. Return the report, with `audio_secs` = the input duration.

TDD:
- `plan_one_short_block`: total 100, block 320, context 160 gives `[0..640]`;
- `plan_exact_multiple`: total 640, block 320 gives 2 ranges;
- `plan_covers_all`: the last range end equals the padded length;
- `align_rounds`;
- `wav_roundtrip_f32`;
- `wav_int16_stereo_downmix`: write L = 0.5 and R = −0.5 as i16, and read back ≈ 0;
- `wav_missing_file_is_wav_error`.

Run `cargo test -p vc-engine offline`: it fails, then passes. Gate. Commit: `feat(engine): add offline WAV conversion in blocks`.

### 13. `vc-convert` CLI and the local end-to-end test
Create `crates/engine/src/bin/vc-convert.rs`. Cargo auto-discovers it; no `[[bin]]` entry is needed.
```rust
fn parse_args(args: &[String]) -> Result<(PathBuf, PathBuf, ConvertOptions), String>
```
- Arguments: `<in.wav> <out.wav> --model m.onnx [--sr N] [--pitch N] [--gate-db N] [--provider dml|cuda|cpu] [--device N] [--out-rate N]`. `--device` applies to dml/cuda and defaults to 0.
- Hand-parse; no clap.
- An unknown flag, a missing value, a missing `--model` or a bad provider returns `Err(usage or reason)`.

`main`:
- install nothing for tracing; print errors to stderr and exit 1;
- on success, print `provider: {active}`, `warm-up: {ms} ms` and `real-time factor: {rtf:.2}`, and exit 0.

Unit tests in the bin file:
- `parses_minimal`;
- `parses_all_flags`: `--provider cuda --device 1` gives `Cuda { device: 1 }`, and `--pitch -12` gives −12.0;
- `default_provider_is_dml0`;
- `rejects_missing_model`;
- `rejects_bad_provider`;
- `rejects_unknown_flag`.

`crates/engine/tests/e2e.rs`, test `#[ignore] converts_voiced_signal`:
- `VC_TEST_MODEL` is required; if it is unset, panic with a message saying to set it. `VC_TEST_MODEL_SR` is optional and is parsed into `sr_override`.
- Generate 3 s at 16 kHz: a 150 Hz pulse train through a one-pole low-pass at 0.3 amplitude, with ±5% vibrato. Write it with `write_wav_f32` to a temp file and run `convert_file` with `Provider::Cpu`.
- Read the output back and check:
  - the length is 48,000 ± 320;
  - every sample is finite;
  - the RMS is > 1e-3.

Run:
- `cargo test -p vc-engine --bin vc-convert` (it fails, then passes);
- `cargo test -p vc-engine --test e2e` (0 run, 1 ignored).

If you have a local model: `$env:VC_TEST_MODEL='C:\path\model.onnx'; $env:VC_TEST_MODEL_SR='40000'; cargo test -p vc-engine --test e2e -- --ignored`. Also try `cargo run -p vc-engine --bin vc-convert --release -- in.wav out.wav --model m.onnx --sr 40000` and listen.

Gate. Commit: `feat(engine): add vc-convert CLI and end-to-end test`.

### 14. CI, docs and the final gate
Edit `.github/workflows/ci.yml`, job `checks`, after `Swatinem/rust-cache@v2`:
```yaml
      - name: Cache base models
        uses: actions/cache@v4
        with:
          path: vendor
          key: vendor-${{ hashFiles('assets/base-models.toml') }}
      - name: Fetch base models
        run: cargo xtask fetch-models
```
Then give the `Test` step `env: { VC_REQUIRE_ASSETS: "1" }` (block style).

`README.md`:
- Update `## Status`: M0 and M1 are done, and M2 (the inference engine with offline conversion) is in progress.
- Add `## Base models` after `## Building`:
  - `cargo xtask fetch-models` downloads ContentVec, RMVPE and the DirectML ONNX Runtime DLLs (about 750 MB) into `vendor/`, checked against `assets/base-models.toml`, and it is needed before `cargo test`;
  - ContentVec is GPL-3.0 and is not committed;
  - `VC_ASSETS_DIR` overrides the location.
- Add `## Offline conversion`:
  - the `vc-convert` command from step 13;
  - `--sr` is needed when the model has no `sr` metadata;
  - only RVC WebUI ONNX exports with F0 (fp32) are supported;
  - `--provider cuda` needs a CUDA build of `onnxruntime.dll` in place of the DirectML one.

`CHANGELOG.md`, under `## [Unreleased]` → `### Added`: `- Inference engine: ONNX Runtime (DirectML, CUDA or CPU with automatic fallback) running ContentVec, RMVPE and RVC v2 WebUI ONNX exports, \`cargo xtask fetch-models\` for pinned base models, and the \`vc-convert\` CLI for offline WAV conversion.`

Full gate, run locally after `cargo xtask fetch-models`:
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `$env:VC_REQUIRE_ASSETS='1'; cargo test --workspace`
- `cargo deny check`
- `cargo build --release`

Commit: `ci: fetch base models and require asset tests; docs for M2`.

---

## Final verification
1. `cargo xtask fetch-models` downloads, or reports `ok` for, all 4 files. Corrupt one byte of `vendor/models/rmvpe.onnx` and run it again: it reports a mismatch, deletes the file and exits 1. Run it once more and it restores the file.
2. `$env:VC_REQUIRE_ASSETS='1'; cargo test --workspace` passes. That includes the real HuBERT 50×768 test, the RMVPE 220 Hz test, the tiny-generator validation tests and the runtime CPU test.
3. Clippy `-D warnings`, fmt, `cargo deny check` and `cargo build --release` are clean.
4. With a real RVC v2 WebUI ONNX model:
   - `$env:VC_TEST_MODEL=…; cargo test -p vc-engine --test e2e -- --ignored` passes;
   - `vc-convert voice.wav out.wav --model m.onnx --sr 40000` prints `provider: DirectML (GPU 0)` on a DX12 GPU, or `CPU` after the fallback warning, plus the warm-up and RTF, and `out.wav` sounds like the target voice. Note the result in the PR description.
   - `--provider cuda` with the DirectML DLL falls back to the CPU with a warning, and does not crash.
5. An fp16 or no-F0 export gives `unsupported model: fp16 export` or `… no F0 inputs …`. A model with no `sr` metadata and no `--sr` gives `… pass --sr`.
6. The PR (into `develop`) describes:
   - the ContentVec GPL-3.0 note;
   - any `deny.toml` additions with their reasons;
   - the final `api-NN` level;
   - the listening check.