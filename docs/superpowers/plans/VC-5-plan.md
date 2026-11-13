# VC-5 — M3: Real-time streaming — implementation plan

## Goal
The user picks an RVC v2 `.onnx` model on the Voice tab and presses Start. Their converted voice then streams into the virtual cable (and the monitor), with live pitch, gains and gate, Performance settings (provider, block, context, crossfade) and a status bar showing measured latency, inference time and the provider.

## Architecture
`vc_engine::stream` (no cpal, no vc-audio) holds `StreamSettings`, `Controls`, `StreamStats`, `Sola` and `Streamer<C: Convert>`. `Streamer` resamples the mic to 16 kHz, keeps a past-context history, runs `Pipeline::convert_past` once per block and SOLA-crossfades the result at the model rate. `vc-audio`'s worker gets a `Processor` hook (`Any + Send`): downmix → input gain → meter → `process` → output gain → resample per output from the processor's rate → primed, slack-limited rings. The worker also computes end-to-end latency. The `voicechanger` app owns a `ModelSlot<Pipeline>` loaded on a `vc-model-loader` thread. It wraps the pipeline in `EngineProcessor` (a `Streamer` adapter) at Start, and gets it back from `AudioSession::stop()` by downcasting.

## Design gap closed with the human
The design's `Processor` trait had no way to learn the mic rate. **Answer: `prepare(&mut self, in_rate: u32)`.** The worker calls it with the input device rate before its loop, and `Streamer` builds its resampler there, then warms up. Read `delay()` only after `prepare`.

## Small decisions made in this plan (inside the design)
- `trait Processor: Any + Send`, so the app can recover its `EngineProcessor<Pipeline>` with `(p as Box<dyn Any>).downcast()`. Trait upcasting is stable on the repo's stable toolchain.
- `AudioSession::start` returns `Result<AudioSession, (StartError, Option<Box<dyn Processor>>)>`, so a failed start does not lose the loaded model. `None` happens only if the worker thread could not be spawned.
- The `ProcessorFailed` banner reason comes from an `ErrorSlot = Arc<Mutex<Option<String>>>` filled by the app adapter. A mutex is fine: it runs on the worker thread, not in a callback. Details go to `tracing`. The app installs `tracing-subscriber` (fmt to stderr, INFO) so "Details are in the log" holds.
- Output backlog limit: a push of `n` samples is allowed when `occupied + n ≤ n + slack`, with `slack = rate·(40 + 100) ms`. That is the design's "block + margin + 100 ms" for whatever block size arrives. Output rings get `2·rate` capacity.
- After `prepare`, the worker discards the input backlog that built up during the warm-up, so the first blocks are not counted as late.
- `rfd` dialogs run on a short-lived `vc-file-dialog` thread. rfd initialises COM, and an STA UI thread breaks cpal's WASAPI (see the `main.rs` drag-and-drop comment).
- The model warm-up in `Pipeline::new` uses `warmup_len = window_16k`, `warmup_context = 0`. The symmetric `convert` would reject `context ≥ window/2`, and only padding/shape matters for a warm-up.

## Repo facts (checked at 122f682)
- Workspace: edition 2024, stable toolchain, `[workspace.lints.clippy] all = warn`, and CI runs `clippy --all-targets --all-features -D warnings`. Windows only.
- `vc-audio`: `worker.rs` (`State`, `process_block`, `spawn`), `session.rs` (`AudioSession::start(sel, shared)`, `stop(self)`), `shared.rs` (`StreamStatus` codes 0–6, `Shared`), `ring.rs` (`push_input`, `fill_output`, `push_output`). Deps: cpal 0.16, ringbuf 0.5, rubato 0.16.
- `vc-engine`: `pipeline.rs` (`Pipeline::new/convert`, `trim_output(out, context, core, sr)`, `check_block`, `FRAME = 160`, `CONTENT_HOP = 320`), `dsp/resample.rs` (`StreamResampler`), `offline.rs`, `bin/vc-convert.rs` (every `--flag` takes a value), `tests/e2e.rs` (`#[ignore]`, `VC_TEST_MODEL`).
- `cpal::StreamInstant::new(secs, nanos)` and `duration_since` are public. `OutputCallbackInfo::timestamp()` gives `{callback, playback}`.
- egui 0.32 has `Response::drag_stopped()`. rfd 0.17 is MIT (use `default-features = false`; the defaults are Linux-only).
- Commits are conventional and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The PR targets `develop`.

**Gate** (run at the end of every step, in PowerShell):
`cargo fmt --all -- --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test --workspace`

## File map
| File | Responsibility |
|---|---|
| `crates/engine/src/pipeline.rs` (modify) | `generate` (shared internals), `convert_past`, `check_past`, `trim_output(out, skip, len, sr)` |
| `crates/engine/src/stream/mod.rs` (new) | module docs, re-exports |
| `crates/engine/src/stream/settings.rs` (new) | `StreamSettings`, ranges, 16 kHz sizes, `Controls` |
| `crates/engine/src/stream/stats.rs` (new) | `StreamStats` |
| `crates/engine/src/stream/sola.rs` (new) | `Sola` |
| `crates/engine/src/stream/streamer.rs` (new) | `Convert`, `impl Convert for Pipeline`, `Streamer` |
| `crates/engine/src/stream/bench.rs` (new) | `stream_file`, `StreamReport`, `percentile` |
| `crates/engine/src/bin/vc-convert.rs` (modify) | `--stream`, `--block-ms`, `--context-ms`, `--crossfade-ms` |
| `crates/engine/tests/e2e.rs` (modify) | `streams_voiced_signal` (ignored, assets) |
| `crates/audio/src/processor.rs` (new) | `Processor` trait, `Passthrough` |
| `crates/audio/src/shared.rs` (modify) | gains, latency atomics, `ProcessorFailed` (code 7) |
| `crates/audio/src/worker.rs` (modify) | processor in `State`, gains, failure, priming/slack, latency |
| `crates/audio/src/ring.rs` (modify) | `push_block`, `store_device_latency` |
| `crates/audio/src/session.rs` (modify) | start with processor, stop returns it, cable device latency |
| `crates/app/src/model.rs` (new) | `ModelInfo`, `ModelSlot<P>`, `provider_label`, `spawn_load`, `pick_model` |
| `crates/app/src/engine.rs` (new) | `ErrorSlot`, `EngineProcessor<C>` |
| `crates/app/src/status_bar.rs` (new) | `StatusSnapshot`, texts, thresholds, `show` |
| `crates/app/src/app.rs`, `tabs/voice.rs`, `tabs/settings.rs`, `main.rs`, `Cargo.toml` (modify) | wiring and UI |
| `CHANGELOG.md`, `README.md` (modify) | docs |

