# VC-7 — M5 Convert tab: implementation plan

## Goal
Users convert a PyTorch RVC v2 `.pth` (or a `.zip` holding one) into an ONNX model inside the app, with no Python and nothing from the file executed. The result is checked and added to the model library.

## Architecture
`vc-convert` (`crates/convert`) reads the `.pth` zip with its own restricted unpickler. It turns fp16 and bf16 into fp32 and fills one of three weight-free ONNX templates (exported once with Python by `tools/export-templates/` and embedded with `include_bytes!`). It writes the result with committed `prost` types and validates it on the CPU through `vc_engine::Generator` (default feature `validate`). The app runs inspection and conversion on worker threads through a small, testable controller (`crates/app/src/convert.rs`), writes the result into the library with the new `vc_models::stage_bytes`, and shows the five approved states on the Convert tab. CI gains a Linux cargo-fuzz job. A public-model corpus test runs locally only.

## Conventions every step follows
- **Tests first.** Write the test, run it and watch it fail (a compile error for a missing item counts), implement, then run it again and watch it pass. Use real behaviour, not mocks: injected closures are fine, as `import.rs` does. Use `#[expect(dead_code, reason = "…")]`, never `#[allow]`. The one exception is the generated prost module (see Appendix E).
- **Gate (G)**, run before every commit:
  `cargo fmt --all -- --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test --workspace; cargo deny check; cargo build --release`
  Run `cargo xtask fetch-models` once first, so the ORT DLL exists. Baseline: 230 passed, 0 failed, 3 ignored. That count may only grow.
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never rewrite history. Never commit `.megoocolab/`, `vendor/` or any model file other than the 3 templates.
- **Deviation from the design:** `ConvertError` lives in `crates/convert/src/error.rs` and is re-exported from `lib.rs`, so that `lib.rs` stays small. Its behaviour is unchanged.

## File map
| File | Responsibility |
|---|---|
| `Cargo.toml` | add `exclude = ["crates/convert/fuzz"]` |
| `crates/convert/Cargo.toml` | deps, `[features] default = ["validate"]` |
| `crates/convert/src/lib.rs` | modules, `Cancel`, `Stage`, `Progress`, `Inspection`, `Converted`, `inspect`, `convert` |
| `crates/convert/src/error.rs` | `ConvertError`, `user_message`, `From<io::Error>` |
| `crates/convert/proto/onnx.proto`, `crates/convert/proto/LICENSE` | vendored ONNX schema (Apache-2.0, onnx v1.17.0) |
| `crates/convert/src/onnx/mod.rs` | `include!("onnx.rs")`, `decode`, `encode`, metadata helpers |
| `crates/convert/src/onnx/onnx.rs` | prost output, committed, written by `cargo xtask gen-onnx` |
| `crates/convert/src/pickle.rs` | restricted unpickler → `Value` |
| `crates/convert/src/pth.rs` | torch zip / nested zip, storages → fp32 |
| `crates/convert/src/checkpoint.rs` | `ArchConfig`, `Checkpoint`, classification |
| `crates/convert/src/template.rs` | embedded templates + manifest, matching, shape check, fill, metadata |
| `crates/convert/src/validate.rs` | (feature `validate`) synthetic run and checks |
| `crates/convert/src/testutil.rs` | (`cfg(test)`) `assets_or_skip`, `TempDir`, test `.pth` writer, fixtures |
| `crates/convert/src/mutation_tests.rs` | (`cfg(test)`) seeded mutation tests, seed-corpus writer |
| `crates/convert/tests/corpus.rs` | `#[ignore]` public-model corpus test |
| `crates/convert/fuzz/**` | cargo-fuzz crate (outside the workspace), `pickle` and `pth` targets, seed corpus |
| `assets/templates/rvc-v2-f0-{32k,40k,48k}.onnx`, `assets/templates/templates.toml` | weight-free templates and their manifest |
| `assets/test-models.toml` | pinned public corpus (about 20 models) |
| `tools/export-templates/{export.py,requirements.txt,README.md}` | dev-only template export and check |
| `xtask/src/main.rs`, `xtask/src/gen_onnx.rs`, `xtask/Cargo.toml` | `gen-onnx` and `fetch-test-models` |
| `crates/models/src/library.rs`, `crates/models/src/lib.rs` | `stage_bytes`, `Library::add_bytes` |
| `crates/app/Cargo.toml` | depend on `vc-convert` |
| `crates/app/src/convert.rs` | `Converter` controller (worker threads, state machine) |
| `crates/app/src/tabs/convert.rs` | the five-state Convert tab |
| `crates/app/src/app.rs`, `crates/app/src/import.rs`, `crates/app/src/files.rs`, `crates/app/src/main.rs` | wiring, drop routing, save dialog |
| `.github/workflows/ci.yml` | `fuzz` job |
| `CHANGELOG.md`, `README.md`, `AGENTS.md` | docs |

