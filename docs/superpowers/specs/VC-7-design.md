# VC-7 — M5: Convert tab (PyTorch `.pth` → ONNX without Python)

## Goal
A user with only a PyTorch RVC v2 voice model (a `.pth` file, or a `.zip` holding one) opens the **Convert** tab, sees what the file contains, sets a name, default pitch and icon, then presses **Convert**. The app writes an ONNX model, checks it, and adds it to the model library, where it is ready to use. Nothing from the file is ever executed, and the app needs no Python. Converting while the voice changer runs causes no audio dropouts.

## Decisions
- **Templates are exported by the implementing agent** (your answer). The agent builds a throwaway Python venv outside the repo, with CPU `torch`, `onnx` and RVC-Project pinned to one commit. It runs `tools/export-templates/` and commits the 3 templates plus a manifest. Python is never needed at runtime or in CI.
- **Public-model test set: a pinned manifest, run locally only** (your answer). `assets/test-models.toml` lists about 20 public RVC v2 F0 models, mixing 32k, 40k and 48k, each pinned to a Hugging Face revision with its SHA-256. The agent picks them. `cargo xtask fetch-test-models` downloads them into `vendor/test-models/`. An `#[ignore]` test converts them all and reports the pass rate, which must be ≥ 95%. CI does not download them.
- **Fuzzing: stable mutation tests plus a cargo-fuzz job on Linux** (your answer). Every `cargo test` runs seeded random-mutation tests on Windows. A separate `ubuntu-latest` nightly job runs cargo-fuzz for about 5 minutes per PR.
- **PRD §17: no Python fallback and no `.safetensors`** (your answer). Models the templates can't cover get "This model's settings aren't supported yet.", with the config values in the log. New templates or name mappings come later.
- **Convert tab layout: the approved mockup** (5 states: no file, file read, converting, done, problem).
- **Our own restricted unpickler, not `candle-core`.** RVC files need only a small part of pickle protocol 2. A reader of about 400 lines with an explicit allow-list is easier to audit and fuzz than a large ML crate with its own semantics.
- **ONNX types come from `prost`, with the generated code committed.** `cargo xtask gen-onnx` runs `prost-build` with `protox` (pure Rust, so no `protoc`) on a vendored `onnx.proto` (Apache-2.0). It writes `crates/convert/src/onnx/onnx.rs`. Normal builds have no build script and no protoc.
- **Templates are embedded in the binary** with `include_bytes!` from `assets/templates/`. They are small because they carry no weights. Embedding means no runtime lookup, no "template missing" failure mode, and nothing extra for the M6 installer to copy. This is a small change from PRD §9, which planned to ship them as files.
- **Validation runs in `vc-convert` behind a default `validate` feature**, which pulls in `vc-engine`. The fuzz crate turns it off, so the Linux job never builds ONNX Runtime or the engine.

## Architecture

### `tools/export-templates/` (dev only, not shipped)
- **Files:** `export.py`, `requirements.txt` (pinned versions) and `README.md` (venv setup, RVC commit, how to re-export).
- **Export.** The script builds the WebUI's ONNX-friendly generator (`SynthesizerTrnMsNSFsidM`, the class `infer/modules/onnx/export.py` uses) for 3 configs: v2 32k, v2 48k, and 40k (the v1 40k config with 768-dim features, as RVC v2 does).
  - It exports with the WebUI signature (`phone, phone_lengths, pitch, pitchf, ds, rnd → audio`), the same dynamic axes and the same opset.
  - `do_constant_folding=False`; weight norm stays as `weight_g` / `weight_v`.
- **Stripping.** It then strips the data of every initializer whose name is a generator state-dict key and keeps its dtype and dims. Other constants keep their data.
- **Self-checks.** It asserts that every stripped name is a state-dict key, and that `emb_g.weight` is present (its first dim comes from the checkpoint at conversion time).
- **`--check <model.pth>` mode.** It fills the template the same way the Rust converter does and compares the ONNX output with PyTorch on a fixed input (risk mitigation from PRD §16). The agent runs it once per sample rate with corpus models.
- **Output:** `assets/templates/rvc-v2-f0-{32k,40k,48k}.onnx` and `assets/templates/templates.toml`. The manifest gives, per file: the architecture config it matches, its SHA-256, the RVC commit and the torch version.

### `crates/convert` (`vc-convert`)