---

## Steps

### 1. engine: add `Pipeline::convert_past` with past-only context
Files: `crates/engine/src/pipeline.rs`.
- Rename `trim_output`'s parameters to `(out, skip, len, sr)`. Both are counts of 16 kHz input samples; `skip` is floored to whole 10 ms frames as today. Update the doc: "keeps `len` input samples' worth after skipping `skip`". Existing callers and tests keep working.
- Extract `fn generate(&mut self, block_16k: &[f32], params: &Params) -> Result<Vec<f32>>`: everything in `convert` from padding up to `generator.run`. It returns the full generator output.
- `convert` = `check_block` + `generate` + `trim_output(&out, context, len - 2*context, sr)` (behaviour unchanged).
- New `pub fn check_past(len, context) -> Result<()>`: `context % FRAME == 0`, otherwise `EngineError::Inference("context of {context} samples is not a multiple of 160")`; `len > context`, otherwise `Inference("window of {len} samples leaves no audio after {context} samples of context")`.
- New `pub fn convert_past(&mut self, window_16k: &[f32], context: usize, params: &Params) -> Result<Vec<f32>>` = `check_past` + `generate` + `trim_output(&out, context, window.len() - context, sr)`. Doc: model-rate audio for `window[context..]`, using only past context.

Tests first (module `tests`):
- `check_past_rules`: `check_past(9600, 100)` is Err, `check_past(8000, 8000)` is Err, `check_past(8001, 8000)` is Ok, `check_past(320, 0)` is Ok.
- `trim_output_past_keeps_tail`: `out` = 60·400 ramp values at 40 kHz, `trim_output(&out, 8000, 1600, 40000)` has length 4000 and `got[0] == (50*400) as f32`.

Run `cargo test -p vc-engine pipeline` (it fails to compile), implement, re-run (it passes), then the gate.
Commit: `feat(engine): add Pipeline::convert_past with past-only context`.

### 2. engine: stream settings, live controls and stats
Create `crates/engine/src/stream/{mod.rs,settings.rs,stats.rs}` and add `pub mod stream;` to `lib.rs` plus `pub use stream::{Controls, StreamSettings, StreamStats};`. Contracts: **Appendix A**.
Tests first:
- settings: `defaults_are_prd` (250/500/50); `clamps_and_rounds` (`{block 10, context 5000, crossfade 999}` → `{50, 2000, 200}`; block 255 → 260); `sizes_at_16k` (defaults: block 4000, crossfade 800, search 160, context 8000 or 8160 with window % 320 == 0); `window_is_aligned_for_all` (loop block 50..=1000 step 10 × context 0..=2000 step 10 × crossfade {10, 50, 200}: `window_16k() % 320 == 0`, `context_16k() % 160 == 0`, `context_ms*16 ≤ context_16k() < context_ms*16 + 320`).
- controls: `controls_defaults_and_clamp` (pitch 0, gate −50; set 30 → 24, −30 → −24; gate −100 → −90, 0 → −20; `params()` carries both and `speaker_id 0`).
- stats: `first_block_sets_ratio` (block 250 ms, record 100 ms → ratio 0.4, blocks 1, inference 100 ms); `ratio_is_smoothed` (then record 250 ms → ratio = 0.4 + (1/12)(1.0 − 0.4) ± 1e-4); `late_and_reset`.
Run `cargo test -p vc-engine stream`, then the gate. Commit: `feat(engine): add stream settings, live controls and stats`.

### 3. engine: SOLA crossfade
Create `crates/engine/src/stream/sola.rs` (`pub mod sola;` in `stream/mod.rs`, re-export `Sola`). Contract: **Appendix B**.
Tests first (40 kHz-like sizes: block 10000, crossfade 2000, search 400):
- `finds_known_offset`: build a 1 kHz sine `s` (40 kHz). Prime with `conv1 = s[0..12400]`; the tail kept is `s[10000..12000]` (k = 0, since prev was zero). Then `conv2 = s[10000 - 137 .. 10000 - 137 + 12400]`; `best_offset(&conv2) == 137`.
- `joins_sine_without_step`: feed consecutive windows of a continuous 220 Hz sine, where each window starts `block` after the previous one minus the search slack, the way the streamer slides. Over 5 blocks, `max |out[i+1] − out[i]|` stays ≤ 1.5 × the sine's own max per-sample step.
- `emits_exactly_block`: 5 calls give `out.len() == 5 * 10000`.
- `first_block_fades_in_from_silence`: `out[0].abs() < 1e-3` for a DC input of 0.5.
Run `cargo test -p vc-engine sola`, then the gate. Commit: `feat(engine): add SOLA crossfade for streaming`.