## Steps

### 1. vc-convert: error, cancel and progress types
See the step detail. The contracts are in Appendix A.

### 2. ONNX protobuf types with prost (`cargo xtask gen-onnx`)
See the step detail and Appendix E.

### 3. Test `.pth` writer and restricted unpickler
See the step detail and Appendix B.

### 4. `.pth` archive reader (zip, nested zip, storages, fp16/bf16 → fp32)
See the step detail and Appendix B (zip limits).

### 5. RVC checkpoint reading and classification
See the step detail and Appendix C.

### 6. Public test-model manifest and `cargo xtask fetch-test-models`
See the step detail and Appendix D.1.

### 7. Export and commit the weight-free templates (Python, dev only)
See the step detail and Appendix D.

### 8. Embedded templates: matching, shape check, fill and metadata
See the step detail and Appendix E.

### 9. `inspect` and `convert` with progress and cancel
See the step detail and Appendix A.

### 10. Validation (feature `validate`) and real-template tests
See the step detail.

### 11. Seeded mutation tests and fuzz seed corpus
See the step detail.

### 12. cargo-fuzz crate and the Linux CI fuzz job
See the step detail and Appendix G.

### 13. vc-models: `stage_bytes` and `Library::add_bytes`
See the step detail.

### 14. app: the convert controller
See the step detail and Appendix F.

### 15. app: the Convert tab UI and wiring
See the step detail and Appendix F.

### 16. Public-model corpus test (local, ≥ 95%)
See the step detail.

### 17. Docs and the final gate
See the step detail.

(The step details in the `steps` array are the full instructions. They are repeated here in short form only to save space; the appendices hold the shared contracts.)

---

## Appendix A — vc-convert public API

```rust
// error.rs
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("not a PyTorch zip archive")] NotPyTorch,
    #[error("unsafe or unsupported pickle content: {0}")] UnsafeContent(String),
    #[error("RVC v1 model")] RvcV1,
    #[error("training checkpoint")] TrainingCheckpoint,
    #[error("model without F0 (f0 = 0)")] NoF0,
    #[error("the zip holds several inference models")] SeveralModels,
    #[error("no matching template: {0}")] NoTemplate(String),
    #[error("validation failed: {0}")] ValidationFailed(String),
    #[error("not enough disk space")] DiskFull,
    #[error("cancelled")] Cancelled,
    #[error("{0}")] Io(std::io::Error),
}
impl From<std::io::Error> for ConvertError { /* ErrorKind::StorageFull → DiskFull, else Io */ }
impl ConvertError { pub fn user_message(&self) -> &'static str }
```
`user_message` (the PRD §5.8 table, word for word):
| Variant | Message |
|---|---|
| NotPyTorch | This file isn't a PyTorch model. |
| UnsafeContent | This file contains code the app won't run, so it can't be converted. It may be unsafe. |
| RvcV1 | RVC v1 models aren't supported. Use an RVC v2 model. |
| TrainingCheckpoint | This is a training checkpoint. Export the model's weights file from RVC and convert that instead. |
| NoF0, NoTemplate | This model's settings aren't supported yet. |
| SeveralModels | This zip holds several models; extract the one you want and convert that. |
| ValidationFailed | The converted model didn't pass the checks and wasn't added. |
| DiskFull | Not enough disk space to save the converted model. |
| Cancelled | The conversion was cancelled. |
| Io | The file couldn't be read or written. Details are in the log. |

