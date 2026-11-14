# VC-131: Original voice in the Model list, plus Bypass

## Goal
The user can send their real, unconverted voice to the virtual cable. An "Original voice (no model)" entry in the Voice tab's Model list makes Start work without a voice model, and a Bypass switch (Voice tab and tray menu) flips a running voice to the original and back without unloading the model.

Mockup v2 shows the Voice tab and the tray menu as designed.

## Base branch (read this first)
This task's branch `megoo/VC-131-…` currently sits on the old M1 commit `c73fa76` (a stale local `main`). Everything the task describes (`EngineProcessor`, `start_hint`, the tray, `Controls`) is on `origin/develop` at `252da20` (1.1.0). The design is written against `origin/develop`. **The build must start from `origin/develop`**, and the PR targets `develop`.

## Decisions

| Question | Answer | Why / what was rejected |
|---|---|---|
| Model list entry, bypass switch, or both? | **Both** (human's choice, C) | The entry alone has no one-click way back. The switch alone leaves "no model, Start disabled, silence" unfixed. |
| How smooth is the Bypass flip? | **Quick restart with a short gap** (human's choice) | Rejected: seamless crossfade. It needs the original voice delayed about 0.5 s and the GPU busy while bypassed, plus a new dry path in the engine. |
| Where is Bypass reachable? | **Voice tab and tray menu** (human's choice) | Rejected: Voice tab only. Rejected: a second global hotkey, because `hotkey.rs` holds exactly one combination today. |
| How is "Original voice" stored? | `last_model` absent in `settings.toml` | No new setting and no migration. A fresh install starts on Original voice. |
| Is Bypass saved? | No. It is off at launch and kept across Stop/Start within a run. | A saved bypass would surprise the user with their real voice after a restart. |
| What do the status texts show? | "original voice" in the status line, the status bar and the tray tooltip | The task's open question; shown in the mockup. |

**One deviation from the task text.** "Done when" says toggling bypass switches dry/converted "without stopping the session". With the chosen quick restart, the audio streams do restart underneath. What holds from the user's side: Stop is never shown, the model is not reloaded, and the app goes straight back to running. Expected gaps (my estimates from the code, not measured): about ¼ s going to the original voice, a bit over ½ s coming back.

**Safety rule that drives several details below: the original voice is only sent when the user chose it** (the list entry or Bypass). A model that is loading, failed or lost never falls back to the original voice; Start stays disabled instead.

## Architecture

### `vc-audio`
- `crates/audio/src/processor.rs`: `Processor::out_rate` gets the input rate: `fn out_rate(&self, in_rate: u32) -> u32`. Today `AudioSession::start` reads `out_rate()` before the processor knows the mic's rate, so `Passthrough::new(rate)` cannot be built by the app without opening the device first.
- `Passthrough` becomes a unit struct whose `out_rate(in_rate)` returns `in_rate`. `Passthrough::new(rate)` is removed and its uses in the `control.rs`, `session.rs` and `worker.rs` tests are updated. Its doc comment changes from "tests and bring-up" to the app's original-voice path.
- `crates/audio/src/session.rs`: `let proc_rate = processor.out_rate(input.rate);`. Nothing else changes: the worker already resamples from the processor rate to the cable and the monitor, and input gain, output gain, meters and latency already work with any processor.

### `app`
- **New `crates/app/src/source.rs`** (pure, like `switch.rs`):
  - `enum VoiceSource { Original, Model }`
  - `wanted_source(model_chosen: bool, bypass: bool) -> VoiceSource`: `Original` when no model is chosen or Bypass is on.
  - `start_source(wanted, model_ready: bool) -> Option<VoiceSource>`: `None` when a model is wanted but not ready (Start is blocked).
  - `source_restart(running: Option<VoiceSource>, wanted, model_ready) -> bool`: true when a running session sends the wrong source and the wanted one can start. Going to `Original` is always possible; going to `Model` waits until the model is ready.
  - `ORIGINAL_VOICE: &str = "Original voice (no model)"`.
- **`crates/app/src/app.rs`**
  - `VcApp` gains `bypass: bool` and `session_source: Option<VoiceSource>` (set with `session_info`).
  - "Model chosen" means `prefs.last_model` is set and still in the library.
  - `start()` builds `Box::new(Passthrough)` for `Original`, and today's `EngineProcessor` for `Model`.
  - `recover()` returns the pipeline only when the slot is in a session. A passthrough session has nothing to give back and must not raise the "model was unloaded" banner.
  - After drawing, the existing restart check also fires on `source_restart(...)`. A restart for another reason (device or block-size change) while the chosen model is still loading restarts with the source that was running.
  - New `select_original()`: clears `last_model`, `bypass` and any pending switch. The loaded pipeline stays in the slot.
  - `select_model()`: "already loaded" is decided by the slot's model ID, so picking the loaded model again after Original voice is instant. The switch plan asks "is the model in a session", not "is audio running".
  - `poll_switch()`: stops and restarts only when the session holds the model. In an original-voice session the new generator is swapped into the ready pipeline directly.
  - Provider or GPU change: the slot is always emptied; it reloads only when a model is chosen. Otherwise a pipeline on the old provider would be reused.
  - `status_snapshot()` and the Settings inference warning use the inference stats only when the running session converts (the stats are stale otherwise).
  - `poll_tray()` handles `TrayCommand::Bypass` by flipping `bypass` when a model is chosen.
- **`crates/app/src/model.rs`**: `ModelInfo` gains `id` (the library ID); `ModelSlot::is_in_session()`.
- **`crates/app/src/tabs/voice.rs`**
  - The Model list is always shown. Its first entry is "Original voice (no model)", selected when no model is chosen; the label beside it reads "Original voice · no conversion". With an empty library the "No models yet. Open Models" row stays below it.
  - A checkbox "Bypass: send my original voice" next to Start/Stop, disabled while Original voice is selected.
  - Pitch and Noise gate are greyed only when no model is chosen, with the note "Pitch and noise gate apply to converted voices only." In Bypass they stay enabled, because they belong to the selected model.
  - `start_hint(audio_alive, blocker, voice_ready)`: the third hint becomes "The model is not loaded" and only appears when a model is chosen but not ready.
  - `status_line(info, original)`: "Running · original voice · input 48 kHz · …".
- **`crates/app/src/status_bar.rs`**: `StatusSnapshot` gains `original: bool`. Then the bar shows "Latency 52 ms · Original voice (no conversion) · underruns 0" and no inference, provider or late-block parts.
- **`crates/app/src/tray.rs`**: a ticked "Bypass" item between Stop and Exit (`TrayCommand::Bypass`, ID `bypass`), enabled when a model is chosen. `TrayState` gains `bypass` and `bypass_enabled`. The tooltip names what is sent: "VoiceChanger: Running (Original voice)" or "(Ana)". The lockfile has tray-icon 0.26.1 / muda 0.21.2; the plan confirms `CheckMenuItem` there, and falls back to a plain item whose text carries the tick if needed.

## Data and interfaces
- `Processor::out_rate(&self, in_rate: u32) -> u32` (changed signature); `Passthrough` (unit struct).
- `settings.toml`: no new fields. `last_model` absent means Original voice.
- New: `VoiceSource`, `wanted_source`, `start_source`, `source_restart`, `TrayCommand::Bypass`, `ModelInfo::id`, `StatusSnapshot::original`.

## Error handling and edge cases
- **Model loading or failed while stopped:** Start is disabled with "The model is not loaded". No fallback to the original voice.
- **Running the original voice, then picking a model:** the original voice keeps running while the model loads, then the audio restarts into it. If the load fails, the banner says why and the original voice keeps running.
- **Bypass on, then picking another model:** Bypass stays on; the new voice is loaded and used when Bypass is turned off.
- **Picking Original voice** clears Bypass.
- **Conversion error mid-session:** unchanged. The session stops with the banner; it does not fall back.
- **A saved model that is no longer in the library:** treated as Original voice, and the Model list shows that.
- **Deleting the active model** stays blocked while audio is running, including in Bypass.
- **The Start/Stop hotkey and tray Start** use the same rule as the Start button through `start_hint`.
- No audio-callback code changes; `Passthrough` copies on the worker thread into a preallocated buffer.

## Testing
All automated, run by `cargo test --workspace` in CI. The runners have no audio devices, so the tests drive the worker over ring buffers.
- **`vc-audio`**: `Passthrough.out_rate` follows the input rate. A worker-thread test sends a 44.1 kHz stereo tone through `Passthrough` to a 48 kHz cable ring and a monitor ring and checks the tone arrives with the right frequency and level (the voice is heard on the cable). Existing tests are updated for the new signature.
- **`source.rs`**: tables for `wanted_source`, `start_source` and `source_restart`, including "a model that is not ready never restarts the session" and "never falls back to the original voice".
- **Bypass round trip (app, real worker threads):** run `EngineProcessor<FakeConvert>`, stop and take the converter back, run `Passthrough` (output equals input), stop, run the same converter again. This shows the flip keeps the model.
- **`voice.rs`**: `start_hint` order and the Original-voice case; `status_line` with the original voice.
- **`status_bar.rs`**, **`tray.rs`** (state, tooltip, menu ID), **`model.rs`** (`is_in_session`, ID), **`switch.rs`**: updated and new cases.

## Docs
- `CHANGELOG.md` under [Unreleased] → Added.
- `README.md`: a short paragraph on Original voice and Bypass, and the tray menu sentence gains Bypass.

## Out of scope
- Seamless crossfade or latency alignment between the original and converted voice.
- A global hotkey for Bypass.
- Saving Bypass across launches.
- A noise gate or pitch shift for the original voice.