### 4. engine: `Streamer` and the `Convert` trait
Create `crates/engine/src/stream/streamer.rs` (re-export `Convert` and `Streamer` from `stream` and the crate root). It implements `Convert for Pipeline` (delegating to `Pipeline::sample_rate` and `Pipeline::convert_past`). Contract and algorithm: **Appendix C**.
Tests first, with `FakeConvert { rate: 40000, calls: Vec<(usize, usize, Params)> }`. Its `convert_past` returns `(w.len()-ctx)*5/2` samples with `out[i] = w[ctx + i*2/5]`.
- `output_length_and_rate`: prepare(16000), 1 s of a sine in 320-sample chunks → `out.len() == 4 * 10000`, `sample_rate() == 40000`, every fake call has `window == window_16k()` and `ctx == context_16k()`.
- `delay_at_16k_is_crossfade_search`: `delay() == 60 ms` (defaults).
- `delay_includes_input_resampler`: prepare(48000) → `delay() > 60 ms`.
- `delay_aligns_output_to_input`: after one ramp block from silence, output sample j holds input j·2/5 − delay·16000 (past the fade-in).
- `drops_late_block`: one `process` with `2*block` samples → `out.len() == 10000`, `stats.late_blocks() == 1`, `stats.blocks() == 1`.
- `reads_controls_per_block`: `controls.set_pitch(5.0); set_gate_db(-40.0)` before the block → the fake saw pitch 5, gate −40.
- `prepare_warms_up_and_resets`: after prepare, the fake has 1 call (the warm-up) and `stats.blocks() == 0`.
- `into_inner_returns_converter`.
Run `cargo test -p vc-engine streamer`, then the gate. Commit: `feat(engine): add Streamer for block-wise real-time conversion`.

### 5. engine: streaming benchmark in `vc-convert` and the asset e2e test
- `crates/engine/src/stream/bench.rs`: see **Appendix D**.
- `vc-convert.rs`: `--stream` is a flag without a value. `--block-ms`, `--context-ms` and `--crossfade-ms` take numbers (`u32`). `parse_args` returns `Cli { input, output, opts, stream: Option<StreamSettings> }`. The `stream` field is `Some(settings.clamped())` only with `--stream`. In stream mode, print `provider`, `warm-up`, `block B ms`, `inference p50 X ms · p95 Y ms · max Z ms`, `p95 / block P%`. Update `USAGE`.
- `tests/e2e.rs`: add `#[ignore = same text] fn streams_voiced_signal()`. It loads `Pipeline::new` on CPU with `warmup_len = window_16k`, `warmup_context = 0`, builds a `Streamer` with defaults, calls `prepare(16000)` and feeds `voiced_signal()` in 320-sample chunks. Assert `out.len() == 12 * (4000 * sr / 16000)`, all values finite, and RMS of the second half `> 1e-3`.
Tests first: `percentile_nearest_rank` (bench.rs; `[1..=10] ms`: p50 = 5 ms, p95 = 10 ms, p100 = 10 ms, empty → 0); CLI `parses_stream_flags` (`--stream --block-ms 120` → stream block 120, others default), `stream_absent_by_default`, `stream_flag_takes_no_value` (`--stream in.wav out.wav --model m.onnx` parses both paths). Keep the existing CLI tests (adapt them to `Cli`).
Run `cargo test -p vc-engine`, then the gate. With a model, also run `cargo test -p vc-engine --test e2e -- --ignored`.
Commit: `feat(engine): add vc-convert --stream benchmark and streaming e2e test`.

### 6. audio: gains, latency atomics and `ProcessorFailed` status
`crates/audio/src/shared.rs`:
- `StreamStatus::ProcessorFailed` is code 7.
- New fields:
  - `input_gain` and `output_gain`: `AtomicU32` f32 bits, default 1.0, with `input_gain()`, `set_input_gain()`, `output_gain()` and `set_output_gain()`. Kept by `reset`.
  - `pub latency_us: AtomicU64` and `pub device_latency_us: AtomicU64`. Both are zeroed by `reset`.
Update the `ALL` array to 8 entries.

App: `banner_text(status, sel, processor_error: Option<&str>)` gets the arm `ProcessorFailed => "Voice conversion stopped: {short_reason(processor_error)}. Details are in the log. Press Start to try again."`. Add `pub fn short_reason(e: Option<&str>) -> String`: the first line, trimmed, cut to 120 chars plus "…" if longer; `None` or empty gives "unknown error". `poll_status` passes `None` for now.

Tests first: `status_codes_round_trip` covers 8 entries; `gains_default_unity_and_survive_reset`; `reset_zeroes_latency`. App: `banner_text_processor_failed`, `short_reason_truncates_and_defaults`.
Gate. Commit: `feat(audio): add gains, latency stats and ProcessorFailed status`.

### 7. audio: `Processor` hook in the worker and session
See **Appendix E**.
- New `crates/audio/src/processor.rs` with `Processor` and `Passthrough`; re-export both from `lib.rs`.
- `worker.rs`: `WorkerConfig.proc_rate`. `State::new(cfg, processor)` resamples `proc_rate → out.rate`. `process_block` applies input gain → input meter → process (timed) → output gain → paths. `State::prepare`, `State::into_processor`, `State::last_process()`. `spawn` returns `JoinHandle<Box<dyn Processor>>`, runs prepare first, then skips the input backlog.
- `session.rs`: `start(sel, shared, processor) -> Result<AudioSession, (StartError, Option<Box<dyn Processor>>)>`. `stop(self) -> Option<Box<dyn Processor>>` uses a shared `shutdown()` with `Drop`. Keep `max_backlogs = rate/5` for now.
- Add `tracing = "0.1"` to `crates/audio/Cargo.toml`. On failure, log `tracing::error!(reason = %e, "voice processor failed")`.
- App (temporary until step 10): `start` passes `Box::new(Passthrough::new(48_000))` and maps `Err((e, _))`; `stop` drops the result.