| Module | Purpose |
|---|---|
| `pickle.rs` | Restricted unpickler (below) → a small `Value` tree |
| `pth.rs` | Opens the `.pth` zip (or a `.zip` holding one), runs the unpickler, resolves storages to tensors, fp16/bf16 → fp32 |
| `checkpoint.rs` | Reads the RVC dict: `weight`, `config`, `sr`, `f0`, `version`, `info` → `Checkpoint`; classifies v1, training checkpoint, no F0 |
| `template.rs` | Embedded templates + manifest; `match_template(&config)`; `fill(template, &checkpoint) -> ModelProto` |
| `onnx/` | Committed prost types; `write(model) -> Vec<u8>`; `metadata_props` |
| `validate.rs` (feature `validate`) | Loads on CPU and runs a synthetic input |
| `lib.rs` | `inspect(path) -> Inspection`, `convert(&Inspection, &Cancel, progress) -> Converted`, `ConvertError` |

**Restricted unpickler.**
- **Opcodes:** only the protocol-2 opcodes `torch.save` emits (PROTO, FRAME, EMPTY_DICT/LIST/TUPLE, MARK, the int/float/string/bytes loads, TUPLE1–3, SETITEM(S), APPEND(S), the memo ops, GLOBAL, REDUCE, BUILD, BINPERSID, NEWTRUE/NEWFALSE/NONE, STOP).
- **Allowed globals:**
  - `collections.OrderedDict`;
  - `torch._utils._rebuild_tensor_v2` and `torch._utils._rebuild_parameter`;
  - `torch.{Float,Half,BFloat16,Long,Int,Bool}Storage`.
