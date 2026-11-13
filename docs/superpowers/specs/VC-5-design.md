# VC-5 — M3: Real-time streaming (blocks, SOLA crossfade, pitch shift, latency stats)

## Goal
The user picks an RVC v2 `.onnx` model on the Voice tab and presses Start. Their converted voice then plays into the virtual cable (and the monitor) in real time. They can change pitch, input/output gain and noise gate live. Block size, extra context, crossfade and execution provider live in Settings → Performance. A status bar shows measured end-to-end latency, inference time per block and the active provider. Mockup v1 shows the screens.

## Decisions
- **Model selection**: a "Model…" native file picker on the Voice tab (new dep `rfd`, MIT). The model loads on a background thread with a "Loading…" status, and Start is disabled until it is ready. The choice is not saved; M4 replaces the picker with the library. *Rejected:* a path text field, a CLI/env var only, and running Start without a model as passthrough.
- **Integration**: vc-audio's worker gets a `Processor` hook. The streaming logic lives in a new `vc_engine::stream` module, which does not depend on cpal or vc-audio. The app glues the two with a small adapter. Inference runs on the existing worker thread, which becomes the PRD's "inference worker". *Rejected:* a second inference thread (more ring hops and bookkeeping), and making vc-audio depend on vc-engine (ONNX Runtime in the audio crate).
- **Latency target**: the PRD defaults stay (block 250, context 500, crossfade 50 ms). The status bar shows the honestly measured latency, which is about 450–500 ms at the defaults. The ≤ 300 ms target is checked at a block of about 100–120 ms on the reference GPU. The inference < 60 % check uses the default settings. The PRD §6 wording ("with default settings") should be corrected; this is noted in the PR, not edited by this task.
- **Live vs restart**: pitch, gains and gate apply on the next block (atomics). Block, context and crossfade changes restart the stream on slider release (`drag_stopped`), and the loaded model is kept. The provider/GPU and Model… controls are disabled while running, because they reload the model.

## Architecture
```
mic cb → in ring → worker: downmix → ×input gain → input meter
          → Processor::process (vc_engine::stream::Streamer via app adapter)
               device-rate → 16 kHz (StreamResampler) → block buffer w/ past context
               → Pipeline::convert_past → SOLA crossfade → model-rate out
          → ×output gain → per-output resample (from processor rate) → output meter / monitor gain
          → cable ring (+ monitor ring) → output cbs
```
**vc-audio** (`worker.rs`, `session.rs`, `shared.rs`, `ring.rs`)
- `pub trait Processor: Send { fn out_rate(&self) -> u32; fn delay(&self) -> Duration; fn prepare(&mut self) -> Result<(), String> {Ok(())}; fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), String>; }`. `delay` is the algorithmic delay: block + crossfade + search + resampler delays.
- `WorkerConfig` gets the processor's out rate. The output paths resample from it, not from the input rate.
- The worker calls `prepare()` (warm-up at the window length) before its loop. It stops calling `process` after an error and reports the new `StreamStatus::ProcessorFailed`; the error text goes to `tracing`.
- `AudioSession::start(sel, shared, processor: Box<dyn Processor>)`. `AudioSession::stop(self) -> Option<Box<dyn Processor>>` hands the processor back, with its pipeline, when the worker joins. The worker thread returns the processor.
- Output `max_backlog` becomes `block + margin + 100 ms` instead of the fixed `rate/5`, which would drop every 250 ms block. Each output ring is primed once with a `margin` of 40 ms of silence when the first converted block arrives, to absorb inference jitter.
- `Shared` gets `input_gain`, `output_gain` (f32-bit atomics like `monitor_gain`), `latency_us`, and `device_latency_us`. The output callback writes `device_latency_us` from cpal's `OutputCallbackInfo` timestamps (playback − callback). This is one atomic store, no allocation.
- **Latency** = input ring backlog + `processor.delay()` + last inference time + cable ring occupancy after the push + device latency. The worker computes it once per block and stores it in `latency_us`.