Tests first (worker): adapt the existing tests to `proc_rate = in_rate` plus `Passthrough::new(in_rate)`; `processor_out_rate_is_resampled` (fake ×2 upsampler that declares `2*in_rate`, cable at `2*in_rate` → no resampler and 2× the samples); `gains_applied_and_metered` (input 0.5 → input gain 2.0 → input meter 1.0, output gain 0.5 → cable 0.5 and output meter 0.5); `processor_error_reports_and_stops_calling` (2 blocks → status `ProcessorFailed`, 1 call, empty outputs); `prepare_error_reports`; `worker_returns_processor` (join → downcast to the fake and check its call count); adapt `worker_thread_moves_samples_and_stops`. Session: `start_unknown_input_is_device_not_found` matches `Err((DeviceNotFound(name), Some(_)))`.
Gate. Commit: `feat(audio): add Processor hook to the worker and session`.

### 8. audio: block-sized backlog, ring priming and end-to-end latency
See **Appendix F**.
- `ring.rs`: `push_block` and `store_device_latency`.
- `worker.rs`: `PRIME_MS = 40` and `SLACK_MS = 100`. `OutputPath` gets `rate` and `primed`. `spawn` loses `max_backlogs`. `LatencyParts` and `end_to_end`. After each push where the cable got samples, store `latency_us`.
- `session.rs`: output rings get `2 * rate` capacity, `max_backlogs` is removed, and the cable output callback calls `store_device_latency(&info.timestamp(), &shared)`. Monitor callbacks do not.
Tests first: `push_block_primes_once` (first push = 40 ms of zeros + samples, second push = no zeros); `push_block_allows_full_block` (250 ms at 48 kHz into an empty ring is accepted; a second push while 200 ms is still queued is dropped and counted in `overflows`); `device_latency_from_timestamps` (callback (1, 0), playback (1, 25_000_000) → 25 000 µs; playback earlier → unchanged); `end_to_end_sums_parts` (4800 frames at 48 kHz + 310 ms + 120 ms + 2400 at 48 kHz + 20 ms = 600 ms); `worker_sets_latency` (thread test with Passthrough: `latency_us > 0` after output).
Gate. Commit: `feat(audio): size output backlog per block, prime rings and measure latency`.

### 9. app: model picker and background loading
See **Appendix G**.
- `crates/app/Cargo.toml`: add `vc-engine = { path = "../engine" }` and `rfd = { version = "0.17", default-features = false }`.
- New `crates/app/src/model.rs` (`mod model;` in `main.rs`).
- `AppState.perf: PerfSettings` (provider choice, gpu, stream settings; defaults DirectML 0 / PRD).
- `VcApp` gets `model: ModelSlot<Pipeline>`, `model_path: Option<PathBuf>` and `pick_rx: Option<Receiver<Option<PathBuf>>>`. Each frame: poll `pick_rx` (Some(path) → `load_model(path)`), then `model.poll()` (Err → banner `"Could not load the model: {msg}"`). Repaint every 100 ms while loading or picking.
- Voice tab: a model row above Start: `ui.label(model.label())` and a `Model…` button, disabled while running, loading or picking. The button calls `pick_model()`. Start stays as in step 7 for now.
Tests first (`model.rs`): `slot_loading_to_ready`, `slot_loading_to_failed`, `slot_loader_disconnected_fails`, `take_and_give_back`, `lost_goes_empty`, `labels`, `provider_label_fallback`, `perf_provider_mapping`. These use `ModelSlot<u32>` with a hand-fed `mpsc` channel.
Gate, then a manual check: pick a model and watch "Loading…" turn into "name · 40 kHz · DirectML (GPU 0)". Commit: `feat(app): add model picker with background loading`.

### 10. app: stream through the engine with live voice controls
See **Appendix H**.
- New `crates/app/src/engine.rs` (`ErrorSlot`, `EngineProcessor<C: Convert + Send + 'static>`).
- `app.rs`: `controls: Arc<Controls>`, `stats: Arc<StreamStats>`, `processor_error: ErrorSlot` and `voice: VoiceSettings` (in `AppState`). `start` takes the pipeline from the slot, builds `Streamer` + `EngineProcessor` and calls `AudioSession::start`. On error it calls `recover`. `stop`/`poll_status` call `recover(session.stop())`. `poll_status` passes `processor_error` to `banner_text`. The update loop pushes the voice values to `controls`/`shared` every frame.
- Voice tab: Start enabled only when `model.is_ready()` and there is no device blocker; otherwise the label "Pick a model first". Sliders: Pitch (−24..=24 st), Input gain (−20..=20 dB), Output gain (−20..=20 dB), Noise gate (−90..=−20 dB, default −50).
- `main.rs`: init `tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).with_writer(std::io::stderr).init()`. Add `tracing = "0.1"` and `tracing-subscriber = { version = "0.3", default-features = false, features = ["fmt", "std"] }`.
Tests first (`engine.rs`, with a fake `Convert`): `processor_reports_rate_and_delay`, `process_error_fills_error_slot`, `prepare_error_fills_error_slot`, `downcast_round_trip` (`Box<dyn Processor>` → `EngineProcessor<Fake>` → `into_inner`); `app.rs`: `voice_defaults`.
Gate, `cargo deny check`, then manual: Start/Stop twice without reloading; pitch, gains and gate act live. Commit: `feat(app): stream voice conversion with live pitch, gain and gate`.