- Any other GLOBAL, or REDUCE/BUILD on anything else, gives `UnsafeContent`.
- `BINPERSID` accepts only `('storage', <allowed storage>, key, location, numel)`.
- **Hard limits:** stack depth, memo size, string length, total opcodes and container sizes. Errors only, never a panic.
- **Tensors:** they must be contiguous and lie inside their storage's bytes (`archive/data/<key>`, little-endian).
- **Zip limits:** a cap on entry count and total uncompressed size. A nested `.zip` must hold exactly one inference `.pth` (`G_*`/`D_*` don't count); more than one gives "This zip holds several models; extract the one you want and convert that." The `.index` file is ignored.

**Template matching and filling.**
- The key is the sample rate plus every architecture field of `config`. Training-only fields and `spk_embed_dim` are left out.
- Every stripped initializer must have a tensor with the same name and the same shape, except `emb_g.weight`'s first dim, which comes from the checkpoint. Otherwise the error is `NoTemplate`, with the details in the log.
- Extra checkpoint keys (such as `enc_q.*`) are ignored, and their names are logged.
- `metadata_props`:
  - `sr` (e.g. `40000`, which the engine already reads);
  - `f0=1`;
  - `version=v2`;
  - `speakers`;
  - `source_sha256`;
  - `converter=vc-convert <version>`.

**Validation.**
- It calls `vc_engine::Generator::load_from_memory(bytes, Provider::Cpu, None)`, which checks the signature and the `sr`, then runs 1 s of synthetic input: random 768-dim features with a voiced 220 Hz pitch track, speaker 0.
- It checks that the output length matches the frames × hop at the model's rate, that every value is finite, and that the RMS is above −60 dBFS.

**Progress and cancel.**
- Progress has 4 stages: Read file, Fill weights, Check model, Save to library. Each stage reports a fraction over an `mpsc` channel.
- `Cancel` is an `Arc<AtomicBool>`, checked between stages and per tensor. A cancelled run leaves no file behind.

### `crates/models`
- New `Library::add_bytes(name, bytes, sample_rate, f0)`. It writes into the library folder atomically, with the same id and unique-file rules as `import`, then indexes the file. The caller then sets the default pitch and icon with the existing methods.

### `crates/app`
- `tabs/convert.rs` follows the approved mockup:
  - **No file:** Choose file…, or drop a file on the window.
  - **File read:** version, rate, F0, speakers, file size (and fp16), training info; name (default: the file name), default pitch, icon; then **Convert**.
  - **Converting:** a stage label, a progress bar and Cancel. The inputs are locked.
  - **Done:** **Use now** (the existing `select_model`), **Show in Models** (switches the tab), **Export .onnx…** (an `rfd` save dialog, then a copy) and **Convert another…**.
  - **Problem:** the PRD message, plus Open logs folder.
- Inspecting and converting both run on a `vc-convert` worker thread, the same way `import.rs` uses `spawn`. The UI polls the result and repaints while work is running.
- Setting `convert_file` (from Open in Convert, or from a `.pth`/`.zip` dropped while the Convert tab is open) starts an inspection.
- Inspection is quick: it reads the pickle and config without the tensors, then hashes the file.

### CI and repo
- `ci.yml` gains a `fuzz` job (`ubuntu-latest`, nightly, `cargo install cargo-fuzz`). It runs `cargo fuzz run pickle` and `cargo fuzz run pth`, about 150 s each, with the committed seed corpus.
- `crates/convert/fuzz/` is outside the workspace (`exclude`).
- Also updated: CHANGELOG (Unreleased/Added), README (the Convert tab, `xtask fetch-test-models`, how templates are made) and AGENTS.md (the `.pth` rule already exists).
- New dependencies, all checked by `cargo deny`: `zip`, `prost`, `half` and `sha2` (runtime); `prost-build` and `protox` (xtask only).

## Data and interfaces
```rust
pub struct Inspection { pub source: PathBuf, pub pth_name: String, pub file_size: u64, pub fp16: bool,
    pub version: String, pub sample_rate: u32, pub f0: bool, pub speakers: u32, pub info: Option<String>, pub sha256: String }
pub enum Stage { Read, Fill, Check, Save }
pub struct Progress { pub stage: Stage, pub fraction: f32 }
pub struct Converted { pub onnx: Vec<u8>, pub sample_rate: u32 }
pub enum ConvertError { NotPyTorch, UnsafeContent(String), RvcV1, TrainingCheckpoint, NoF0, SeveralModels,
    NoTemplate(String), ValidationFailed(String), DiskFull, Cancelled, Io(io::Error) }
impl ConvertError { pub fn user_message(&self) -> &'static str }   // PRD §5.8 table; details go to the log via Display
```

## Error handling and edge cases

| Case | Detected by | Message |
|---|---|---|
| Not a zip, a legacy (non-zip) torch file, or no `data.pkl` | `pth.rs` | "This file isn't a PyTorch model." |
| A global or opcode that isn't allowed, or a limit exceeded | `pickle.rs` | "…contains code the app won't run… It may be unsafe." |
| v1: `version` is missing or `v1`, or `enc_p.emb_phone.weight` is 256-dim | `checkpoint.rs` | "RVC v1 models aren't supported…" |
| Keys `model`/`optimizer`/`iteration`, or a `G_*`/`D_*` name | `checkpoint.rs` | "This is a training checkpoint…" |
| `f0 = 0` | `checkpoint.rs` | "This model's settings aren't supported yet." (models without F0 come later) |
| Config or shapes match no template | `template.rs` | "This model's settings aren't supported yet." (config in the log) |
| Shape, NaN/inf or silence check fails | `validate.rs` | "The converted model didn't pass the checks and wasn't added." |
| `io::ErrorKind::StorageFull` while saving or exporting | `models` / `app` | "Not enough disk space to save the converted model." |
| Cancel | any stage | Back to the file-read state, with nothing written |

- Problems are shown right after inspection, when possible, before the user presses Convert.

## Testing
Tests are written first and observed failing, as in the quality gate.
- **Fixtures.** A test-only Rust `.pth` writer (pickle protocol 2 plus the torch zip layout) builds tiny synthetic checkpoints: fp16 and fp32, v1, training-style, no-F0, a malicious GLOBAL (`os.system`), non-contiguous strides, and out-of-range storage offsets.
- **Unit tests:**
  - every unpickler opcode;
  - allow-list rejections;
  - limits;
  - fp16/bf16 → fp32 (exact values);
  - classification of each error case;
  - template matching on the configs in the manifest;
  - fill with a toy template (missing tensor, wrong shape, `emb_g` speaker count);
  - metadata written and read back;
  - the `user_message` mapping;
  - `Library::add_bytes`.
- **Real templates** (with `assets_or_skip` for the ORT DLL):
  - for each committed template, a synthetic checkpoint with matching shapes and small random weights converts and loads in `Generator`;
  - its output is the right length and finite.
- **Mutation tests** (stable, in `cargo test`): a seeded RNG applies bit flips, truncations and splices to the fixtures, for N iterations. The result must be `Ok` or `Err`, never a panic or a hang.
- **Fuzz** (CI, Linux): the `pickle` and `pth` targets.
- **Corpus** (`#[ignore]`, local): every model in `assets/test-models.toml` converts and validates, with a per-model report and a pass rate of at least 95%. It runs once before the PR, and the result goes in the PR description. A conversion time under 30 s for a 55 MB file is noted there too.
- **App:** the state transitions of the convert controller (inspect → ready → converting → done / error / cancelled), using the existing app test helpers.
- **Manual check:** convert a model while the voice changer runs (no dropouts), then Use now, Show in Models and Export.

## Out of scope
- A Python fallback converter and `.safetensors` input (PRD §17, decided: no).
- Models without F0, RVC v1, and training checkpoints.
- Feature retrieval with `.index` files, and a multi-speaker UI.
- Downloading the test corpus in CI.
- Shipping templates as separate installer files (they are embedded).