**vc-engine**
- `pipeline.rs`: a new `Pipeline::convert_past(window_16k, context) -> Vec<f32>` returns model-rate audio for `window[context..]`, with past context only. `convert` (symmetric, used by offline) shares the same internals; `trim_output` is generalised to (skip, len).
- `stream.rs` (new):
  - `trait Convert { fn sample_rate(&self) -> u32; fn convert_past(&mut self, w: &[f32], ctx: usize, p: &Params) -> Result<Vec<f32>>; }`, implemented by `Pipeline` and by a test fake.
  - `StreamSettings { block_ms, context_ms, crossfade_ms }` with PRD ranges, defaults and clamping.
  - `Controls` (`Arc`, atomics: pitch −24..+24 semitones, gate dB −90..−20).
  - `StreamStats` (`Arc`, atomics: last inference µs, block µs, inference/block ratio smoothed over ~3 s, blocks, late blocks).
  - `Sola`: RVC-style. Search offset k ∈ [0, 10 ms) maximising normalised cross-correlation with the previous tail, a sin²/cos² crossfade over the crossfade length, then emit `block` samples and keep the next tail. Buffers are preallocated.
  - `Streamer<C: Convert>`: input→16 k resampler, history buffer (`context + crossfade + search + block`, extra context rounded up so the window is a multiple of 320). It runs one conversion per full block. If ≥ 2 blocks are pending (inference too slow), it drops the oldest block and counts a late block, which bounds latency. It also offers `warm_up()` and `into_inner() -> C`.
- `vc-convert --stream`: runs `Streamer` over the WAV (unpaced). It writes the output and prints per-block inference p50/p95/max and the ratio to block length, for benchmarking the targets.

**voicechanger app**
- Depends on `vc-engine` and `rfd`. A small `EngineProcessor` adapter implements `vc_audio::Processor` for `Streamer<Pipeline>`.
- A `ModelSlot` state machine: `Empty | Loading(rx) | Ready(Pipeline) | InSession | Failed(msg)`. Loading runs `Pipeline::new` on a `vc-model-loader` thread and reports through an `mpsc` channel polled each frame.
- Voice tab: model row (name · rate · state, Model…), Start/Stop, sliders for pitch, input gain (−20..+20 dB), output gain (−20..+20 dB) and noise gate (−90..−20 dB, default −50), then the meters.
- Settings tab: a new **Performance** group with provider (DirectML/CUDA/CPU) plus a GPU index, block (50–1000), context (0–2000) and crossfade (10–200 ms) sliders. It shows a warning when the smoothed inference ratio is above 60 %.
- A bottom status bar on every tab while running: `Latency N ms · Inference X ms / B ms (P%) · <active provider> · late blocks · underruns`. Values turn amber when latency is above 300 ms or inference is above 60 %.

## Data and interfaces
- New `StreamStatus::ProcessorFailed` (u8 code 7). Its banner reads: "Voice conversion stopped: <short reason>. Details are in the log. Press Start to try again."
- `Params` is unchanged. Pitch and gate are read from `Controls` once per block.
- No settings persistence (M4).

## Error handling and edge cases
- A model load failure (bad file, unsupported signature, missing vendor DLLs) shows in the Voice banner, and the slot goes to `Failed`. If the chosen provider fails to start, the app falls back to CPU, as M2 already does, and the status bar shows "CPU (fallback)".
- Inference that is slower than real time drops input blocks (late blocks counter) instead of letting latency grow. Underruns are still counted by the existing callbacks.
- If a device is lost during a session, the M1 path applies. The processor/pipeline is recovered from `stop()`, so a restart does not reload the model.
- A restart blocks the UI for at most one in-flight inference while the worker is joined. This is acceptable; restarts happen only on slider release.
- Sample-rate mismatch: the model rate (32/40/48 k) is resampled to each device rate in the worker.

## Testing
- **vc-engine unit tests (no assets, CI)**:
  - Settings clamping and window alignment.
  - `Sola` finds a known offset, joins a continuous sine without a step larger than a small threshold, and keeps the output length equal to blocks × block.
  - `Streamer` with a fake `Convert` (an identity resample to 40 kHz): output length and rate, delay value, late-block drop when pending ≥ 2 blocks, pitch/gate read from `Controls`.
  - `convert_past` trim maths.
- **vc-audio unit tests**:
  - The worker with a fake processor (a different out rate, a delay).
  - Input/output gains are applied and seen by the meters.
  - The backlog limit allows a full block.
  - Ring priming.
  - A processor error sets `ProcessorFailed` and the worker keeps running safely.
  - `stop()` returns the processor.
  - The status code round-trips for code 7.
- **Asset e2e** (gated like the existing `tests/e2e.rs`): `Streamer<Pipeline>` over the test WAV produces finite, non-silent audio of the expected length.
- **App unit tests**: status-bar text and colour thresholds, `ModelSlot` transitions with a fake loader result, and the banner for `ProcessorFailed`.
- **Manual** (results go in the PR):
  - On an RTX 3060 and/or RX 6600: inference < 60 % at the defaults, and measured latency ≤ 300 ms at a block of about 100–120 ms. Use `vc-convert --stream` and the app status bar.
  - A 2-hour run with zero underruns or late blocks.

## Out of scope
Model library, hot-switching models while running, saving settings, the global hotkey, the bypass/passthrough toggle, auto block sizing, non-F0 models, GPU enumeration by name, and loopback latency measurement.