### 11. app: Performance settings with restart on release
See **Appendix I**. Settings tab, new "Performance" group under the devices:
- Provider ComboBox (DirectML/CUDA/CPU) and GPU `DragValue` 0..=7 (hidden for CPU). Both are disabled while running or loading; a change while stopped with `model_path` set reloads the model.
- Block (50..=1000), Extra context (0..=2000), Crossfade (10..=200) ms sliders, step 10, which update `perf.stream`. If running and a slider response has `drag_stopped() || (changed() && !dragged())`, set `restart_requested`.
- Warning while running when `stats.ratio() > 0.6`.
- `update`: after the panels, if running and `restart_requested`, call `stop()` then `start()` (the model is kept). Clear the flag.
Tests first: `perf_change_needs_restart` (pure `fn needs_restart(running, drag_stopped, changed, dragged) -> bool`); `inference_warning_text` (`Some` at 0.65 with "65%", `None` at 0.6).
Gate and manual check. Commit: `feat(app): add Performance settings with live restart`.

### 12. app: status bar
See **Appendix J**. Create `crates/app/src/status_bar.rs`; show `egui::TopBottomPanel::bottom("status")` while running, before `CentralPanel`.
Tests first: `status_text_full`, `latency_placeholder_before_first_block`, `thresholds`.
Gate and manual check. Commit: `feat(app): add latency and inference status bar`.

### 13. docs: changelog, README and PR notes
- `CHANGELOG.md` `[Unreleased]/### Added`: `- Real-time voice conversion: pick an RVC v2 ONNX model on the Voice tab, live pitch, input/output gain and noise gate, SOLA crossfaded blocks with Performance settings (provider, block, extra context, crossfade), a status bar with measured latency, inference time and provider, and \`vc-convert --stream\` for benchmarking.`
- `README.md`: next to the existing `vc-convert` line, add the `--stream [--block-ms N] [--context-ms N] [--crossfade-ms N]` usage and one sentence on what it prints.
- PR description: manual results (step list in Final verification) and the note that PRD §6 "≤ 300 ms with default settings" should read "at a block of ~100–120 ms"; the PRD is not edited here.
Gate. Commit: `docs: changelog and README for real-time streaming`.

---

## Appendix A — `stream/settings.rs` and `stream/stats.rs`
```rust
pub const BLOCK_MS: RangeInclusive<u32> = 50..=1000;
pub const CONTEXT_MS: RangeInclusive<u32> = 0..=2000;
pub const CROSSFADE_MS: RangeInclusive<u32> = 10..=200;
pub const SEARCH_MS: u32 = 10;
pub const PITCH_RANGE: RangeInclusive<f32> = -24.0..=24.0;
pub const GATE_DB_RANGE: RangeInclusive<f32> = -90.0..=-20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSettings { pub block_ms: u32, pub context_ms: u32, pub crossfade_ms: u32 }
impl Default for StreamSettings { /* 250, 500, 50 */ }
impl StreamSettings {
    /// Each value rounded to the nearest 10 ms ((ms+5)/10*10), then clamped to its range.
    pub fn clamped(self) -> Self;
    // All below use `self.clamped()` and are 16 kHz sample counts.
    pub fn block_16k(&self) -> usize;      // block_ms*16
    pub fn crossfade_16k(&self) -> usize;  // crossfade_ms*16
    pub fn search_16k(&self) -> usize;     // SEARCH_MS*16 = 160
    /// context_ms*16, plus 160 when needed so window_16k() % 320 == 0.
    pub fn context_16k(&self) -> usize;
    pub fn window_16k(&self) -> usize;     // context + crossfade + search + block
}

#[derive(Debug)]
pub struct Controls { pitch: AtomicU32, gate_db: AtomicU32 } // f32 bits
impl Controls {
    pub fn new() -> Self;            // pitch 0.0, gate -50.0 (Default too)
    pub fn set_pitch(&self, semitones: f32);  // clamped to PITCH_RANGE
    pub fn pitch(&self) -> f32;
    pub fn set_gate_db(&self, db: f32);       // clamped to GATE_DB_RANGE
    pub fn gate_db(&self) -> f32;
    pub fn params(&self) -> Params;  // { pitch_semitones, gate_db, speaker_id: 0 }
}

#[derive(Debug, Default)]
pub struct StreamStats { inference_us: AtomicU64, block_us: AtomicU64, ratio: AtomicU32, blocks: AtomicU64, late_blocks: AtomicU64 }
impl StreamStats {
    pub fn new() -> Self;
    pub fn reset(&self);                       // everything 0
    pub fn set_block(&self, block: Duration);
    /// blocks += 1, inference stored; r = inference/block; first block: ratio = r,
    /// else ratio += alpha*(r - ratio) with alpha = min(block_secs/3.0, 1.0).
    pub fn record(&self, inference: Duration);
    pub fn record_late(&self);
    pub fn inference(&self) -> Duration;
    pub fn block(&self) -> Duration;
    pub fn ratio(&self) -> f32;
    pub fn blocks(&self) -> u64;
    pub fn late_blocks(&self) -> u64;
}
```

## Appendix B — `Sola` (model-rate samples)
```rust
pub struct Sola { block: usize, crossfade: usize, search: usize, prev: Vec<f32>, fade_in: Vec<f32> }
impl Sola {
    pub fn new(block: usize, crossfade: usize, search: usize) -> Self; // prev = zeros(crossfade); fade_in[i] = sin²(π/2 · i/(crossfade-1)) (1.0 if crossfade == 1)
    pub fn input_len(&self) -> usize;    // crossfade + search + block
    /// k in 0..search maximising Σ conv[k+i]·prev[i] / sqrt(Σ conv[k+i]² + 1e-8). Ties and an all-zero prev give 0.
    pub fn best_offset(&self, conv: &[f32]) -> usize;
    /// conv.len() must be input_len() (debug_assert). k = best_offset; appends conv[k..k+block] to `out`,
    /// with the first `crossfade` samples = conv[k+i]*fade_in[i] + prev[i]*(1-fade_in[i]);
    /// then prev = conv[k+block .. k+block+crossfade]. No allocation beyond `out` growth.
    pub fn process(&mut self, conv: &[f32], out: &mut Vec<f32>);
    pub fn reset(&mut self);             // prev zeroed
}
```