```rust
// lib.rs
pub mod checkpoint; pub mod onnx; pub mod pickle; pub mod pth; pub mod template;
#[cfg(feature = "validate")] pub mod validate;
mod error; pub use error::ConvertError;
#[cfg(test)] mod testutil; #[cfg(test)] mod mutation_tests;

pub const CONVERTER: &str = concat!("vc-convert ", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);
impl Cancel { pub fn new() -> Self; pub fn cancel(&self); pub fn is_cancelled(&self) -> bool;
              pub fn check(&self) -> Result<(), ConvertError> /* Err(Cancelled) when set */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Stage { Read, Fill, Check, Save }
impl Stage { pub const ALL: [Stage; 4] = [Read, Fill, Check, Save]; pub fn index(self) -> usize }
#[derive(Clone, Copy, Debug, PartialEq)] pub struct Progress { pub stage: Stage, pub fraction: f32 }

#[derive(Clone, Debug, PartialEq)]
pub struct Inspection { pub source: PathBuf, pub pth_name: String, pub file_size: u64, pub fp16: bool,
    pub version: String, pub sample_rate: u32, pub f0: bool, pub speakers: u32, pub info: Option<String>, pub sha256: String }
#[derive(Debug)] pub struct Converted { pub onnx: Vec<u8>, pub sample_rate: u32 }

pub fn inspect(path: &Path) -> Result<Inspection, ConvertError>;                 // fs::read + inspect_with(templates())
pub fn convert(i: &Inspection, cancel: &Cancel, progress: &Sender<Progress>) -> Result<Converted, ConvertError>;
// crate-internal, for tests with toy templates:
fn inspect_with(source: &Path, bytes: Vec<u8>, templates: &[Template]) -> Result<Inspection, ConvertError>;
fn convert_with(i: &Inspection, bytes: Vec<u8>, templates: &[Template], cancel: &Cancel, progress: &Sender<Progress>) -> Result<Converted, ConvertError>;
```

## Appendix B — restricted unpickler and zip limits

**`pickle.rs` types**
```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum DType { F32, F16, BF16, I64, I32, Bool }  // fn size(self) -> usize
#[derive(Clone, Debug, PartialEq)] pub struct TensorRef { pub key: String, pub dtype: DType, pub storage_numel: usize, pub offset: usize, pub shape: Vec<usize> }
#[derive(Clone, Debug, PartialEq)]
pub enum Value { None, Bool(bool), Int(i64), Float(f64), Str(String), Bytes(Vec<u8>),
                 List(Vec<Value>), Tuple(Vec<Value>), Dict(Vec<(Value, Value)>), Tensor(TensorRef) }
impl Value { pub fn get(&self, key: &str) -> Option<&Value>; pub fn as_str(&self) -> Option<&str>;
             pub fn as_int(&self) -> Option<i64>; pub fn as_list(&self) -> Option<&[Value]> /* List or Tuple */ }
pub fn load(bytes: &[u8]) -> Result<Value, ConvertError>;
```
The stack and memo hold a private `enum Item { V(Value), Mark, Global(Allowed), Storage { dtype: DType, key: String, numel: usize } }`. The result of `STOP` must be `Item::V`, otherwise `UnsafeContent`. A memo entry is a **snapshot taken at PUT time**: RVC files never re-reference a container after mutating it. Every `BINGET` clone is charged against `MAX_NODES` (nodes counted recursively), which stops "billion laughs" input.

