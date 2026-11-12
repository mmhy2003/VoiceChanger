# VC-3 — M1: Audio skeleton — implementation plan

## Goal
`voicechanger.exe` opens an egui window with Voice / Models / Convert / Settings tabs. The user picks a mic, a virtual cable output and an optional monitor, and Start passes the mic through to them with live meters. A device unplug stops audio cleanly and shows a banner.

## Architecture
`vc-audio` (no UI) holds the pure device logic (`devices.rs`), lock-free meters and shared state (`meter.rs`, `shared.rs`), the ring buffers and the callback bodies (`ring.rs`), the processing worker (`worker.rs`: downmix → input meter → rubato resample per output → monitor gain → output meter) and `AudioSession` (`session.rs`: 2 or 3 cpal WASAPI streams plus the worker). Signal path: input callback → `HeapRb<f32>` → worker thread → one `HeapRb<f32>` per output → output callbacks. The `voicechanger` app owns `VcApp` on the UI thread. It holds an `Option<AudioSession>` (cpal `Stream` is not `Send`) and an `Arc<Shared>`, and polls `Shared::status()` each frame.

## Facts about the repo (checked at `313ddb1`)
- Workspace `Cargo.toml`: `members = ["crates/*"]`, edition 2024, resolver 3, `[workspace.lints.clippy] all = warn`. CI runs on `windows-latest` with `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check` and `cargo build --release`.
- `crates/app/src/main.rs` contains only `fn main() {}`. `crates/audio/src/lib.rs` contains only a doc comment. Neither crate has any dependencies yet.
- `deny.toml` has a licence `allow` list (MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-2/3, ISC, Zlib, Unicode-3.0) with the comment "Widen this list only with a reason in the PR description". `[advisories]` has `yanked = "deny"`.
- Commit style is conventional (`feat(audio):`, `feat(app):`, `build:`, `docs:`). Every commit ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- The PR targets `develop`. `CHANGELOG.md` already has `## [Unreleased]` / `### Added`.
- Windows only (AGENTS.md): no `cfg` branches for other OSes.

## Pinned dependency versions
Pin these minor lines. Newer majors change the APIs used below: cpal 0.17 deprecates `name()`, rubato 1.x replaces `process_into_buffer`, and egui changes every minor.