## Appendix C — `Convert` and `Streamer`
```rust
pub trait Convert {
    fn sample_rate(&self) -> u32;
    fn convert_past(&mut self, window: &[f32], context: usize, params: &Params) -> Result<Vec<f32>>;
}
impl Convert for Pipeline { /* Pipeline::sample_rate(self), Pipeline::convert_past(self, ..) */ }

pub struct Streamer<C: Convert> {
    inner: C, settings: StreamSettings /* clamped */, controls: Arc<Controls>, stats: Arc<StreamStats>,
    in_rate: u32, resampler: Option<StreamResampler>, resampled: Vec<f32>,
    fresh: Vec<f32>, history: Vec<f32> /* window_16k zeros */, sola: Sola,
}
impl<C: Convert> Streamer<C> {
    pub fn new(inner: C, settings: StreamSettings, controls: Arc<Controls>, stats: Arc<StreamStats>) -> Self;
    // Sola::new(m(block_16k), m(crossfade_16k), m(search_16k)) with m(n) = n * sr / 16000.
    pub fn settings(&self) -> StreamSettings;
    pub fn sample_rate(&self) -> u32;
    /// resampler = None if in_rate == 16000 else Some(StreamResampler::new(in_rate, 16000)?);
    /// clears fresh, zeroes history, sola.reset(), warm_up()?, stats.reset(), stats.set_block(block).
    pub fn prepare(&mut self, in_rate: u32) -> Result<()>;
    /// One convert_past on zeros(window_16k) with context_16k and Params::default(); output discarded.
    pub fn warm_up(&mut self) -> Result<()>;
    /// (block_16k + crossfade_16k + search_16k + resampler output_delay) / 16000 s.
    pub fn delay(&self) -> Duration;
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()>;
    pub fn into_inner(self) -> C;
}
```
`process`:
1. Resample `input` into `resampled`, or copy it at 16 kHz, and append it to `fresh`.
2. `while fresh.len() >= 2*block { fresh.drain(..block); stats.record_late(); }`.
3. If `fresh.len() >= block`, slide the history: `history.copy_within(block.., 0)`, copy `fresh[..block]` into its tail, then `fresh.drain(..block)`.
4. Read `params = controls.params()`, time `inner.convert_past(&history, context_16k, &params)?`, then `stats.record(elapsed)`.
5. If `conv.len() != sola.input_len()`, return `EngineError::Inference("converter returned N samples, expected M")`.
6. `sola.process(&conv, out)`.

At most one conversion runs per call.

## Appendix D — `stream/bench.rs`
```rust
pub struct StreamReport { pub active_provider: ActiveProvider, pub warmup: Duration, pub block: Duration, pub inference: Vec<Duration> /* sorted */ }
impl StreamReport { pub fn p50(&self)->Duration; pub fn p95(&self)->Duration; pub fn max(&self)->Duration; pub fn p95_ratio(&self)->f64 /* p95/block */ }
/// Nearest rank: sorted[ceil(p*n)-1] (clamped); Duration::ZERO when empty.
pub fn percentile(sorted: &[Duration], p: f64) -> Duration;
/// Reads the WAV, loads Pipeline (provider/sr_override from opts; warmup_len = window_16k, warmup_context = 0),
/// Controls from opts.params, Streamer::prepare(wav_rate), feeds wav_rate/50-sample chunks unpaced,
/// times each process() call whose output grew, drops the first round(delay·sr) output samples,
/// truncates/pads to len·sr/wav_rate, resamples to opts.out_rate.unwrap_or(wav_rate) and writes the WAV.
pub fn stream_file(input: &Path, output: &Path, opts: &ConvertOptions, settings: StreamSettings) -> Result<StreamReport>;
```
Re-export `stream_file` and `StreamReport` from `stream`.

## Appendix E — `Processor` and the worker
```rust
// crates/audio/src/processor.rs
pub trait Processor: Any + Send {
    /// Sample rate of the samples `process` appends.
    fn out_rate(&self) -> u32;
    /// Algorithmic delay (crossfade + search + resampler delays; the block is in the cable backlog). Valid after `prepare`.
    fn delay(&self) -> Duration;
    /// Called once on the worker thread before the first `process`, with the input device rate.
    fn prepare(&mut self, in_rate: u32) -> Result<(), String> { let _ = in_rate; Ok(()) }
    /// Appends converted mono samples (possibly none) for `input` (mono, device rate) to `out`.
    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String>;
}
/// Identity processor declaring `rate` (tests and bring-up).
pub struct Passthrough { rate: u32 }
impl Passthrough { pub fn new(rate: u32) -> Self }
// out_rate = rate, delay = ZERO, process = out.extend_from_slice(input)
```
Worker:
- `WorkerConfig { in_rate, in_channels, proc_rate, outputs }`.
- `State` fields: `in_rate, in_channels, mono, processed: Vec<f32> (cap 2*proc_rate), processor: Box<dyn Processor>, failed: bool, last_process: Duration, paths`.
- `State::new(cfg: &WorkerConfig, processor: Box<dyn Processor>) -> Result<State, (String, Box<dyn Processor>)>`. Resamplers are `FftFixedIn::new(proc_rate, out.rate, proc_rate/100, 1, 1)` and are `None` when `out.rate == proc_rate`. `pending` cap `2*proc_rate`, `ready` cap `2*out.rate`.
- `State::prepare(&mut self, shared: &Shared)`: on Err → `self.fail(shared, e)`.
- `fn fail(&mut self, shared, e)` sets `failed`, logs with `tracing::error!`, and calls `shared.report(StreamStatus::ProcessorFailed)`.
- `pub fn into_processor(self) -> Box<dyn Processor>` and `pub fn last_process(&self) -> Duration`.
- `process_block`: downmix; multiply by `shared.input_gain()`; input meter. Clear `processed`. If not failed, time `processor.process(&mono, &mut processed)` (Err → fail and clear `processed`). Multiply `processed` by `shared.output_gain()`. Each path is fed from `processed` (it was `mono`); monitor gain and the output meter work as today.
- `spawn(state, input, outputs, max_backlogs, shared, stop) -> io::Result<JoinHandle<Box<dyn Processor>>>`: `state.prepare(&shared)`, then `input.skip(input.occupied_len())`, then the existing loop, and the thread returns `state.into_processor()`.