**Opcodes (all others → `UnsafeContent("opcode 0x..")`).**
- PROTO `0x80` (version ≤ 5)
- FRAME `0x95` (8-byte length, ignored)
- EMPTY_DICT `}`, EMPTY_LIST `]`, EMPTY_TUPLE `)`, MARK `(`
- TUPLE `t`, TUPLE1 `0x85`, TUPLE2 `0x86`, TUPLE3 `0x87`
- BININT `J`, BININT1 `K`, BININT2 `M`, LONG1 `0x8a` (≤ 8 bytes, two's complement LE)
- BINFLOAT `G` (big-endian f64)
- BINUNICODE `X`, SHORT_BINUNICODE `0x8c`, BINSTRING `T`, SHORT_BINSTRING `U` (UTF-8 → Str, else Bytes)
- BINBYTES `B`, SHORT_BINBYTES `C`
- SETITEM `s`, SETITEMS `u` (target must be Dict), APPEND `a`, APPENDS `e` (target must be List)
- BINPUT `q`, LONG_BINPUT `r`, MEMOIZE `0x94`, BINGET `h`, LONG_BINGET `j`
- GLOBAL `c`, REDUCE `R`, BUILD `b`, BINPERSID `Q`
- NEWTRUE `0x88`, NEWFALSE `0x89`, NONE `N`, STOP `.`

**Allowed globals** (`GLOBAL module\nname\n`); anything else gives `UnsafeContent("global os.system")`:
- `collections OrderedDict`: REDUCE with `()` gives an empty `Dict`.
- `torch._utils _rebuild_tensor_v2`: REDUCE with `(Storage, offset:int, size:tuple, stride:tuple, requires_grad:bool, backward_hooks:Dict[, metadata:None|Dict])` gives `Tensor`.
- `torch._utils _rebuild_parameter`: REDUCE with `(Tensor, bool, Dict)` gives that `Tensor`.
- `torch FloatStorage|HalfStorage|BFloat16Storage|LongStorage|IntStorage|BoolStorage`: only as the 2nd element of a BINPERSID tuple.
- REDUCE on anything else gives `UnsafeContent`. BUILD is accepted only on a `Dict` target with a `Dict` or `None` state, and the state is discarded (`_metadata` of state dicts). Any other BUILD gives `UnsafeContent`.
- BINPERSID pops `('storage', StorageGlobal, key:str, location:str, numel:int)` and gives `Item::Storage`. Any other shape gives `UnsafeContent`.

**Tensor checks at REDUCE.** size and stride have the same length, at most 8, all ints ≥ 0. The tensor is contiguous: for each dim with size > 1, stride equals the product of the later sizes. `offset + numel(shape) ≤ storage_numel`, using checked arithmetic. A scalar `[]` has numel 1. Failure gives `UnsafeContent("tensor …")`.

**Limits** (`pub const`): `MAX_OPS = 10_000_000`, `MAX_STACK = 1_000_000`, `MAX_MARKS = 1_024`, `MAX_MEMO = 1_000_000`, `MAX_STRING = 16 << 20`, `MAX_CONTAINER = 1_000_000`, `MAX_NODES = 10_000_000`. Exceeding one gives `UnsafeContent("limit: …")`. Truncated input gives `UnsafeContent("truncated")`. Never panic, never index unchecked, use `checked_*` arithmetic.

**`pth.rs`**
```rust
pub const MAX_ENTRIES: usize = 100_000; pub const MAX_TOTAL: u64 = 4 << 30;
pub const MAX_PKL: u64 = 64 << 20;      pub const MAX_PTH: u64 = 2 << 30;
pub struct PthArchive { /* ZipArchive<Cursor<Vec<u8>>>, prefix */ pub name: String, pub root: Value }
impl PthArchive {
    pub fn open(bytes: Vec<u8>, file_name: &str) -> Result<Self, ConvertError>;
    pub fn tensor_f32(&mut self, t: &TensorRef) -> Result<Vec<f32>, ConvertError>;
}
pub fn is_training_name(name: &str) -> bool; // base name starts with "G_" or "D_" (ASCII case-insensitive)
```
`open`:
1. If it isn't a zip (`ZipArchive::new` fails, including legacy non-zip torch files), return `NotPyTorch`.
2. More than `MAX_ENTRIES` entries, or a sum of declared sizes above `MAX_TOTAL`, gives `UnsafeContent("zip limits")`.
3. If an entry is `<prefix>/data.pkl` (exactly one `/`), the file is a torch archive: `name = file_name`. Read `data.pkl` (declared size ≤ `MAX_PKL`, read through `take(MAX_PKL + 1)`) and run `pickle::load`.
4. Otherwise treat it as an outer zip. The candidates are non-directory entries ending in `.pth` (any case), and `is_training_name` splits them. More than one inference `.pth` gives `SeveralModels`. None, but some training ones, gives `TrainingCheckpoint`. None at all gives `NotPyTorch`. With exactly one, read it (≤ `MAX_PTH`) and open it as a torch archive only (no further nesting, so a zip inside it gives `NotPyTorch`), with `name` = its base name. `.index` files are ignored.

`tensor_f32`:
- Only F32, F16 and BF16 are accepted; any other dtype gives `NoTemplate("tensor dtype …")`.
- It reads `<prefix>/data/<key>`. A missing entry, or one shorter than `storage_numel × size`, gives `NotPyTorch` (log the detail with `tracing::warn!`).
- It slices `[offset, offset + numel)` and decodes little-endian: `half::f16::from_bits`, `half::bf16::from_bits`, `f32::from_le_bytes`.

## Appendix C — RVC checkpoint

Top-level dict keys: `weight` (dict name → Tensor), `config` (list), `info` (str), `sr` ("40k"), `f0` (int), `version` ("v2").

`config` indices (the v2 list has 18 items; any other length gives `NoTemplate("config: …")`):
| i | field | in `ArchConfig` |
|---|---|---|
| 0 | spec_channels | yes |
| 1 | segment_size | no (training) |
| 2 | inter_channels | yes |
| 3 | hidden_channels | yes |
| 4 | filter_channels | yes |
| 5 | n_heads | yes |
| 6 | n_layers | yes |
| 7 | kernel_size | yes |
| 8 | p_dropout | no (training) |
| 9 | resblock (str; an int becomes a string) | yes |
| 10 | resblock_kernel_sizes | yes |
| 11 | resblock_dilation_sizes | yes |
| 12 | upsample_rates | yes |
| 13 | upsample_initial_channel | yes |
| 14 | upsample_kernel_sizes | yes |
| 15 | spk_embed_dim | no (it comes from `emb_g.weight`) |
| 16 | gin_channels | yes |
| 17 | sr | yes |

```rust
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct ArchConfig { pub spec_channels: u32, pub inter_channels: u32, pub hidden_channels: u32, pub filter_channels: u32,
    pub n_heads: u32, pub n_layers: u32, pub kernel_size: u32, pub resblock: String, pub resblock_kernel_sizes: Vec<u32>,
    pub resblock_dilation_sizes: Vec<Vec<u32>>, pub upsample_rates: Vec<u32>, pub upsample_initial_channel: u32,
    pub upsample_kernel_sizes: Vec<u32>, pub gin_channels: u32, pub sr: u32 }
impl ArchConfig { pub fn from_values(list: &[Value]) -> Result<Self, ConvertError>; pub fn hop(&self) -> u32 /* product of upsample_rates */ }
pub struct Checkpoint { pub version: String, pub sample_rate: u32, pub f0: bool, pub speakers: u32, pub fp16: bool,
    pub info: Option<String>, pub config: ArchConfig, pub weights: BTreeMap<String, TensorRef> }
impl Checkpoint { pub fn from_value(root: &Value, pth_name: &str) -> Result<Self, ConvertError> }
```
Classification order in `from_value`:
1. If `root` isn't a Dict, return `NoTemplate("not an RVC model")`.
2. If `is_training_name(pth_name)`, or the keys `model`, `optimizer` or `iteration` are present, return `TrainingCheckpoint`.
3. If `weight` is missing or isn't a Dict of Tensors, return `NoTemplate("no weight dict")`.
4. If `version` is missing or `"v1"`, or `enc_p.emb_phone.weight` has a last dim of 256, return `RvcV1`.
5. If `f0` is 0, return `NoF0` (a missing `f0` counts as 1).
6. Run `ArchConfig::from_values`. If `sr` is not in {32000, 40000, 48000}, return `NoTemplate`.
7. `speakers` is `emb_g.weight.shape[0]`; if it is missing, return `NoTemplate`. `fp16` is true when any weight is F16 or BF16. `sample_rate` is `config.sr`.

Reference v2 40k config, used in tests: `[1025, 32, 192, 192, 768, 2, 6, 3, 0, "1", [3,7,11], [[1,3,5],[1,3,5],[1,3,5]], [10,10,2,2], 512, [16,16,4,4], 109, 256, 40000]`. For 48k, use `upsample_rates [12,10,2,2]`, kernels `[24,20,4,4]`, sr 48000. For 32k, use spec 513, `[10,8,2,2]`, `[20,16,4,4]`, sr 32000.

## Appendix D — templates (dev only)

**D.1 Throwaway venv, outside the repo** (PowerShell):
```
py -3.11 -m venv $env:TEMP\vc-export-venv
& $env:TEMP\vc-export-venv\Scripts\python.exe -m pip install -r tools\export-templates\requirements.txt --extra-index-url https://download.pytorch.org/whl/cpu
git clone https://github.com/RVC-Project/Retrieval-based-Voice-Conversion-WebUI $env:TEMP\rvc
git -C $env:TEMP\rvc checkout <RVC_COMMIT>
```
Always run Python with `-I`. Load downloaded `.pth` files only with `torch.load(p, map_location="cpu", weights_only=True)`.

**D.2 `requirements.txt`** (pinned): `torch==2.4.1+cpu`, `onnx==1.16.2`, `onnxruntime==1.19.2`, `numpy==1.26.4`. Add any further module the RVC `infer/lib/infer_pack` imports need, pinned. torch 2.4 uses the legacy TorchScript exporter, which keeps state-dict initializer names.

**D.3 `export.py`** (argparse: `--rvc <path>` adds the RVC checkout to `sys.path`; `--out assets/templates`; `--check <model.pth>` runs check mode):
- For each of `32k` (`configs/v2/32k.json`), `40k` (`configs/v1/40k.json` with 768-dim features, as RVC v2 does) and `48k` (`configs/v2/48k.json`):
  - Build the config list exactly as RVC's `infer/lib/train/process_ckpt.py` (`savee`) does (Appendix C order).
  - Build `SynthesizerTrnMsNSFsidM(*config, is_half=False, version="v2")` from `infer/lib/infer_pack/models_onnx.py`.
  - Export with `torch.onnx.export`:
    - inputs `phone [1,200,768] f32`, `phone_lengths [200]`, `pitch [1,200] i64`, `pitchf [1,200] f32`, `ds [0]`, `rnd [1,192,200] f32`;
    - `input_names=["phone","phone_lengths","pitch","pitchf","ds","rnd"]`, `output_names=["audio"]`;
    - `dynamic_axes={"phone":[1],"pitch":[1],"pitchf":[1],"rnd":[2]}`, `opset_version=13`, `do_constant_folding=False` (the same as RVC `infer/modules/onnx/export.py`).
  - Then, in `onnx`: let `keys = net_g.state_dict().keys()`. For each initializer whose name is in `keys`, assert `data_type == FLOAT`, then `ClearField("raw_data")` and clear `float_data`, keeping `dims`.
  - Self-checks:
    - `emb_g.weight` was stripped;
    - every stripped name is in `keys`;
    - every key not starting with `enc_q.` was stripped, unless it is listed in a module-level `UNUSED = {...}` set with a comment saying why (a renamed parameter would keep random weights).
  - Save `rvc-v2-f0-{rate}.onnx` and write `templates.toml`.
- `--check <pth>`:
  - Load the checkpoint, pick the template whose config equals the checkpoint's config (ignoring segment_size, p_dropout and spk_embed_dim), and fill it like Rust does: by name, fp32 raw data, `emb_g.weight` dims from the checkpoint.
  - Run onnxruntime on CPU, and run PyTorch `SynthesizerTrnMsNSFsidM` (with `load_state_dict(weight, strict=False)`) on the same fixed seeded input (T = 100, the same `rnd`).
  - Print the max absolute difference, and exit with 1 if it exceeds `1e-3 × max(1, peak)`.

**D.4 `assets/templates/templates.toml`**
```toml
[[template]]
file = "rvc-v2-f0-40k.onnx"
sha256 = "<64 hex>"
rvc_commit = "<40 hex>"
torch = "2.4.1+cpu"
[template.config]
spec_channels = 1025
inter_channels = 192
hidden_channels = 192
filter_channels = 768
n_heads = 2
n_layers = 6
kernel_size = 3
resblock = "1"
resblock_kernel_sizes = [3, 7, 11]
resblock_dilation_sizes = [[1, 3, 5], [1, 3, 5], [1, 3, 5]]
upsample_rates = [10, 10, 2, 2]
upsample_initial_channel = 512
upsample_kernel_sizes = [16, 16, 4, 4]
gin_channels = 256
sr = 40000
```
(It has one `[[template]]` per file: 32k, 40k and 48k.)

## Appendix E — ONNX, fill and metadata

- `xtask/src/gen_onnx.rs`: `pub fn generate(proto_dir: &Path, out_dir: &Path) -> Result<PathBuf, String>`.
  - `protox::compile([proto_dir.join("onnx.proto")], [proto_dir])`, then `prost_build::Config::new().out_dir(out_dir).compile_fds(fds)`, which writes `onnx.rs`.
  - `cargo xtask gen-onnx` copies it to `crates/convert/src/onnx/onnx.rs`.
  - Use matching versions, `prost`/`prost-build` 0.14 with `protox` 0.9. If `cargo tree -d` shows two `prost` versions, align them.
- `crates/convert/src/onnx/mod.rs`:
```rust
#[allow(clippy::all, clippy::pedantic, reason = "generated by prost-build")]
mod pb { include!("onnx.rs"); }   // include! keeps rustfmt off the generated file
pub use pb::*;
pub const FLOAT: i32 = 1;
pub fn decode(bytes: &[u8]) -> Result<ModelProto, prost::DecodeError>;
pub fn encode(model: &ModelProto) -> Vec<u8>;
pub fn set_metadata(model: &mut ModelProto, key: &str, value: &str); // replaces an existing key, else appends
pub fn metadata<'a>(model: &'a ModelProto, key: &str) -> Option<&'a str>;
```
  The `#[allow]` on `mod pb` is the one allowed exception, because `#[expect]` fails when a lint doesn't fire in generated code. proto2 optional fields come out as `Option<…>`; write the helpers against the generated types.
- `template.rs`:
```rust
#[derive(Clone, Debug, Deserialize)] pub struct TemplateInfo { pub file: String, pub sha256: String, pub rvc_commit: String, pub torch: String, pub config: ArchConfig }
pub struct Template { pub info: TemplateInfo, pub bytes: &'static [u8] }
pub fn templates() -> &'static [Template];   // OnceLock; include_str!("../../../assets/templates/templates.toml") + include_bytes! per file
pub fn match_template<'a>(templates: &'a [Template], cfg: &ArchConfig) -> Result<&'a Template, ConvertError>; // NoTemplate(format!("{cfg:?}")), logged
pub fn is_stripped(t: &TensorProto) -> bool;  // product(dims) > 0, raw_data empty/None, float_data empty
pub fn check_shapes(model: &ModelProto, weights: &BTreeMap<String, TensorRef>) -> Result<(), ConvertError>;
pub fn fill(model: &mut ModelProto, weights: &BTreeMap<String, TensorRef>,
            load: &mut dyn FnMut(&TensorRef) -> Result<Vec<f32>, ConvertError>,
            cancel: &Cancel, progress: &mut dyn FnMut(f32)) -> Result<(), ConvertError>;
pub fn write_metadata(model: &mut ModelProto, ckpt: &Checkpoint, sha256: &str);
```
  - `check_shapes`: every stripped initializer needs a weight with the same name and the same shape. `emb_g.weight` must have rank 2 and the same dim 1, and any dim 0 ≥ 1. A missing tensor or wrong shape gives `NoTemplate("<name>: …")`. Log extra weight names (such as `enc_q.*`) at `info`.
  - `fill`: for each stripped initializer i of n:
    1. `cancel.check()?`;
    2. `load`;
    3. set `dims` (for emb_g, from the weight);
    4. `data_type = FLOAT`, `raw_data = LE f32 bytes`, clear `float_data`;
    5. `progress((i+1)/n)`.
  - `write_metadata` keys: `sr` (e.g. `"40000"`), `f0` = `"1"`, `version` = `"v2"`, `speakers`, `source_sha256`, `converter` = `CONVERTER`.

## Appendix F — app controller (`crates/app/src/convert.rs`)
```rust
pub type InspectFn = Arc<dyn Fn(&Path) -> Result<Inspection, ConvertError> + Send + Sync>;
pub type ConvertFn = Arc<dyn Fn(&Inspection, &Cancel, &Sender<Progress>) -> Result<Converted, ConvertError> + Send + Sync>;
#[derive(Clone, Debug, PartialEq)] pub struct Form { pub name: String, pub pitch: i32, pub icon: Option<PathBuf> }
pub enum State {
    Empty,
    Inspecting { path: PathBuf, rx: Receiver<Result<Inspection, ConvertError>> },
    Ready { inspection: Inspection, form: Form },
    Converting { inspection: Inspection, form: Form, progress: Progress, cancel: Cancel,
                 progress_rx: Receiver<Progress>, rx: Receiver<Result<(StagedModel, u32), ConvertError>> },
    Done { id: String, name: String, note: Option<String>, export_rx: Option<Receiver<String>> },
    Problem { file: String, message: String },
}
pub struct Converter { inspect: InspectFn, convert: ConvertFn, pub state: State }
impl Converter {
    pub fn new(inspect: InspectFn, convert: ConvertFn) -> Self;
    pub fn open(&mut self, path: PathBuf);            // ignored while Converting; else → Inspecting on a "vc-convert" thread
    pub fn start(&mut self, library: &Library);       // Ready with a non-empty trimmed name → Converting; else no-op
    pub fn cancel(&mut self);                         // sets the flag; the state changes when the worker answers
    pub fn reset(&mut self);                          // → Empty (Convert another…)
    pub fn is_converting(&self) -> bool;
    pub fn is_busy(&self) -> bool;                    // Inspecting | Converting | Done with export_rx
    pub fn export(&mut self, src: PathBuf, dest: PathBuf); // Done only: copy on a "vc-convert" thread, note via export_note
    pub fn poll(&mut self, library: &mut Library) -> Option<String>; // Some(id) when a model was added
}
pub fn default_name(source: &Path) -> String;          // file stem of the chosen file
pub fn overall(p: Progress) -> f32;                    // (p.stage.index() as f32 + p.fraction.clamp(0,1)) / 4
pub fn stage_label(s: Stage) -> &'static str;          // "Reading file…", "Filling weights…", "Checking model…", "Saving to library…"
pub fn export_note(dest: &Path, r: io::Result<u64>) -> String;
// Ok → "Exported to \"{dest}\"."; StorageFull → DiskFull user_message; other → "Could not export: {e}"
```
- **Worker** (Converting):
  1. `let r = convert(&i, &cancel, &ptx)?`;
  2. `ptx.send(Progress{Save,0})`;
  3. `cancel.check()?`;
  4. `vc_models::stage_bytes(&root, &name, &r.onnx, &taken)`, mapping `LibraryError::Io(e)` to `ConvertError::from(e)` and any other error to `Io(io::Error::other(..))`;
  5. if it was cancelled now, `discard_staged` and return `Cancelled`;
  6. send `Progress{Save,1}`;
  7. return `Ok((staged, r.sample_rate))`.
- **poll** on Converting:
  - Drain the progress (keep the last value).
  - On `Ok((staged, sr))`:
    - `library.add(staged, sr, true)`; on error, `discard_staged` and go to Problem;
    - `set_default_pitch(id, form.pitch)`;
    - if there is an icon, `set_icon`. An icon failure becomes the `note` "The icon couldn't be set: {e}" and the model is kept;
    - go to Done and return `Some(id)`.
  - `Err(Cancelled)` goes back to Ready with the same inspection and form.
  - Any other `Err(e)`: `tracing::warn!("converting {}: {e}", path)` and go to Problem `{message: e.user_message()}`.
  - A disconnected channel goes to Problem with "The conversion stopped unexpectedly. Details are in the log."
- **poll** on Inspecting: `Ok` goes to Ready `{form: Form{name: default_name(&path), pitch: 0, icon: None}}`. `Err` logs and goes to Problem.

## Appendix G — fuzz crate and CI

`crates/convert/fuzz/Cargo.toml`:
```toml
[package]
name = "vc-convert-fuzz"
version = "0.0.0"
publish = false
edition = "2024"
[package.metadata]
cargo-fuzz = true
[dependencies]
libfuzzer-sys = "0.4"
vc-convert = { path = "..", default-features = false }
[workspace]
members = ["."]
[[bin]]
name = "pickle"
path = "fuzz_targets/pickle.rs"
test = false
doc = false
bench = false
[[bin]]
name = "pth"
path = "fuzz_targets/pth.rs"
test = false
doc = false
bench = false
```
- `fuzz_targets/pickle.rs`: `fuzz_target!(|d: &[u8]| { let _ = vc_convert::pickle::load(d); });`
- `fuzz_targets/pth.rs`: `PthArchive::open(d.to_vec(), "fuzz.pth")`, then on success `Checkpoint::from_value(&a.root, &a.name)`, then `tensor_f32` for every weight.
- `crates/convert/fuzz/.gitignore`: `target`, `artifacts`, `coverage`, `Cargo.lock`.

`ci.yml` job:
```yaml
  fuzz:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: crates/convert/fuzz
      - name: Install cargo-fuzz
        run: cargo install cargo-fuzz --locked
      - name: Fuzz the pickle reader
        working-directory: crates/convert
        run: cargo +nightly fuzz run pickle -- -max_total_time=150
      - name: Fuzz the .pth reader
        working-directory: crates/convert
        run: cargo +nightly fuzz run pth -- -max_total_time=150
```

## Final verification
1. Run the gate G on Windows; every test passes, with only the documented ignored tests.
2. `cargo check -p vc-convert --no-default-features` (the fuzz build shape).
3. `cargo xtask gen-onnx`, then `git diff --exit-code crates/convert/src/onnx/onnx.rs`, which shows no change.
4. `cargo xtask fetch-test-models; cargo test -p vc-convert --release --test corpus -- --ignored --nocapture`: pass rate ≥ 95%, and the 55 MB conversion takes under 30 s. Keep the report for the PR description.
5. The CI `fuzz` job passes on the PR.
6. Manual check in `cargo run -p voicechanger --release`:
   - convert a `.pth` and a `.zip` while the voice changer runs (listen for dropouts);
   - Cancel mid-way (no file appears in `%APPDATA%\VoiceChanger\models`);
   - Use now, Show in Models and Export .onnx…;
   - drop a `G_100.pth` and an `os.system` fixture and check the PRD messages;
   - Open logs folder.