| Crate | In | Version |
|---|---|---|
| `eframe` (use `eframe::egui`, no separate `egui` dep, so the versions can't drift) | app | `"0.32"` |
| `vc-audio` | app | `{ path = "../audio" }` |
| `cpal` | audio | `"0.16"` |
| `ringbuf` | audio | `"0.4"` |
| `rubato` | audio | `"0.16"` |
| `thiserror` | audio | `"2"` |

`anyhow` (listed in the design) is **not** added: no code path in this plan needs it, and an unused dependency only adds `cargo deny` surface. The `StartError` (thiserror) covers every error the UI shows.

## File map
| File | Responsibility |
|---|---|
| `crates/app/Cargo.toml` | add `eframe`, `vc-audio` |
| `crates/app/src/main.rs` | `run_native`, window title, `windows_subsystem` in release, drag-and-drop off (it conflicts with cpal's COM init) |
| `crates/app/src/app.rs` | `Tab`, `VcApp` state and `eframe::App` impl, start/stop/restart, status polling, `banner_text`, `window_title` |
| `crates/app/src/tabs/mod.rs` | `pub mod voice; pub mod models; pub mod convert; pub mod settings;` |
| `crates/app/src/tabs/voice.rs` | banner, Start/Stop, meters, `status_line` |
| `crates/app/src/tabs/settings.rs` | device drop-downs, monitor, Refresh, warnings, VB-CABLE link, input meter |
| `crates/app/src/tabs/models.rs`, `convert.rs` | placeholders "Coming in M4" / "Coming in M5" |
| `crates/app/src/widgets.rs` | `MeterState` (decay), `level_to_fraction`, `meter()` widget |
| `crates/audio/Cargo.toml` | add cpal, ringbuf, rubato, thiserror |
| `crates/audio/src/lib.rs` | `pub mod` + re-exports |
| `crates/audio/src/devices.rs` | `DeviceList`, `Selection`, `is_virtual_cable`, `feedback_warnings`, `Warning`, `start_blocker`, `list_devices` |
| `crates/audio/src/meter.rs` | `Level`, `LevelMeter`, `measure`, `db_to_gain`, `gain_to_db` |
| `crates/audio/src/shared.rs` | `Endpoint`, `StreamStatus`, `Shared` |
| `crates/audio/src/ring.rs` | `ring()`, `push_input`, `fill_output`, `push_output` |
| `crates/audio/src/worker.rs` | `downmix`, `OutputKind`, `OutputConfig`, `WorkerConfig`, `State`, `process_block`, `spawn` |
| `crates/audio/src/session.rs` | `AudioSession`, `SessionInfo`, `StartError`, `status_for` |
| `deny.toml` | licence additions needed by egui fonts and cpal/windows crates, and advisory ignores if unavoidable, each with a reason |
| `CHANGELOG.md` | Unreleased → Added line |

---

## Reference code

### R1 — `devices.rs` public contract
```rust
use cpal::traits::{DeviceTrait, HostTrait};

const CABLE_MARKERS: [&str; 3] = ["cable", "voicemeeter", "virtual audio cable"];

/// True for virtual audio cable endpoints (VB-CABLE, VoiceMeeter, Virtual Audio Cable).
pub fn is_virtual_cable(name: &str) -> bool {
    let lower = name.to_lowercase();
    CABLE_MARKERS.iter().any(|m| lower.contains(m))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceList {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>, // virtual cables first (stable sort), otherwise in enumeration order
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

impl DeviceList {
    pub fn from_names(inputs: Vec<String>, mut outputs: Vec<String>,
                      default_input: Option<String>, default_output: Option<String>) -> Self {
        outputs.sort_by_key(|n| !is_virtual_cable(n)); // stable: cables first
        Self { inputs, outputs, default_input, default_output }
    }
    pub fn has_cable(&self) -> bool { self.outputs.iter().any(|n| is_virtual_cable(n)) }
    /// Windows default input if it is in `inputs`, else the first input.
    pub fn default_input_name(&self) -> Option<&str>;
    /// First virtual cable output, else the Windows default output (if listed), else the first output.
    pub fn default_cable(&self) -> Option<&str>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub input: String,          // "" = nothing selected
    pub cable: String,          // "" = nothing selected
    pub monitor: Option<String>,
    pub monitor_enabled: bool,
}

impl Selection {
    /// input = default_input_name, cable = default_cable,
    /// monitor = default_output if it differs from the cable, monitor_enabled = false.
    pub fn defaults(list: &DeviceList) -> Self;
    /// After a Refresh: any selected name missing from `list` falls back to its default.
    /// A missing monitor becomes `None` and monitor_enabled = false.
    /// Returns one note per fallback, e.g.
    /// `Input "USB Mic" is no longer available; switched to "Microphone (Realtek)".`
    /// `Monitor "Headphones" is no longer available; monitor turned off.`
    /// If there is no default to fall back to, the field becomes "" and the note ends with `; no device selected.`
    pub fn reconcile(&mut self, list: &DeviceList) -> Vec<String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Warning { MonitorSameAsCable, InputIsCableRecordingSide }

impl Warning {
    pub fn message(self) -> &'static str {
        match self {
            Warning::MonitorSameAsCable => "The monitor and the cable output are the same device. Everything listening to the cable will hear the monitor too; pick headphones or speakers for the monitor.",
            Warning::InputIsCableRecordingSide => "The input is a virtual cable's recording side (e.g. \"CABLE Output\"). Sending it back into the cable creates a feedback loop; pick your microphone instead.",
        }
    }
}

/// MonitorSameAsCable: monitor_enabled && monitor == Some(cable) && !cable.is_empty().
/// InputIsCableRecordingSide: input contains "cable output" (case-insensitive) or is_virtual_cable(input).
pub fn feedback_warnings(sel: &Selection) -> Vec<Warning>;

/// Why Start is disabled, or None. Checked in this order:
/// list.inputs empty   -> "No input devices found. Connect a microphone and press Refresh."
/// list.outputs empty  -> "No output devices found. Install VB-CABLE or connect speakers, then press Refresh."
/// sel.input not in list.inputs  -> "Pick an input device in Settings."
/// sel.cable not in list.outputs -> "Pick a cable output in Settings."
pub fn start_blocker(list: &DeviceList, sel: &Selection) -> Option<&'static str>;

/// Enumerates WASAPI endpoints by name. Never fails: errors give empty lists.
pub fn list_devices() -> DeviceList {
    let Ok(host) = cpal::host_from_id(cpal::HostId::Wasapi) else { return DeviceList::default() };
    let inputs = host.input_devices().map(|ds| ds.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
    let outputs = host.output_devices().map(|ds| ds.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
    let default_input = host.default_input_device().and_then(|d| d.name().ok());
    let default_output = host.default_output_device().and_then(|d| d.name().ok());
    DeviceList::from_names(inputs, outputs, default_input, default_output)
}
```

### R2 — `meter.rs` and `shared.rs`
```rust
// meter.rs
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Level { pub peak: f32, pub rms: f32 }

/// Peak = max |x|, RMS = sqrt(mean(x²)). Empty slice -> Level::default().
pub fn measure(samples: &[f32]) -> Level;

/// Lock-free level: f32 bits in AtomicU32, Relaxed stores/loads.
#[derive(Debug, Default)]
pub struct LevelMeter { peak: AtomicU32, rms: AtomicU32 }
impl LevelMeter {
    pub fn store(&self, level: Level);
    pub fn load(&self) -> Level;
    pub fn reset(&self); // store(Level::default())
}

pub fn db_to_gain(db: f32) -> f32 { 10f32.powf(db / 20.0) }
/// 20*log10(gain), floored at -120.0 for gain <= 1e-6.
pub fn gain_to_db(gain: f32) -> f32;

// shared.rs
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};
use crate::meter::{LevelMeter, db_to_gain};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endpoint { Input, Cable, Monitor }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamStatus {
    Running,
    DeviceLost { which: Endpoint },
    Failed { which: Endpoint },
}
// u8 codes: 0 Running; 1/2/3 DeviceLost Input/Cable/Monitor; 4/5/6 Failed Input/Cable/Monitor.
impl StreamStatus { fn to_code(self) -> u8; fn from_code(c: u8) -> Self /* unknown -> Running */ }

pub const DEFAULT_MONITOR_DB: f32 = -6.0;

/// Shared by the UI, the worker and the audio callbacks (behind an Arc).
#[derive(Debug)]
pub struct Shared {
    pub input_meter: LevelMeter,
    pub output_meter: LevelMeter,
    monitor_gain: AtomicU32, // linear gain, f32 bits
    status: AtomicU8,
    pub overflows: AtomicU64,  // samples dropped
    pub underruns: AtomicU64,  // output frames filled with silence
}
impl Shared {
    pub fn new() -> Self;           // monitor gain = db_to_gain(DEFAULT_MONITOR_DB), status Running
    pub fn monitor_gain(&self) -> f32;
    pub fn set_monitor_gain(&self, gain: f32);
    pub fn status(&self) -> StreamStatus;
    /// First error wins: compare_exchange(0, code, Relaxed, Relaxed); later reports are ignored.
    pub fn report(&self, status: StreamStatus);
    /// Before each session: status Running, meters zero, counters zero. Monitor gain is kept.
    pub fn reset(&self);
}
impl Default for Shared { fn default() -> Self { Self::new() } }
```

### R3 — `ring.rs`
```rust
use ringbuf::{HeapRb, traits::{Consumer, Observer, Producer, Split}};
pub use ringbuf::{HeapCons, HeapProd};
use std::sync::atomic::Ordering::Relaxed;
use crate::shared::Shared;

/// Preallocated SPSC ring of f32 (capacity >= 1).
pub fn ring(capacity: usize) -> (HeapProd<f32>, HeapCons<f32>) { HeapRb::<f32>::new(capacity.max(1)).split() }

/// Input callback body. Pushes the whole interleaved buffer or none of it (keeps frames
/// channel-aligned). If vacant_len() < data.len(): overflows += data.len(), return.
/// Never allocates, locks or blocks.
pub fn push_input<T>(data: &[T], prod: &mut HeapProd<f32>, shared: &Shared)
where T: cpal::Sample, f32: cpal::FromSample<T>
{ /* for &s in data { let _ = prod.try_push(s.to_sample::<f32>()); } */ }

/// Output callback body. For each frame of `data` (data.len() / channels frames) pop one mono
/// sample (0.0 if empty, counting the frame as an underrun) and write T::from_sample(v) to every
/// channel. One `underruns.fetch_add` per call, only if > 0.
pub fn fill_output<T>(data: &mut [T], channels: usize, cons: &mut HeapCons<f32>, shared: &Shared)
where T: cpal::Sample + cpal::FromSample<f32>;

/// Worker-side push. If occupied_len() + samples.len() > max_backlog: overflows += samples.len()
/// and drop the block (keeps latency bounded when clocks drift). Otherwise push_slice and count any
/// samples that did not fit as overflows.
pub fn push_output(prod: &mut HeapProd<f32>, samples: &[f32], max_backlog: usize, shared: &Shared);
```

### R4 — `worker.rs`
```rust
use rubato::{FftFixedIn, Resampler};

/// Average the channels of an interleaved buffer into `out` (cleared first). channels >= 1.
/// A trailing partial frame is ignored.
pub fn downmix(interleaved: &[f32], channels: usize, out: &mut Vec<f32>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputKind { Cable, Monitor }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputConfig { pub kind: OutputKind, pub rate: u32 }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig { pub in_rate: u32, pub in_channels: usize, pub outputs: Vec<OutputConfig> }

struct OutputPath {
    kind: OutputKind,
    resampler: Option<FftFixedIn<f32>>, // None when rate == in_rate
    pending: Vec<f32>,                  // mono input waiting for a full resampler chunk
    chunk_out: Vec<Vec<f32>>,           // [1][output_frames_max()]
    ready: Vec<f32>,                    // processed mono samples from the last process_block
}

pub struct State { in_channels: usize, mono: Vec<f32>, paths: Vec<OutputPath> }

impl State {
    /// All buffers preallocated (capacities: mono/pending = in_rate, ready = out rate).
    /// Resampler: FftFixedIn::<f32>::new(in_rate as usize, rate as usize, (in_rate / 100) as usize, 1, 1).
    /// Construction errors are returned as their Display string.
    pub fn new(cfg: &WorkerConfig) -> Result<State, String>;
    pub fn output(&self, index: usize) -> &[f32]; // `ready` of output `index` (same order as cfg.outputs)
    pub fn outputs(&self) -> usize;
}

/// 1. downmix(interleaved) into state.mono
/// 2. shared.input_meter.store(measure(&mono))
/// 3. for each path: ready.clear(); pending.extend_from_slice(&mono);
///      no resampler -> ready.extend_from_slice(&pending); pending.clear();
///      resampler    -> while pending.len() >= r.input_frames_next() {
///                         let n = r.input_frames_next();
///                         let (_, out_n) = r.process_into_buffer(&[&pending[..n]], &mut chunk_out, None)
///                             .expect("buffers sized from the resampler");
///                         ready.extend_from_slice(&chunk_out[0][..out_n]);
///                         pending.drain(..n);
///                       }
/// 4. Monitor paths: multiply ready by shared.monitor_gain()
/// 5. If the first Cable path's ready is non-empty: shared.output_meter.store(measure(ready))
pub fn process_block(state: &mut State, interleaved: &[f32], shared: &Shared);

/// Spawns thread "vc-audio-worker". `outputs[i]` feeds `state.output(i)`. `max_backlogs[i]` = out rate / 5 (200 ms).
/// Loop until stop is true:
///   take = min(input.occupied_len(), scratch.len()) / ch * ch   (scratch = in_channels * (in_rate/50) samples, allocated before the loop)
///   take == 0 -> thread::sleep(2 ms); continue
///   input.pop_slice(&mut scratch[..take]); process_block(...);
///   for i: ring::push_output(&mut outputs[i], state.output(i), max_backlogs[i], &shared)
pub fn spawn(state: State, input: HeapCons<f32>, outputs: Vec<HeapProd<f32>>, max_backlogs: Vec<usize>,
             shared: Arc<Shared>, stop: Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>>;
```
`process_into_buffer` in rubato 0.16 takes `wave_in: &[V: AsRef<[f32]>]`, `wave_out: &mut [V: AsMut<[f32]>]` and `Option<&[bool]>`, and returns `ResampleResult<(usize, usize)>`. Always read the chunk size from `input_frames_next()`: FftFixedIn rounds the requested chunk to its FFT size.

### R5 — `session.rs`
```rust
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("device \"{0}\" was not found")]
    DeviceNotFound(String),
    #[error("device \"{device}\" has an unsupported format: {reason}")]
    UnsupportedConfig { device: String, reason: String },
    #[error("could not start \"{device}\": {reason}")]
    BuildStream { device: String, reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionInfo { pub input_rate: u32, pub cable_rate: u32, pub monitor_rate: Option<u32> }

pub struct AudioSession {
    streams: Vec<cpal::Stream>,   // input first, then cable, then monitor
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    info: SessionInfo,
}

pub fn status_for(which: Endpoint, err: &cpal::StreamError) -> StreamStatus {
    match err { cpal::StreamError::DeviceNotAvailable => StreamStatus::DeviceLost { which },
                _ => StreamStatus::Failed { which } }
}

impl AudioSession {
    pub fn start(sel: &Selection, shared: Arc<Shared>) -> Result<AudioSession, StartError>;
    pub fn info(&self) -> SessionInfo { self.info }
    pub fn stop(self) {} // Drop does the work
}

impl Drop for AudioSession {
    fn drop(&mut self) {
        for s in &self.streams { let _ = s.pause(); }
        self.streams.clear();                 // drops the cpal streams (callbacks stop)
        self.stop.store(true, Relaxed);
        if let Some(h) = self.worker.take() { let _ = h.join(); }
    }
}
```
`start` sequence:
1. `host = cpal::host_from_id(cpal::HostId::Wasapi)`. On error → `BuildStream { device: "WASAPI".into(), reason }`.
2. `find(host.input_devices(), &sel.input)` → `DeviceNotFound(sel.input.clone())` if no device's `name()` equals it. Same for the cable with `output_devices()`, and for the monitor only when `sel.monitor_enabled && sel.monitor.is_some()`.
3. `default_input_config()` / `default_output_config()`. On error → `UnsupportedConfig { device, reason: e.to_string() }`. Take `sample_format()`, `channels()`, `sample_rate().0` and `config()` (`StreamConfig`).
4. `shared.reset()`.
5. Rings: input `ring(in_rate as usize * in_channels)` (1 s). Each output gets `ring(out_rate as usize)` (1 s mono), with max_backlog `out_rate as usize / 5`.
6. `State::new(&WorkerConfig { .. })`. On error → `UnsupportedConfig { device: sel.input.clone(), reason }`.
7. Build the streams with `match sample_format { I16 => build::<i16>, U16 => build::<u16>, F32 => build::<f32>, other => UnsupportedConfig { reason: format!("sample format {other:?}") } }`. Input data callback: `move |data: &[T], _: &cpal::InputCallbackInfo| ring::push_input(data, &mut prod, &shared_cb)`. Output data callback: `move |data: &mut [T], _: &cpal::OutputCallbackInfo| ring::fill_output(data, channels, &mut cons, &shared_cb)`. Error callback: `move |err| shared_err.report(status_for(which, &err))`. Timeout: `None`. A build error → `BuildStream { device, reason: e.to_string() }`.
8. `stop = Arc::new(AtomicBool::new(false))`. `worker::spawn(...)`. On error → `BuildStream { device: "processing thread".into(), reason }`.
9. Construct `AudioSession` **before** playing, then `for s in &session.streams { s.play().map_err(..BuildStream..)?; }`. If one fails, `Drop` stops the others and joins the worker.

### R6 — App pure helpers
```rust
// app.rs
pub fn window_title() -> String { format!("VoiceChanger {}", env!("CARGO_PKG_VERSION")) }

/// Banner text for a stopped session. Role names: Input -> "input", Cable -> "cable output", Monitor -> "monitor".
/// DeviceLost: Audio stopped: the input device "USB Mic" was disconnected. Pick another device in Settings and press Start.
/// Failed:     Audio stopped: the cable output device "CABLE Input (VB-Audio Virtual Cable)" reported an error. Press Start to try again.
/// Running:    None
pub fn banner_text(status: StreamStatus, sel: &Selection) -> Option<String>;

// widgets.rs
/// Maps a linear peak to 0..=1 over -60..0 dBFS: ((gain_to_db(peak) + 60) / 60).clamp(0, 1).
pub fn level_to_fraction(peak: f32) -> f32;
#[derive(Clone, Copy, Debug, Default)]
pub struct MeterState { pub shown: f32 }
impl MeterState {
    /// Fast attack, ~300 ms release: shown = max(new_peak, shown * (-dt / 0.3).exp()).
    pub fn update(&mut self, new_peak: f32, dt: f32) -> f32;
    pub fn reset(&mut self);
}
/// Draws `label` + egui::ProgressBar(level_to_fraction(shown)) with text "-12.3 dB" ("-∞ dB" below -60).
pub fn meter(ui: &mut egui::Ui, label: &str, shown: f32);

// tabs/voice.rs
/// None -> "Stopped".
/// Some -> "Running · input 44.1 kHz · cable 48 kHz · monitor off" or "... · monitor 48 kHz".
/// kHz formatting: 48000 -> "48 kHz", 44100 -> "44.1 kHz" (one decimal, trailing ".0" dropped).
pub fn status_line(info: Option<SessionInfo>) -> String;
```

---

## Step 1 — App window with four tabs (eframe) and deny.toml licences
Files: `crates/app/Cargo.toml`, `crates/app/src/main.rs`, `crates/app/src/app.rs`, `crates/app/src/tabs/{mod,voice,models,convert,settings}.rs`, `deny.toml`, `Cargo.lock`.

1. Test first, in `app.rs` `#[cfg(test)] mod tests`:
   - `window_title_has_version`: `assert_eq!(window_title(), format!("VoiceChanger {}", env!("CARGO_PKG_VERSION")))`;
   - `tabs_in_order`: `assert_eq!(Tab::ALL.map(Tab::label), ["Voice", "Models", "Convert", "Settings"])`.

   Run `cargo test -p voicechanger`. It fails to compile because `app` doesn't exist.
2. `crates/app/Cargo.toml`: `[dependencies] eframe = "0.32"`.
3. `main.rs`:
   ```rust
   #![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
   //! VoiceChanger desktop app.
   mod app;
   mod tabs;
   use eframe::egui;
   fn main() -> eframe::Result {
       let options = eframe::NativeOptions {
           viewport: egui::ViewportBuilder::default()
               .with_title(app::window_title())
               .with_inner_size([720.0, 520.0])
               .with_min_inner_size([520.0, 400.0])
               // winit's OLE drag-and-drop puts the UI thread in STA, which breaks cpal's WASAPI COM init.
               .with_drag_and_drop(false),
           ..Default::default()
       };
       eframe::run_native(&app::window_title(), options, Box::new(|cc| Ok(Box::new(app::VcApp::new(cc)))))
   }
   ```
4. `app.rs`:
   - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum Tab { #[default] Voice, Models, Convert, Settings }`;
   - `Tab::ALL: [Tab; 4]` and `fn label(self) -> &'static str`;
   - `pub struct VcApp { tab: Tab }` with `new(_cc: &eframe::CreationContext<'_>)`;
   - `impl eframe::App` → `update`: a `egui::TopBottomPanel::top("tabs")` with `ui.selectable_value(&mut self.tab, t, t.label())` for each tab, then a `CentralPanel` that dispatches to `tabs::<name>::show(ui)`.
5. `tabs/models.rs` shows `ui.heading("Models"); ui.label("Coming in M4.");`. `convert.rs` does the same with "Coming in M5.". `voice.rs` and `settings.rs` temporarily show the heading plus "Coming soon." (replaced in steps 7–8).
6. Run `cargo test -p voicechanger` (passes), then `cargo deny check`. For each licence failure from eframe's dependency tree (expected: `OFL-1.1` and `LicenseRef-UFL-1.0` for egui's default fonts; possibly `BSL-1.0`, `CC0-1.0` or `MPL-2.0` from windows-side crates), add exactly that SPDX id to `allow` in `deny.toml`, each with a trailing comment naming the crate. Example: `"OFL-1.1", # epaint_default_fonts (egui bundled fonts)`. If an advisory (e.g. "unmaintained") fires for a transitive crate with no fixed version, add `ignore = [{ id = "RUSTSEC-…", reason = "transitive via eframe, no fixed release" }]` under `[advisories]`. Write down every addition for the PR description.
7. Run the full gate: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo build --release`. Run `cargo run -p voicechanger` and check that the window opens and the four tabs switch.
8. Commit `feat(app): add eframe window with Voice, Models, Convert and Settings tabs`.

## Step 2 — Device list, selection and feedback warnings (vc-audio)
Files: `crates/audio/Cargo.toml` (`cpal = "0.16"`), `crates/audio/src/lib.rs` (`pub mod devices;`), `crates/audio/src/devices.rs`, `deny.toml` if needed.

1. Tests first in `devices.rs` (`#[cfg(test)]`), with a helper `fn list(inputs: &[&str], outputs: &[&str], di: Option<&str>, dout: Option<&str>) -> DeviceList` that calls `from_names`:
   - `detects_virtual_cables`: true for `"CABLE Input (VB-Audio Virtual Cable)"`, `"CABLE Output (VB-Audio Virtual Cable)"`, `"VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)"`, `"Line 1 (Virtual Audio Cable)"` and lowercase `"cable input"`; false for `"Speakers (Realtek(R) Audio)"` and `"Headphones (USB Audio)"`.
   - `outputs_sorted_cables_first_stable`: `["Speakers", "CABLE Input (VB-Audio Virtual Cable)", "Headphones", "VoiceMeeter Input"]` → `["CABLE Input (VB-Audio Virtual Cable)", "VoiceMeeter Input", "Speakers", "Headphones"]`.
   - `default_cable_prefers_first_cable`, `default_cable_falls_back_to_windows_default`, `default_cable_falls_back_to_first_output`, `default_cable_none_when_no_outputs`.
   - `default_input_uses_windows_default_or_first`.
   - `selection_defaults`: the monitor is the default output when it differs from the cable, and `None` when the default output *is* the cable. `monitor_enabled` is false.
   - `reconcile_keeps_valid_selection` returns no notes. `reconcile_falls_back_input_and_cable` gives the exact notes from R1. `reconcile_drops_missing_monitor` sets the monitor to `None`, `monitor_enabled` to false and gives one note. `reconcile_without_default` sets the field to `""`.
   - `feedback_warnings_cases`:
     - none for mic + cable + headphones monitor;
     - `MonitorSameAsCable` when the monitor equals the cable and is enabled, and not when it is disabled;
     - `InputIsCableRecordingSide` for input `"CABLE Output (VB-Audio Virtual Cable)"` and for `"VoiceMeeter Output"`;
     - both at once, in the order `[MonitorSameAsCable, InputIsCableRecordingSide]`.
   - `start_blocker_cases`: each of the four messages in R1, and `None` for a valid selection.

   Run `cargo test -p vc-audio devices`. It fails to compile.
2. Implement R1 exactly. Doc comments on public items.
3. Run `cargo test -p vc-audio devices` (passes) and `cargo deny check`. Handle cpal/windows licences the same way as step 1.6. `list_devices()` isn't unit-tested (hardware). Instead, add `#[test] fn list_devices_does_not_panic() { let _ = list_devices(); }`, which works on a CI runner with no audio devices.
4. Run the full gate (step 1.7 commands). Commit `feat(audio): add WASAPI device listing, default selection and feedback warnings`.

## Step 3 — Level meters and shared state
Files: `crates/audio/src/meter.rs`, `crates/audio/src/shared.rs`, `lib.rs` (`pub mod meter; pub mod shared;`).

1. Tests first:
   - `meter.rs`:
     - `measure_empty_is_zero`;
     - `measure_dc` (`[0.5; 100]` → peak 0.5, rms 0.5);
     - `measure_sine` (1 kHz at amplitude 0.8 over 48 000 samples → peak ≈ 0.8 ± 1e-3, rms ≈ 0.8/√2 ± 1e-3);
     - `measure_uses_absolute_peak` (`[0.1, -0.9]` → 0.9);
     - `level_meter_round_trip` (store then load gives equal values, reset gives zero);
     - `db_gain_conversions` (`db_to_gain(0)=1`, `db_to_gain(-6)≈0.501`, `gain_to_db(1)=0`, `gain_to_db(0)=-120`).
   - `shared.rs`:
     - `status_codes_round_trip` for all 7 variants;
     - `first_error_wins` (report DeviceLost Input, then Failed Cable → status stays DeviceLost Input);
     - `reset_clears_status_meters_counters_keeps_gain`;
     - `default_monitor_gain_is_minus_6_db`.

   Run `cargo test -p vc-audio meter shared`. It fails to compile.
2. Implement R2. Use only `Ordering::Relaxed`. No locks.
3. Run the tests (they pass) and the full gate. Commit `feat(audio): add lock-free level meters and shared session state`.

## Step 4 — Ring buffers and callback bodies
Files: `crates/audio/Cargo.toml` (`ringbuf = "0.4"`), `crates/audio/src/ring.rs`, `lib.rs` (`pub mod ring;`).

1. Tests first in `ring.rs`:
   - `push_input_converts_i16` (`[i16::MAX, 0, i16::MIN]` → popped ≈ `[1.0, 0.0, -1.0]` ± 1e-3);
   - `push_input_converts_u16` (`[32768u16]` → ≈ 0.0);
   - `push_input_overflow_drops_whole_buffer` (ring of 4 with 3 already pushed; pushing 2 adds nothing, `overflows == 2`, `occupied_len == 3`);
   - `fill_output_duplicates_to_channels` (ring `[0.5, -0.5]`, stereo f32 buffer of 4 → `[0.5, 0.5, -0.5, -0.5]`, `underruns == 0`);
   - `fill_output_underrun_fills_silence` (ring `[0.25]`, stereo buffer of 6 → `[0.25, 0.25, 0, 0, 0, 0]`, `underruns == 2`);
   - `fill_output_i16` (ring `[1.0]` → `i16::MAX`);
   - `push_output_respects_backlog` (max_backlog 4, pushing 3 then 3 → second block dropped, `overflows == 3`, `occupied_len == 3`).

   Run `cargo test -p vc-audio ring`. It fails to compile.
2. Implement R3. Callbacks use only `try_push`/`try_pop`/`vacant_len`/`occupied_len` and atomics: no `Vec`, no `format!`, no locks.
3. Run the tests (they pass), `cargo deny check` and the full gate. Commit `feat(audio): add lock-free rings and audio callback bodies`.

## Step 5 — Processing worker (downmix, resample, gain, meters)
Files: `crates/audio/Cargo.toml` (`rubato = "0.16"`), `crates/audio/src/worker.rs`, `lib.rs` (`pub mod worker;`).

1. Tests first in `worker.rs`:
   - `downmix_stereo_averages` (`[1.0, 0.0, 0.5, 0.5]`, 2 ch → `[0.5, 0.5]`);
   - `downmix_mono_is_copy`;
   - `downmix_ignores_partial_frame`;
   - `same_rate_passthrough` (48 k mono in, cable 48 k: `output(0)` equals the input block and the input meter peak equals the block peak);
   - `resample_44k1_to_48k_length_and_sine`: feed 1 s of a 1 kHz sine (amplitude 0.5, 44.1 kHz, stereo with identical channels) in 441-frame blocks and collect `output(0)` after each call. Assert that the total length is within `48000 ± 2 * 480` samples. Skipping the first 4800 output samples (resampler delay), the count of positive-going zero crossings divided by the remaining seconds is 1000 ± 20 Hz, and the peak is 0.5 ± 0.05.
   - `monitor_gain_applied`: outputs `[Cable 48k, Monitor 48k]`, `shared.set_monitor_gain(0.5)`, input `[0.8; 480]` → `output(0)` all 0.8 and `output(1)` all 0.4. The output meter peak is 0.8 (from the cable).
   - `two_output_rates`: cable 48 k and monitor 44.1 k from a 44.1 k input. After 1 s, the lengths are ≈ 48000 and exactly 44100.
   - `worker_thread_moves_samples_and_stops`: build rings with `ring()`, push 4800 samples, `spawn`, poll the output consumer for up to 1 s until ≥ 4800 samples arrive, set `stop`, `join()` (must return), and check that the samples equal the input.

   Run `cargo test -p vc-audio worker`. It fails to compile.
2. Implement R4. Allocate every buffer in `State::new` / before the loop in `spawn`. `process_block` may only grow a Vec if a caller passes a block larger than 1 s (tests only). The `.expect` in `process_block` is acceptable because buffer sizes come from the resampler itself. Add a one-line comment saying so.
3. Run the tests (they pass), `cargo deny check` and the full gate. Commit `feat(audio): add processing worker with downmix, resampling and meters`.

## Step 6 — AudioSession: cpal streams + worker lifecycle
Files: `crates/audio/Cargo.toml` (`thiserror = "2"`), `crates/audio/src/session.rs`, `lib.rs`.

1. Tests first in `session.rs`:
   - `status_for_device_not_available` → `DeviceLost { which: Endpoint::Monitor }`;
   - `status_for_backend_error` (`cpal::StreamError::BackendSpecific { err: cpal::BackendSpecificError { description: "x".into() } }`) → `Failed { which: Endpoint::Cable }`;
   - `start_unknown_input_is_device_not_found`: `AudioSession::start(&Selection { input: "No Such Device 7f3a".into(), cable: "No Such Cable".into(), monitor: None, monitor_enabled: false }, Arc::new(Shared::new()))` → `Err(StartError::DeviceNotFound(n))` with `n == "No Such Device 7f3a"`. This runs without hardware;
   - `start_error_messages` (Display of each variant matches R5).

   Run `cargo test -p vc-audio session`. It fails to compile.
2. Implement R5 and its 9-step `start` sequence. Generic helpers: `fn build_input<T: cpal::SizedSample>(dev, cfg, prod, shared) -> Result<cpal::Stream, cpal::BuildStreamError> where f32: cpal::FromSample<T>` and `fn build_output<T: cpal::SizedSample + cpal::FromSample<f32>>(dev, cfg, cons, shared, which)`. Each callback closure owns its producer/consumer and an `Arc<Shared>` clone. Nothing else is captured.
3. `lib.rs` re-exports the app-facing API: `pub use devices::{DeviceList, Selection, Warning, feedback_warnings, is_virtual_cable, list_devices, start_blocker}; pub use meter::{Level, db_to_gain, gain_to_db}; pub use session::{AudioSession, SessionInfo, StartError}; pub use shared::{Endpoint, Shared, StreamStatus, DEFAULT_MONITOR_DB};`.
4. Run the tests (they pass) and the full gate. Commit `feat(audio): add AudioSession with WASAPI streams and clean shutdown`.

## Step 7 — Settings tab: devices, monitor, Refresh, warnings
Files: `crates/app/Cargo.toml` (`vc-audio = { path = "../audio" }`), `app.rs`, `tabs/settings.rs`.

1. Tests first in `app.rs`: `vc_app_state_defaults`. Build the state with `AppState::new(list)`, where `list` is a fake `DeviceList` that is not enumerated, so the test runs without hardware. Check that the selection equals `Selection::defaults(&list)`, `monitor_db == DEFAULT_MONITOR_DB` and there are no notes. `refresh_with` applies a new list and stores the reconcile notes. Run `cargo test -p voicechanger`. It fails to compile.
2. Split the state for testability. Put `pub struct AppState { pub devices: DeviceList, pub selection: Selection, pub monitor_db: f32, pub notes: Vec<String> }` in `app.rs` with:
   - `new(devices) -> Self`;
   - `refresh_with(&mut self, devices: DeviceList)`: sets `devices`, then `notes = selection.reconcile(&devices)`.

   `VcApp { tab, state: AppState }`. `VcApp::new` uses `AppState::new(list_devices())`. A `refresh_devices()` method calls `state.refresh_with(list_devices())`.
3. `tabs/settings.rs` has `pub fn show(ui: &mut egui::Ui, app: &mut VcApp)`. App fields are `pub(crate)`. It contains:
   - heading "Audio devices" and an `egui::Grid::new("devices")`;
   - **Input**: `egui::ComboBox::from_id_salt("input")` with `selected_text` set to the input or "(none)", and one `selectable_value` per `devices.inputs`;
   - **Cable output**: `ComboBox` "cable" over `devices.outputs`. The label is `format!("★ {name} (recommended)")` when `is_virtual_cable(name)`, otherwise the name;
   - **Monitor**: `ui.checkbox(&mut sel.monitor_enabled, "Monitor (hear yourself)")`, then inside `ui.add_enabled_ui(sel.monitor_enabled, …)` a `ComboBox` "monitor" over the outputs (`selectable_value(&mut sel.monitor, Some(name.clone()), name)`) and `egui::Slider::new(&mut monitor_db, -40.0..=0.0).suffix(" dB").text("Volume")`;
   - a `Refresh` button that calls `app.refresh_devices()`;
   - if `!devices.has_cable()`: a warning label in `ui.visuals().warn_fg_color`: "No virtual audio cable found. Install VB-CABLE so other apps can use VoiceChanger as a microphone. You can still pick any output to test." Then `if ui.link("Download VB-CABLE (free)").clicked() { ui.ctx().open_url(egui::OpenUrl::new_tab("https://vb-audio.com/Cable/")) }`;
   - each `feedback_warnings(&sel)` as `⚠ {message}` in `warn_fg_color`;
   - each note as a plain label;
   - `start_blocker` (if any) in `error_fg_color`.
4. Run the tests (they pass), the full gate, and `cargo run -p voicechanger`. Check that the drop-downs list real devices, that cables come first with ★, and that the warnings appear when you pick `CABLE Output` as input or the cable as monitor.
5. Commit `feat(app): add Settings tab with device selection, monitor and feedback warnings`.

## Step 8 — Voice tab, session lifecycle, meters, unplug handling, changelog
Files: `app.rs`, `widgets.rs` (new, add `mod widgets;` in `main.rs`), `tabs/voice.rs`, `tabs/settings.rs`, `CHANGELOG.md`.

1. Tests first:
   - `app.rs`: `banner_text_device_lost_input`, `banner_text_failed_cable` and `banner_text_running_none`, with the exact strings in R6;
   - `widgets.rs`:
     - `fraction_bounds` (0 → 0, 1 → 1, `db_to_gain(-30)` → 0.5 ± 1e-3);
     - `meter_attack_is_instant` (update(0.8, 0.016) → 0.8);
     - `meter_decays` (after 0.8, update(0, 0.3) ≈ 0.8·e⁻¹ ± 1e-3);
   - `tabs/voice.rs`: `status_line_stopped`, `status_line_running_monitor_off` (44100/48000/None → `"Running · input 44.1 kHz · cable 48 kHz · monitor off"`) and `status_line_running_monitor_on`.

   Run `cargo test -p voicechanger`. It fails to compile.
2. Implement R6. `VcApp` gains `session: Option<AudioSession>`, `shared: Arc<Shared>` (created once in `new`), `banner: Option<String>`, `input_meter: MeterState` and `output_meter: MeterState`. Methods:
   - `start()`: clear the banner, then `AudioSession::start(&state.selection, shared.clone())`. `Ok` → `session = Some`. `Err(e)` → `banner = Some(format!("Could not start audio: {e}"))`.
   - `stop()`: `session = None`, then reset both meters.
   - `poll_status()`: if running and `banner_text(shared.status(), &selection)` is `Some(t)`, then `stop()`, `banner = Some(t)` and `refresh_devices()`.
3. `update(ctx, _)` order:
   1. `poll_status()`;
   2. `shared.set_monitor_gain(db_to_gain(state.monitor_db))`;
   3. if running, update the meters from `shared.input_meter.load().peak` / `output_meter` with `dt = ctx.input(|i| i.stable_dt)`;
   4. `let before = state.selection.clone()`, then draw the tabs. After drawing, if running and `state.selection != before`, `stop(); start();` (a volume change is not a selection change, so it doesn't restart);
   5. if running, `ctx.request_repaint_after(Duration::from_millis(33))`.
4. `tabs/voice.rs` `show(ui, app)`:
   - the banner in an `egui::Frame::group` with its text in `error_fg_color` and a `Dismiss` button that clears it;
   - a Start/Stop button. Start uses `ui.add_enabled(start_blocker(..).is_none(), egui::Button::new("▶ Start"))` and shows the blocker text below it. Stop is `"■ Stop"`;
   - `widgets::meter(ui, "Input", ..)` and `widgets::meter(ui, "Output", ..)`;
   - `ui.label(status_line(session.as_ref().map(AudioSession::info)))`.
5. In `settings.rs`, next to the Input row while running, add `widgets::meter(ui, "Level", app.input_meter.shown)`.
6. In `CHANGELOG.md`, under `## [Unreleased]` → `### Added`, add: `- Audio skeleton: Voice / Models / Convert / Settings tabs, microphone, virtual cable and monitor selection, mic passthrough with live level meters, and a clean stop with a message when a device is unplugged.`
7. Run the tests (they pass), the full gate, and the manual checklist under Final verification. Commit `feat(app): add Voice tab with Start/Stop, live meters and device-loss handling`.

---

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo build --release
```
Manual checklist (copy into the PR, Windows + VB-CABLE):
- [ ] The window title is `VoiceChanger 0.1.0`. The four tabs switch. Models and Convert show the "Coming in M4/M5" text.
- [ ] Settings lists `CABLE Input (VB-Audio Virtual Cable)` first with ★. Without VB-CABLE, the warning and link appear, and the link opens vb-audio.com only on click.
- [ ] Start → your voice is heard through CABLE Output (Windows Sound → Recording → CABLE Output → Listen, or Discord's mic test).
- [ ] The monitor in headphones works. The volume slider changes loudness without a restart. Unchecking the monitor restarts without it.
- [ ] The Input/Output meters (Voice) and the input level (Settings) move and decay.
- [ ] A USB mic plugged in after launch appears after Refresh.
- [ ] Unplugging the USB mic mid-run shows the "input device … was disconnected" banner, with no crash, and Start works again with another mic. The same applies to unplugging USB headphones used as the monitor.
- [ ] The input `CABLE Output` warning and the monitor = cable warning each appear in the right case and don't block Start.
- [ ] The PR description lists every `deny.toml` licence/advisory addition with its reason.