Session:
- Order: open devices, `shared.reset()`, rings, `State::new` (Err((reason, p)) → `Err((UnsupportedConfig{..}, Some(p)))`), build streams (Err → `Err((e, Some(state.into_processor())))`), spawn (Err → `Err((build_error(..), None))`), play (Err → `let p = session.shutdown(); Err((e, p))`).
- `fn shutdown(&mut self) -> Option<Box<dyn Processor>>` pauses and drops the streams, sets `stop` and joins (`h.join().ok()`). `stop(mut self)` returns `self.shutdown()`; `Drop` calls `shutdown()`.

## Appendix F — rings, priming, latency
```rust
// ring.rs
/// On the first non-empty push (`*primed == false`) pushes `prime` zeros first and sets `primed`.
/// Then push_output(prod, samples, samples.len() + slack, shared).
pub fn push_block(prod: &mut HeapProd<f32>, samples: &[f32], primed: &mut bool, prime: usize, slack: usize, shared: &Shared);
/// Stores (playback − callback) in µs in shared.device_latency_us; leaves it unchanged when playback < callback.
pub fn store_device_latency(ts: &cpal::OutputStreamTimestamp, shared: &Shared);
// worker.rs
pub const PRIME_MS: u32 = 40;
pub const SLACK_MS: u32 = 100;
pub struct LatencyParts { pub input_backlog: usize /* frames */, pub in_rate: u32, pub delay: Duration,
    pub inference: Duration, pub cable_backlog: usize, pub cable_rate: u32, pub device: Duration }
pub fn end_to_end(p: &LatencyParts) -> Duration; // backlog/in_rate + delay + inference + cable_backlog/cable_rate + device
```
- Per path: `prime = rate*PRIME_MS/1000` and `slack = rate*(PRIME_MS+SLACK_MS)/1000`.
- After pushing, if the cable path's `ready` was non-empty, store `latency_us = end_to_end(..)` with these parts:
  - input backlog: `input.occupied_len()/ch`;
  - delay: `processor.delay()` + the cable resampler's `output_delay()/cable_rate` (0 when there is no resampler);
  - inference: `state.last_process()`;
  - cable backlog: the cable prod's `occupied_len()`;
  - device: `device_latency_us`.

  Add `State::cable_index()` and `State::path_delay(i) -> Duration`.

## Appendix G — `model.rs`
```rust
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo { pub name: String, pub sample_rate: u32, pub requested: Provider, pub active: Provider }
pub type LoadResult<P> = Result<(P, ModelInfo), String>;
pub enum ModelSlot<P> { Empty, Loading { name: String, rx: Receiver<LoadResult<P>> }, Ready(P, ModelInfo), InSession(ModelInfo), Failed(String) }
impl<P> ModelSlot<P> {
    pub fn loading(name: String, rx: Receiver<LoadResult<P>>) -> Self;
    /// Loading only: Ok msg → Ready, Some(Ok(())); Err(msg) → Failed, Some(Err(msg));
    /// Disconnected → Failed("the model loader stopped unexpectedly"); Empty → None.
    pub fn poll(&mut self) -> Option<Result<(), String>>;
    pub fn take(&mut self) -> Option<P>;         // Ready → InSession(info)
    pub fn give_back(&mut self, p: P);           // InSession(info) → Ready(p, info); otherwise drops p
    pub fn lost(&mut self);                      // InSession → Empty
    pub fn info(&self) -> Option<&ModelInfo>;    // Ready or InSession
    pub fn is_ready(&self) -> bool; pub fn is_loading(&self) -> bool;
    /// "No model", "Loading {name}…", "{name} · {khz} · {provider_label}", "Failed to load: {msg}"
    pub fn label(&self) -> String;
}
impl<P> Default for ModelSlot<P> { Empty }
/// "CPU (fallback)" when active == Cpu and requested != Cpu, else active.to_string().
pub fn provider_label(requested: Provider, active: Provider) -> String;
/// Thread "vc-model-loader": Pipeline::new(PipelineConfig { model: path, sr_override: None, provider,
/// warmup_len: settings.window_16k(), warmup_context: 0 }) → (pipeline, ModelInfo{ name: file stem, .. }), errors as Display text.
pub fn spawn_load(path: PathBuf, provider: Provider, settings: StreamSettings) -> std::io::Result<Receiver<LoadResult<Pipeline>>>;
/// Thread "vc-file-dialog": rfd::FileDialog::new().set_title("Choose an RVC v2 ONNX model")
/// .add_filter("RVC v2 ONNX model", &["onnx"]).pick_file(). A spawn failure yields a closed channel.
pub fn pick_model() -> Receiver<Option<PathBuf>>;
```
In `app.rs`:
```rust
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub enum ProviderChoice { #[default] DirectMl, Cuda, Cpu }
#[derive(Clone, Copy, Debug, Default, PartialEq)] pub struct PerfSettings { pub provider: ProviderChoice, pub gpu: i32, pub stream: StreamSettings }
impl PerfSettings { pub fn provider(&self) -> Provider }  // DirectMl{device: gpu}, Cuda{device: gpu}, Cpu
```
`VcApp::load_model(&mut self, path)`: store `model_path`; `model = ModelSlot::loading(stem, rx)`, or `Failed(e.to_string())` if the thread can't spawn. Make `voice::khz` `pub(crate)` and reuse it in `label`.

## Appendix H — `engine.rs` and app wiring
```rust
pub type ErrorSlot = Arc<Mutex<Option<String>>>;
pub struct EngineProcessor<C: Convert> { streamer: Streamer<C>, error: ErrorSlot }
impl<C: Convert> EngineProcessor<C> {
    pub fn new(streamer: Streamer<C>, error: ErrorSlot) -> Self;
    pub fn into_inner(self) -> C;   // streamer.into_inner()
}
impl<C: Convert + Send + 'static> vc_audio::Processor for EngineProcessor<C> {
    // out_rate = streamer.sample_rate(); delay = streamer.delay();
    // prepare/process: map Err(e) to e.to_string(), store a clone in `error` (lock().unwrap_or_else(PoisonError::into_inner)), return Err.
}
```
```rust
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceSettings { pub pitch: i32, pub input_db: f32, pub output_db: f32, pub gate_db: f32 } // default 0, 0, 0, -50
```
- `VcApp::recover(&mut self, p: Option<Box<dyn Processor>>)`: `p.and_then(|p| (p as Box<dyn Any>).downcast::<EngineProcessor<Pipeline>>().ok())` → `model.give_back(ep.into_inner())`, else `model.lost()`. When lost, also add the banner text "The model was unloaded; choose it again with Model…".
- Per frame: `controls.set_pitch(voice.pitch as f32)`, `controls.set_gate_db(voice.gate_db)`, `shared.set_input_gain(db_to_gain(voice.input_db))`, `shared.set_output_gain(db_to_gain(voice.output_db))`.
- Remove the temporary `Passthrough` use from step 7.

## Appendix I — Performance group
```rust
/// True when a running session must restart after this slider interaction.
pub fn needs_restart(running: bool, drag_stopped: bool, changed: bool, dragged: bool) -> bool {
    running && (drag_stopped || (changed && !dragged))
}
/// Shown while running when ratio > 0.6.
pub fn inference_warning(ratio: f32) -> Option<String>;
// "⚠ Inference takes {p}% of each block. Raise the block size or pick a faster provider to avoid dropouts."
```
Both go in `tabs/settings.rs`. `VcApp` gets `restart_requested: bool`. The provider/GPU change detection compares `perf.provider()` before and after the UI; if it changed, the app is not running and `model_path` is set, call `load_model`.

## Appendix J — status bar
```rust
pub struct StatusSnapshot { pub latency: Duration, pub inference: Duration, pub block: Duration, pub ratio: f32,
    pub provider: String, pub late_blocks: u64, pub underruns: u64 }
pub const LATENCY_WARN: Duration = Duration::from_millis(300);
pub const RATIO_WARN: f32 = 0.6;
pub fn latency_text(s: &StatusSnapshot) -> String;   // "Latency 480 ms", or "Latency –" when latency is zero
pub fn inference_text(s: &StatusSnapshot) -> String; // "Inference 140 ms / 250 ms (56%)"
pub fn status_text(s: &StatusSnapshot) -> String;    // latency · inference · provider · "late blocks N" · "underruns N"
pub fn latency_warn(s: &StatusSnapshot) -> bool;     // latency > LATENCY_WARN
pub fn ratio_warn(s: &StatusSnapshot) -> bool;       // ratio > RATIO_WARN
pub fn show(ui: &mut egui::Ui, s: &StatusSnapshot);  // horizontal labels joined by " · ", warn parts in ui.visuals().warn_fg_color
```
`VcApp::status_snapshot()` reads `shared.latency_us`, `stats`, `provider_label(info.requested, info.active)` and `shared.underruns`. Rounding: ms are rounded to whole numbers, and the percent is `(ratio*100).round()`.
Test `status_text_full` expects `"Latency 480 ms · Inference 140 ms / 250 ms (56%) · DirectML (GPU 0) · late blocks 0 · underruns 3"`. Test `thresholds`: 300 ms is not a warning but 301 ms is; a ratio of 0.6 is not a warning but 0.61 is.

---

## Final verification
1. `cargo fmt --all -- --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test --workspace; cargo deny check; cargo build --release`.
2. With a model: `$env:VC_TEST_MODEL='C:\path\model.onnx'; cargo test -p vc-engine --test e2e -- --ignored` (both e2e tests pass).
3. `cargo run -p vc-engine --bin vc-convert --release -- voice.wav out.wav --model m.onnx --stream` on an RTX 3060 and/or RX 6600: `p95 / block` < 60 % at the defaults. Then run it again with `--block-ms 120` and note p95.
4. App: pick a model ("Loading…" → ready), Start, speak, and hear converted audio on the cable and the monitor. Pitch, gains and gate act live. Changing block, context or crossfade restarts on release without reloading. The status bar shows latency (~450–500 ms at the defaults, ≤ 300 ms at a block of ~100–120 ms on the reference GPU), turning amber above 300 ms or 60 %.
5. Unplug the mic: the banner appears, and Start works again without reloading the model. Force a bad model, and the load-failure banner appears.
6. A 2-hour run: underruns and late blocks stay at 0. Record the results in the PR, with the PRD §6 wording note.
