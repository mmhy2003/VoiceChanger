# VC-131 — Original voice in the Model list, plus Bypass: implementation plan

## Goal
The user can send their real, unconverted voice to the virtual cable: an "Original voice (no model)" entry in the Voice tab's Model list makes Start work without a voice model, and a Bypass switch (Voice tab and tray menu) flips a running voice to the original and back without unloading the model.

## Architecture
`vc-audio`'s `Passthrough` becomes a unit struct whose output rate follows the microphone's rate (`Processor::out_rate(in_rate)`), so the app can build it without opening a device. The app gets a pure module `crates/app/src/source.rs` that decides which voice is wanted, whether Start may run and when a running session must restart. `VcApp` keeps `bypass` and `session_source`, builds `Passthrough` or today's `EngineProcessor` at Start, and restarts the stream (model kept in the slot) when the wanted source changes. The Voice tab, status bar and tray show and control it.

## Read this first

**Base branch.** The task branch `megoo/VC-131-…` sits on the old M1 commit `c73fa76`. Everything this plan names exists only on `origin/develop` (`252da20`, 1.1.0). Step 1 starts by fast-forwarding the branch to `origin/develop`. The PR targets `develop`.

**Rules for every step**
- Gate before each commit: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
- `voicechanger` is a binary crate, so an item nothing uses is a `dead_code` warning and fails clippy. The steps are cut so that every new item is used in the step that adds it. Do not add `#[allow]`.
- Commit messages follow the repository's style (`feat(app): … (VC-131)`), with the Co-Authored-By trailer your session is configured with. No history rewrite.
- No manual test steps anywhere: every check is an automated test.
- Never block or allocate in audio callbacks. This plan changes no callback code.

**The instruction "show me a mockup too".** It was met in the design stage (mockup v2, which the approved design refers to). I tried to show a v3 with the exact texts below, but `show_mockup` is refused in the plan stage (`not_allowed_in_role`), so there is no new mockup. The design is unchanged.

## Details settled while planning (no design change)
1. **`CheckMenuItem` is available.** I read muda 0.21.2 in the local cargo registry: `CheckMenuItem::with_id(id, text, enabled, checked, None)`, `set_checked`, `set_enabled`; tray-icon 0.26.1 re-exports it as `tray_icon::menu::CheckMenuItem`. The design's plain-item fallback is not needed.
2. **Windows flips the tick itself on every click** (`handle_item_activate` in muda's `platform_impl/windows/mod.rs`). `Tray::sync` only calls Windows when the state changed, so `Tray::poll` mirrors that flip into `shown` (`tick_after`). If the app refuses a flip, the next `sync` puts the tick back.
3. **`restart_source`**, a fourth pure function in `source.rs`, wraps the design's `source_restart` so the rule "a restart for another reason restarts the source that was running" has a table test.
4. **`ringbuf = "0.5"` as a dev-dependency of the app**, for the round-trip test's ring traits. It is already in `Cargo.lock` through `vc-audio`; no new crate.
5. **Deleting a model that is still loaded.** After "Original voice" the old pipeline stays in the slot (design). Deleting that model on the Models tab must empty the slot first, like the active model today, so its file is not in use.
6. **Bypass is cleared whenever no model is chosen** (for example after the chosen model is deleted while stopped), so the checkbox and the tray tick never show a stale Bypass.
7. `VcApp::start_blocked()` becomes the one caller of `start_hint` for the Start button, the hotkey and the tray.

## File map

| File | Change |
|---|---|
| `crates/audio/src/processor.rs` | `Processor::out_rate(&self, in_rate: u32)`; `Passthrough` unit struct; test |
| `crates/audio/src/session.rs` | `processor.out_rate(input.rate)`; test update |
| `crates/audio/src/worker.rs` | tests only: helpers updated, new tone test through a worker thread |
| `crates/audio/src/control.rs` | tests only: `Box::new(Passthrough)` |
| `crates/app/Cargo.toml`, `Cargo.lock` | dev-dependency `ringbuf = "0.5"` |
| `crates/app/src/engine.rs` | `out_rate(_in_rate)`; Bypass round-trip test on real worker threads |
| `crates/app/src/model.rs` | `ModelInfo::id`, `ModelSlot::is_in_session()`, loaders take the ID |
| `crates/app/src/switch.rs` | `plan_switch(has_pipeline, in_session)` (meaning and docs) |
| `crates/app/src/source.rs` (new) | `VoiceSource`, `wanted_source`, `start_source`, `source_restart`, `restart_source`, `ORIGINAL_VOICE` |
| `crates/app/src/main.rs` | `mod source;` |
| `crates/app/src/app.rs` | `bypass`, `session_source`, `chosen_model`, `start`/`start_with`/`stop`/`recover`, restart rule, provider rule, `select_original`, `toggle_bypass`, `status_snapshot`, `sync_tray`, `poll_tray` |
| `crates/app/src/tabs/voice.rs` | Model list entry, Bypass checkbox, greyed Pitch/Noise gate, `start_hint`, `status_line`, `model_state` |
| `crates/app/src/tabs/settings.rs` | inference warning only while converting |
| `crates/app/src/tabs/models.rs` | `delete` empties the slot when it holds the deleted model |
| `crates/app/src/status_bar.rs` | `StatusSnapshot::original` and its three-part bar |
| `crates/app/src/tray.rs` | ticked Bypass item, `TrayCommand::Bypass`, `TrayState::{bypass, bypass_enabled}`, `tick_after` |
| `CHANGELOG.md`, `README.md` | user-facing notes |

## Steps

### Step 1 — vc-audio: `Processor::out_rate(in_rate)`, unit `Passthrough`, tone test through the worker

**Base branch first.** `git fetch origin`, then `git merge-base --is-ancestor origin/develop HEAD`. If it fails, `git merge --ff-only origin/develop` (the branch has no commits of its own; `c73fa76` is in develop's history). If the fast-forward is refused, stop and report; do not rebase or reset.

Files: `crates/audio/src/processor.rs`, `session.rs`, `worker.rs`, `control.rs`; `crates/app/src/engine.rs`.

Tests first
1. `processor.rs`: replace `passthrough_appends_input` with `passthrough_follows_the_input_rate` (Code A1).
2. `worker.rs` tests: add `tone_freq` and `original_voice_reaches_cable_and_monitor` (Code A2). A 44.1 kHz stereo 1 kHz tone at 0.5 goes through `Passthrough` on a real spawned worker to a 48 kHz cable ring and a 44.1 kHz monitor ring (monitor gain 0.5). Cable: 1000 ± 20 Hz at 0.5 ± 0.05. Monitor: 1000 ± 20 Hz at 0.25 ± 0.03.
3. `cargo test -p vc-audio`. Expected: does not compile (`Passthrough` is not a value; `out_rate` takes no argument).

Implementation
- `processor.rs`: `/// Sample rate of the samples `process` appends, for input at `in_rate`.` `fn out_rate(&self, in_rate: u32) -> u32;`. `Passthrough` becomes `#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)] pub struct Passthrough;` with the doc comment "The original voice: copies the input unchanged, at the input rate. The app runs it when no voice model is chosen and in Bypass." Delete `Passthrough::new`. `out_rate` returns `in_rate`; `delay` and `process` stay.
- `session.rs`, `AudioSession::start`: `let proc_rate = processor.out_rate(input.rate);`. Test `start_unknown_input_is_device_not_found`: `Box::new(Passthrough)`.
- `control.rs` tests: both `Box::new(Passthrough::new(48_000))` become `Box::new(Passthrough)`.
- `worker.rs` tests: the helper `passthrough()` uses `Box::new(Passthrough)`; `Fake::out_rate` and `SlowBlocks::out_rate` take `_in_rate: u32`.
- `crates/app/src/engine.rs`: `fn out_rate(&self, _in_rate: u32) -> u32 { self.streamer.sample_rate() }`; the test `processor_reports_rate_and_delay` calls `p.out_rate(16_000)`.

Verify: `cargo test -p vc-audio` and `cargo test -p voicechanger engine::` pass; then the gate.

Commit: `feat(audio): Passthrough follows the input rate and carries the original voice (VC-131)`

### Step 2 — app: Bypass round trip on real worker threads

Files: `crates/app/Cargo.toml`, `Cargo.lock`, `crates/app/src/engine.rs` (tests module only).

This step adds a test that pins behaviour step 1 delivered, so it is expected to **pass on its first run**. It is the design's "Bypass round trip": converted voice, then the original voice, then the same converter again.

1. `crates/app/Cargo.toml`, under `[dev-dependencies]`: `ringbuf = "0.5"` (same version as `vc-audio`; `Cargo.lock` only gains the edge).
2. `engine.rs` tests: add the imports, `processor_with`, `into_engine`, `run_on_worker` and the test `bypass_round_trip_keeps_the_converter` (Code B). `processor(fail_from)` becomes a call to `processor_with`.
3. `cargo test -p voicechanger bypass_round_trip`. Expected: 1 passed.
4. Show that it can fail: change the expected pair `[0.25, -0.5]` in the assertion to `[0.25, 0.5]`, run again, see the failure "the original voice was changed", and change it back.
5. `cargo deny check` (no new crate) and the gate.

What the test proves: `EngineProcessor<FakeConvert>` streams one block through a worker thread; after `stop` the converter comes back with its call count; `Passthrough` on a worker returns the input sample for sample and comes back as `Passthrough`; the same converter then streams again and its call count grows.

Commit: `test(app): Bypass round trip on real worker threads (VC-131)`

### Step 3 — app: the model slot knows its model's ID and whether a session holds it

Files: `crates/app/src/model.rs`, `switch.rs`, `app.rs`. Behaviour does not change yet: today every session holds the model, so "in a session" equals "running".

Tests first (`model.rs` tests)
- The helper `info()` gains `id: "voice-id".into()`.
- New test `is_in_session_only_while_a_session_holds_the_model` (Code C1).
- `cargo test -p voicechanger model::`. Expected: does not compile (no field `id`, no method `is_in_session`).

Implementation
- `model.rs`: `ModelInfo` gains `/// The model's ID in the library.` `pub id: String,` as its first field. `ModelSlot::is_in_session(&self) -> bool { matches!(self, Self::InSession(_)) }` with the doc comment "A running session holds the model." `spawn_load` and `spawn_generator_load` get a new first parameter `id: String` and put it in the `ModelInfo` they build.
- `switch.rs`: rename `plan_switch`'s second parameter to `in_session`. Docs: `Swap` = "The pipeline is idle (stopped, or the original voice is running): load the new generator and swap it in." `SwapWithRestart` = "A session is streaming the model: load the new generator while the old voice plays, then stop, swap and start." In `plan_switch_cases` add the comment `// (has_pipeline, in_session): a session that sends the original voice does not hold the model.` The table itself is unchanged.
- `app.rs` `select_model`: replace the "Already active" check with Code C2 (decided by the slot's model ID). Call `plan_switch(self.model.info().is_some(), self.model.is_in_session())`. Pass `entry.id.clone()` as the first argument of both loaders.
- `app.rs` `poll_switch`: `let in_session = self.model.is_in_session();` replaces `let running = self.is_running();` (both `if running` become `if in_session`). Doc comment: "Swaps in a pending generator once it is loaded, with a quick stop and start when a session holds the model. A failed load keeps the old voice."
- `app.rs` `recover`: start with `if !self.model.is_in_session() { return; }` and extend the doc comment: "A session that sent the original voice has nothing to give back."

Verify: `cargo test -p voicechanger model::` and `switch::` pass; then the gate.

Commit: `feat(app): the model slot knows its model's ID and whether a session holds it (VC-131)`

### Step 4 — app: `source.rs`; Start without a model sends the original voice

Files: new `crates/app/src/source.rs`; `main.rs`; `app.rs`; `tabs/voice.rs`.

Tests first
- `source.rs` with only its tests module (Code D1): `wanted_source_table`, `start_source_never_falls_back_to_the_original_voice`, `source_restart_table`, `restart_source_table`.
- `voice.rs` tests: in `start_hint_order` the fifth assertion expects `Some("The model is not loaded")`; add `start_hint_original_voice_needs_no_model` (Code D2).
- `cargo test -p voicechanger source::`. Expected: does not compile (the functions do not exist).

Implementation
- `source.rs`: Code D3. `main.rs`: `mod source;` between `mod self_test;` and `mod status_bar;`.
- `voice.rs`: `start_hint(audio_alive, blocker, voice_ready)`; the third hint is `"The model is not loaded"`; doc: "… then a chosen model that is not loaded." In `show`, the hint comes from `app.start_blocked()`; remove the now unused `start_blocker` import.
- `app.rs`: Code D4. In short: fields `bypass` and `session_source`; `chosen_model`, `model_chosen`, `wanted`; `start` picks the source with `start_source` and calls the new `start_with`, which builds `Box::new(Passthrough)` or today's `EngineProcessor`; `stop` clears `session_source`; `start_blocked` becomes `pub(crate)` and passes `start_source(..).is_some()`; `sync_tray` names what is sent; the after-draw restart uses `restart_source`; a provider or GPU change always empties the slot and reloads only a chosen model.

After this step Start is enabled with no model and sends the original voice; the Model list still looks as before (step 6 changes it).

Verify: `cargo test -p voicechanger source::` (4 passed) and `voice::` pass; then the gate.

Commit: `feat(app): Start without a model sends the original voice (VC-131)`

### Step 5 — app: the status line and status bar say "original voice"

Files: `crates/app/src/status_bar.rs`, `tabs/voice.rs`, `tabs/settings.rs`, `app.rs`.

Tests first
- `status_bar.rs`: the helper `snapshot()` gains `original: false`; add `status_text_original` (Code E1): `"Latency 52 ms · Original voice (no conversion) · underruns 0"`.
- `voice.rs`: the three `status_line` tests pass `false` as the second argument; add `status_line_running_original` (Code E2): `"Running · original voice · input 48 kHz · cable 48 kHz · monitor off"`, and `status_line(None, true) == "Stopped"`.
- `cargo test -p voicechanger status`. Expected: does not compile (no field `original`; `status_line` takes one argument).

Implementation
- `status_bar.rs`: `StatusSnapshot` gains `/// The session sends the original voice: there is no inference, provider or late block to show.` `pub original: bool,` after `latency`. `parts` returns `Vec<(String, bool)>`: for `s.original` it is latency, `("Original voice (no conversion)", false)`, underruns; otherwise today's five parts. Module doc: add "or that the original voice is sent".
- `voice.rs`: `status_line(info: Option<SessionInfo>, original: bool)` inserts `"original voice · "` after `"Running · "` when `original`. `show` calls `status_line(app.session_info, app.session_source == Some(VoiceSource::Original))`.
- `app.rs`: add `/// The running session converts the voice.` `pub(crate) fn is_converting(&self) -> bool { self.session_source == Some(VoiceSource::Model) }`. `status_snapshot` (Code E3) reads the inference stats and the provider only when converting; otherwise zeros, an empty provider and `original: true`.
- `settings.rs` `performance`: `if app.is_converting() && let Some(warning) = inference_warning(app.stats.ratio())`.

Verify: `cargo test -p voicechanger status` passes; then the gate.

Commit: `feat(app): the status line and status bar say when the original voice is sent (VC-131)`

### Step 6 — app: "Original voice (no model)" in the Model list and the Bypass checkbox

Files: `crates/app/src/source.rs`, `tabs/voice.rs`, `tabs/models.rs`, `app.rs`.

Tests first (`voice.rs` tests)
- `model_state_original_and_model` (Code F1): `ORIGINAL_VOICE == "Original voice (no model)"`; `model_state(false, slot)` is `"Original voice · no conversion"`; `model_state(true, slot)` is the slot's label.
- `cargo test -p voicechanger voice::`. Expected: does not compile (`ORIGINAL_VOICE` and `model_state` do not exist).

Implementation
- `source.rs`: `/// The Model list's first entry.` `pub const ORIGINAL_VOICE: &str = "Original voice (no model)";`.
- `voice.rs`: `model_state`, `CONVERTED_ONLY`, the new `model_picker` and the changed part of `show` (Code F2). The Model list is always shown; its first entry is `ORIGINAL_VOICE`, selected when no model is chosen. With an empty library the "No models yet. / Open Models" row is drawn below it. The Start/Stop button and the checkbox "Bypass: send my original voice" share one row; the checkbox is disabled while no model is chosen. Pitch and Noise gate are disabled only while no model is chosen, with the note "Pitch and noise gate apply to converted voices only." below the sliders. In Bypass they stay enabled.
- `app.rs`: `select_original` (Code F3). In `update`, right after the `if !self.in_tray { self.draw(ctx); }` block and before the restart check: `// Bypass only exists for a chosen model (it may just have been deleted).` `if !self.model_chosen() { self.bypass = false; }`.
- `models.rs` `delete`: empty the slot also when it still holds the deleted model (Code F4). `delete_blocked(is_active, app.is_running())` is unchanged, so deleting the active model stays blocked while audio runs, including in Bypass.

Behaviour to check against the design (all follow from `restart_source` in step 4): picking Original voice while a voice runs restarts into the original voice and clears Bypass; ticking Bypass restarts into the original voice and keeps the model in the slot; unticking restarts into the model; picking another model with Bypass on loads it and keeps Bypass on; picking the loaded model again after Original voice is instant.

Verify: `cargo test -p voicechanger voice::` passes; then the gate.

Commit: `feat(app): Original voice in the Model list and a Bypass switch on the Voice tab (VC-131)`

### Step 7 — app: Bypass in the tray menu

Files: `crates/app/src/tray.rs`, `app.rs`.

Tests first (`tray.rs` tests, Code G1)
- `menu_ids_map_to_commands`: `command_for_menu(BYPASS_ID) == Some(TrayCommand::Bypass)` and `BYPASS_ID == "bypass"`.
- Existing `tray_state(..)` calls get two more arguments (`false, false`); `Tray::new` uses `tray_state(false, true, None, false, false)`.
- New `tray_state_bypass`, `tick_after_flips_once_per_click`; `tooltip_text` gains `tooltip(true, Some("Original voice")) == "VoiceChanger: Running (Original voice)"`.
- `cargo test -p voicechanger tray::`. Expected: does not compile (no `TrayCommand::Bypass`, `tray_state` takes three arguments).

Implementation (Code G2)
- `TrayCommand::Bypass`; `const BYPASS_ID: &str = "bypass";`; `command_for_menu` maps it.
- `TrayState` gains `/// The Bypass item is ticked.` `pub bypass: bool,` and `/// A model is chosen, so Bypass can be flipped.` `pub bypass_enabled: bool,`.
- `tray_state(running, start_blocked, sent: Option<&str>, bypass: bool, model_chosen: bool)`; `sent` is what the session sends (the model's name or "Original voice"). `tooltip(running, sent)` keeps its body; its doc says `<what is sent>`.
- `tick_after(shown: bool, commands: &[TrayCommand]) -> bool`.
- `Tray` gains `bypass: CheckMenuItem`, built with `CheckMenuItem::with_id(MenuId::new(BYPASS_ID), "Bypass", shown.bypass_enabled, shown.bypass, None)` and placed between Stop and Exit. `Tray::poll` takes `&mut self` and mirrors Windows' own tick flip into `shown`. `Tray::sync` also calls `self.bypass.set_enabled(state.bypass_enabled)` and `self.bypass.set_checked(state.bypass)`.
- `app.rs`: `toggle_bypass` (Code G3); `poll_tray` uses `self.tray.as_mut().map(Tray::poll)` and handles `TrayCommand::Bypass => self.toggle_bypass()`; `sync_tray` passes `self.bypass, self.model_chosen()`.

The restart after a tray click happens in the same frame: `poll_tray` runs before the after-draw restart check, which also runs while the window is hidden.

Verify: `cargo test -p voicechanger tray::` passes; then the gate.

Commit: `feat(app): Bypass in the tray menu (VC-131)`

### Step 8 — docs: changelog and README

Files: `CHANGELOG.md`, `README.md`. No code.

1. `CHANGELOG.md`: under `## [Unreleased]` add `### Added` with the entry in Code H1.
2. `README.md`: add the section `## Original voice and Bypass` (Code H2) directly before `## Model library and settings`. In the tray sentence of "Model library and settings" change "Open VoiceChanger, Start, Stop and Exit" to "Open VoiceChanger, Start, Stop, Bypass and Exit".
3. Do not bump the version and do not touch the 1.1.0 section.
4. Run `bash .github/scripts/test-version.sh` (it tests the changelog reader) and the gate.

Commit: `docs: Original voice and Bypass in the changelog and README (VC-131)`

## Code

### Code A1 — `crates/audio/src/processor.rs` test
```rust
#[test]
fn passthrough_follows_the_input_rate() {
    let mut p = Passthrough;
    assert_eq!(p.out_rate(44_100), 44_100);
    assert_eq!(p.out_rate(48_000), 48_000);
    assert_eq!(p.delay(), Duration::ZERO);
    assert_eq!(p.prepare(48_000), Ok(()));
    let mut out = vec![1.0];
    p.process(&[0.1, -0.2], &mut out).unwrap();
    assert_eq!(out, [1.0, 0.1, -0.2]);
}
```

### Code A2 — `crates/audio/src/worker.rs` tests
```rust
/// Upward zero crossings per second.
fn tone_freq(samples: &[f32], rate: u32) -> f32 {
    let crossings = samples
        .windows(2)
        .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
        .count();
    crossings as f32 / (samples.len() as f32 / rate as f32)
}

#[test]
fn original_voice_reaches_cable_and_monitor() {
    const IN_RATE: u32 = 44_100;
    const CABLE_RATE: u32 = 48_000;
    let c = WorkerConfig {
        in_rate: IN_RATE,
        in_channels: 2,
        proc_rate: Passthrough.out_rate(IN_RATE),
        outputs: vec![
            OutputConfig {
                kind: OutputKind::Cable,
                rate: CABLE_RATE,
            },
            OutputConfig {
                kind: OutputKind::Monitor,
                rate: IN_RATE,
            },
        ],
    };
    assert_eq!(c.proc_rate, IN_RATE);
    let state = State::new(&c, Box::new(Passthrough))
        .map_err(|(e, _)| e)
        .unwrap();
    let (mut in_prod, in_cons) = ring(2 * IN_RATE as usize);
    let (cable_prod, mut cable_cons) = ring(2 * CABLE_RATE as usize);
    let (monitor_prod, mut monitor_cons) = ring(2 * IN_RATE as usize);
    let shared = Arc::new(Shared::new());
    shared.set_monitor_gain(0.5);
    let stop = Arc::new(AtomicBool::new(false));
    let outputs = vec![cable_prod, monitor_prod];
    let handle = spawn(state, in_cons, outputs, shared, stop.clone()).unwrap();

    // One second of a 1 kHz tone at 0.5, as stereo, in 10 ms chunks. Input queued before the
    // worker is ready is skipped, so a little less than the whole second comes out.
    let tone = sine(IN_RATE, 1000.0, 0.5, IN_RATE as usize);
    let (mut cable, mut monitor) = (Vec::new(), Vec::new());
    let mut stereo = Vec::new();
    for chunk in tone.chunks(441) {
        stereo.clear();
        stereo.extend(chunk.iter().flat_map(|&s| [s, s]));
        assert_eq!(in_prod.push_slice(&stereo), stereo.len());
        std::thread::sleep(Duration::from_millis(5));
        cable.extend(cable_cons.pop_iter());
        monitor.extend(monitor_cons.pop_iter());
    }
    // The worker finishes the block it is in before it sees `stop`.
    let deadline = Instant::now() + Duration::from_secs(2);
    while in_prod.occupied_len() > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    stop.store(true, Relaxed);
    handle.join().unwrap();
    cable.extend(cable_cons.pop_iter());
    monitor.extend(monitor_cons.pop_iter());

    let cable_prime = (CABLE_RATE * PRIME_MS / 1000) as usize;
    let monitor_prime = (IN_RATE * PRIME_MS / 1000) as usize;
    let half_second = |rate: u32| rate as usize / 2;
    assert!(
        cable.len() >= cable_prime + half_second(CABLE_RATE),
        "cable got {}",
        cable.len()
    );
    assert!(
        monitor.len() >= monitor_prime + half_second(IN_RATE),
        "monitor got {}",
        monitor.len()
    );

    // Past the priming silence and the resampler's first 100 ms.
    let cable = &cable[cable_prime + CABLE_RATE as usize / 10..];
    let freq = tone_freq(cable, CABLE_RATE);
    assert!((freq - 1000.0).abs() <= 20.0, "cable freq {freq}");
    let peak = measure(cable).peak;
    assert!((peak - 0.5).abs() <= 0.05, "cable peak {peak}");

    // The monitor runs at the input rate (no resampler), at half volume.
    let monitor = &monitor[monitor_prime..];
    let freq = tone_freq(monitor, IN_RATE);
    assert!((freq - 1000.0).abs() <= 20.0, "monitor freq {freq}");
    let peak = measure(monitor).peak;
    assert!((peak - 0.25).abs() <= 0.03, "monitor peak {peak}");
}
```
The tests module already imports `Arc`, `AtomicBool`, `Relaxed`, `Duration`, `Instant`, `Consumer`, `Observer`, `Producer`, `measure`, `ring`, `Shared` and `Passthrough`.

### Code B — `crates/app/src/engine.rs` tests
Imports to add to the tests module:
```rust
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Instant;

use ringbuf::traits::{Consumer, Producer};
use vc_audio::ring::ring;
use vc_audio::worker::{OutputConfig, OutputKind, PRIME_MS, State, WorkerConfig, spawn};
use vc_audio::{Passthrough, Shared};
```
Helpers and the test:
```rust
fn processor_with(convert: FakeConvert) -> (EngineProcessor<FakeConvert>, ErrorSlot) {
    let streamer = Streamer::new(
        convert,
        StreamSettings::default(),
        Arc::new(Controls::new()),
        Arc::new(StreamStats::new()),
    );
    let error = ErrorSlot::default();
    (EngineProcessor::new(streamer, error.clone()), error)
}

fn processor(fail_from: usize) -> (EngineProcessor<FakeConvert>, ErrorSlot) {
    processor_with(FakeConvert {
        calls: 0,
        fail_from,
    })
}

/// The converter inside the processor a worker handed back.
fn into_convert(p: Box<dyn Processor>) -> FakeConvert {
    (p as Box<dyn Any>)
        .downcast::<EngineProcessor<FakeConvert>>()
        .expect("an EngineProcessor<FakeConvert>")
        .into_inner()
}

/// Runs `processor` on a real worker thread: 16 kHz mono in, one cable ring at the processor's
/// rate. Feeds `chunk` every few milliseconds until `done` accepts what reached the cable (after
/// the priming silence) or 5 s pass. Returns those samples and the processor the worker gives back.
fn run_on_worker(
    processor: Box<dyn Processor>,
    chunk: &[f32],
    done: impl Fn(&[f32]) -> bool,
) -> (Vec<f32>, Box<dyn Processor>) {
    const IN_RATE: u32 = 16_000;
    let rate = processor.out_rate(IN_RATE);
    let cfg = WorkerConfig {
        in_rate: IN_RATE,
        in_channels: 1,
        proc_rate: rate,
        outputs: vec![OutputConfig {
            kind: OutputKind::Cable,
            rate,
        }],
    };
    let state = State::new(&cfg, processor).map_err(|(e, _)| e).unwrap();
    let (mut in_prod, in_cons) = ring(IN_RATE as usize);
    let (out_prod, mut out_cons) = ring(2 * rate as usize);
    let stop = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Shared::new());
    let worker = spawn(state, in_cons, vec![out_prod], shared, stop.clone()).unwrap();
    let prime = (rate * PRIME_MS / 1000) as usize;
    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !done(got.get(prime..).unwrap_or_default()) && Instant::now() < deadline {
        in_prod.push_slice(chunk);
        std::thread::sleep(Duration::from_millis(5));
        got.extend(out_cons.pop_iter());
    }
    stop.store(true, Relaxed);
    let processor = worker.join().unwrap();
    let voice = got.split_off(prime.min(got.len()));
    assert!(got.iter().all(|&s| s == 0.0), "the ring starts with silence");
    (voice, processor)
}

#[test]
fn bypass_round_trip_keeps_the_converter() {
    // 25 ms at 16 kHz; ten of them make one default block, which converts to 10 000 samples.
    let voice = vec![0.1; 400];
    let one_block = |out: &[f32]| out.len() >= 10_000;

    // The converted voice.
    let (p, _) = processor(usize::MAX);
    let (converted, back) = run_on_worker(Box::new(p), &voice, one_block);
    assert!(one_block(&converted), "no converted block reached the cable");
    assert!(converted.iter().any(|&s| s > 0.05), "the converted voice is silent");
    let convert = into_convert(back);
    let calls = convert.calls;
    assert!(calls >= 2, "the warm-up and a block, got {calls} calls");

    // Bypass on: the original voice, sample for sample. The chunk has an even length and the
    // input ring holds a whole number of chunks, so the pairs stay aligned.
    let dry = [0.25, -0.5].repeat(80);
    let enough = |out: &[f32]| out.len() >= 1600;
    let (original, back) = run_on_worker(Box::new(Passthrough), &dry, enough);
    assert!(enough(&original), "the original voice did not reach the cable");
    assert!(
        original.chunks_exact(2).all(|pair| pair == [0.25, -0.5]),
        "the original voice was changed"
    );
    assert!((back as Box<dyn Any>).downcast::<Passthrough>().is_ok());

    // Bypass off: the same converter streams again.
    let (p, error) = processor_with(convert);
    let (converted, back) = run_on_worker(Box::new(p), &voice, one_block);
    assert!(one_block(&converted), "the converter did not stream again");
    assert!(into_convert(back).calls > calls);
    assert_eq!(*error.lock().unwrap(), None);
}
```

### Code C1 — `crates/app/src/model.rs` test
```rust
#[test]
fn is_in_session_only_while_a_session_holds_the_model() {
    let mut slot = ModelSlot::Ready(7u32, info());
    assert!(!slot.is_in_session());
    slot.take();
    assert!(slot.is_in_session());
    assert_eq!(slot.info().map(|i| i.id.as_str()), Some("voice-id"));
    slot.give_back(7);
    assert!(!slot.is_in_session());
    assert!(!ModelSlot::<u32>::Empty.is_in_session());
    assert!(!loading().0.is_in_session());
    assert!(!ModelSlot::<u32>::Failed("bad".into()).is_in_session());
}
```

### Code C2 — `VcApp::select_model`, replacing the "Already active" check
```rust
if self.model.info().is_some_and(|info| info.id == id) {
    // Already loaded, also after Original voice: nothing to load.
    self.pending = None;
    if self.prefs.last_model.as_deref() != Some(id) {
        self.activate(&entry);
    }
    return;
}
```

### Code D1 — `crates/app/src/source.rs` tests
```rust
#[cfg(test)]
mod tests {
    use super::VoiceSource::{Model, Original};
    use super::*;

    #[test]
    fn wanted_source_table() {
        // (model_chosen, bypass)
        assert_eq!(wanted_source(false, false), Original);
        assert_eq!(wanted_source(false, true), Original);
        assert_eq!(wanted_source(true, true), Original);
        assert_eq!(wanted_source(true, false), Model);
    }

    #[test]
    fn start_source_never_falls_back_to_the_original_voice() {
        assert_eq!(start_source(Original, false), Some(Original));
        assert_eq!(start_source(Original, true), Some(Original));
        assert_eq!(start_source(Model, true), Some(Model));
        // A model that is loading, failed or lost blocks Start.
        assert_eq!(start_source(Model, false), None);
    }

    #[test]
    fn source_restart_table() {
        // Stopped: nothing to restart.
        for wanted in [Original, Model] {
            for ready in [false, true] {
                assert!(!source_restart(None, wanted, ready));
            }
        }
        // Already sending what is wanted.
        assert!(!source_restart(Some(Original), Original, false));
        assert!(!source_restart(Some(Original), Original, true));
        assert!(!source_restart(Some(Model), Model, false));
        // Going to the original voice is always possible. The session holds the model then,
        // so the slot is not "ready".
        assert!(source_restart(Some(Model), Original, false));
        // A model that is not ready never restarts the session.
        assert!(!source_restart(Some(Original), Model, false));
        assert!(source_restart(Some(Original), Model, true));
    }

    #[test]
    fn restart_source_table() {
        // (running, wanted, model_ready, forced)
        assert_eq!(restart_source(Original, Original, false, false), None);
        assert_eq!(restart_source(Model, Model, false, false), None);
        assert_eq!(restart_source(Model, Original, false, false), Some(Original));
        assert_eq!(restart_source(Original, Model, true, false), Some(Model));
        // The chosen model is still loading: no restart on its own.
        assert_eq!(restart_source(Original, Model, false, false), None);
        // A device or block-size change restarts what was running.
        assert_eq!(restart_source(Original, Model, false, true), Some(Original));
        assert_eq!(restart_source(Model, Model, false, true), Some(Model));
        assert_eq!(restart_source(Original, Original, true, true), Some(Original));
        // When the source is due to switch as well, the wanted one starts.
        assert_eq!(restart_source(Model, Original, false, true), Some(Original));
        assert_eq!(restart_source(Original, Model, true, true), Some(Model));
    }
}
```

### Code D2 — `crates/app/src/tabs/voice.rs` test
```rust
#[test]
fn start_hint_original_voice_needs_no_model() {
    use crate::source::{start_source, wanted_source};
    // (model_chosen, bypass), with a model that is not loaded.
    let ready = |chosen, bypass| start_source(wanted_source(chosen, bypass), false).is_some();
    assert_eq!(start_hint(true, None, ready(false, false)), None);
    assert_eq!(
        start_hint(true, None, ready(true, false)),
        Some("The model is not loaded")
    );
    // Bypass asks for the original voice, which needs no model.
    assert_eq!(start_hint(true, None, ready(true, true)), None);
}
```

### Code D3 — `crates/app/src/source.rs`
```rust
//! Which voice a session sends to the cable: the user's own or the model's.
//!
//! The original voice is only sent when the user chose it (the Model list entry or Bypass).
//! A model that is loading, failed or lost never falls back to it.

/// What a session sends to the cable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceSource {
    /// The microphone, unconverted.
    Original,
    /// The voice model's conversion.
    Model,
}

/// What the user wants sent: the original voice when no model is chosen or Bypass is on.
pub fn wanted_source(model_chosen: bool, bypass: bool) -> VoiceSource {
    if model_chosen && !bypass {
        VoiceSource::Model
    } else {
        VoiceSource::Original
    }
}

/// What Start runs; `None` when a model is wanted but not ready, which blocks Start.
pub fn start_source(wanted: VoiceSource, model_ready: bool) -> Option<VoiceSource> {
    match wanted {
        VoiceSource::Original => Some(VoiceSource::Original),
        VoiceSource::Model => model_ready.then_some(VoiceSource::Model),
    }
}

/// Whether a running session sends the wrong source and the wanted one can start.
pub fn source_restart(running: Option<VoiceSource>, wanted: VoiceSource, model_ready: bool) -> bool {
    running.is_some_and(|running| {
        running != wanted && start_source(wanted, model_ready).is_some()
    })
}

/// The source a running session restarts with, if it restarts: the wanted one when the source
/// must switch, else the running one when the restart is `forced` (a device, block, context or
/// crossfade change).
pub fn restart_source(
    running: VoiceSource,
    wanted: VoiceSource,
    model_ready: bool,
    forced: bool,
) -> Option<VoiceSource> {
    if source_restart(Some(running), wanted, model_ready) {
        Some(wanted)
    } else {
        forced.then_some(running)
    }
}
```
(`rustfmt` decides the final line breaks.)

### Code D4 — `crates/app/src/app.rs`, step 4
Imports: add `Passthrough` to the `vc_audio` list, and `use crate::source::{VoiceSource, restart_source, start_source, wanted_source};`.

Fields of `VcApp`, after `session_info` (initialise with `bypass: false, session_source: None` in `new`):
```rust
/// What the running session sends; `Some` while running.
pub(crate) session_source: Option<VoiceSource>,
/// Bypass: send the original voice although a model is chosen. Never saved; off at launch.
pub(crate) bypass: bool,
```
Methods:
```rust
/// The chosen library model. `None` is the original voice, also when the saved model is gone.
pub(crate) fn chosen_model(&self) -> Option<&ModelEntry> {
    let id = self.prefs.last_model.as_deref()?;
    self.library.get(id)
}

pub(crate) fn model_chosen(&self) -> bool {
    self.chosen_model().is_some()
}

/// What the user wants sent to the cable.
fn wanted(&self) -> VoiceSource {
    wanted_source(self.model_chosen(), self.bypass)
}

/// Streams the wanted voice with the current selection. Does nothing when a model is wanted
/// but not ready: the original voice is only sent when the user chose it.
pub(crate) fn start(&mut self) {
    if let Some(source) = start_source(self.wanted(), self.model.is_ready()) {
        self.start_with(source);
    }
}

/// Starts a session that sends `source`; on failure, the banner says why.
fn start_with(&mut self, source: VoiceSource) {
    self.banner = None;
    let processor: Box<dyn Processor> = match source {
        VoiceSource::Original => Box::new(Passthrough),
        VoiceSource::Model => {
            let Some(pipeline) = self.model.take() else {
                return;
            };
            let streamer = Streamer::new(
                pipeline,
                self.state.perf.stream,
                self.controls.clone(),
                self.stats.clone(),
            );
            Box::new(EngineProcessor::new(streamer, self.processor_error.clone()))
        }
    };
    *self
        .processor_error
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    match self.audio.start(&self.state.selection, processor) {
        Ok(info) => {
            self.session_info = Some(info);
            self.session_source = Some(source);
        }
        Err((e, back)) => {
            self.recover(back);
            let text = format!("Could not start audio: {e}");
            self.banner = Some(match self.banner.take() {
                Some(lost) => format!("{text}. {lost}"),
                None => text,
            });
        }
    }
}
```
`stop`: inside `if self.session_info.take().is_some() {` add `self.session_source = None;` as the first line.

`start_blocked` (now `pub(crate)`):
```rust
pub(crate) fn start_blocked(&self) -> Option<&'static str> {
    start_hint(
        self.audio.is_alive(),
        start_blocker(&self.state.devices, &self.state.selection),
        start_source(self.wanted(), self.model.is_ready()).is_some(),
    )
}
```
`sync_tray`: the third argument of `tray_state` becomes `sent`:
```rust
let sent = match self.session_source {
    Some(VoiceSource::Original) => Some("Original voice"),
    Some(VoiceSource::Model) => self.model.info().map(|info| info.name.as_str()),
    None => None,
};
```
`update`, replacing the block from "Block, context and crossfade restart…" down to the end of the provider block:
```rust
// A switch of the source (the model became ready; later Bypass and Original voice), or a
// device, block, context or crossfade change, restarts the stream. The model is kept.
if let Some(running) = self.session_source {
    let forced = self.state.selection != before || self.restart_requested;
    if let Some(source) =
        restart_source(running, self.wanted(), self.model.is_ready(), forced)
    {
        self.stop();
        self.start_with(source);
    }
}
self.restart_requested = false;
// A new provider or GPU needs a fresh session. The slot is always emptied, so a pipeline on
// the old provider is never reused; it is reloaded only for a chosen model.
if self.state.perf.provider() != provider_before && !self.is_running() {
    self.pending = None;
    self.model = ModelSlot::Empty;
    if let Some(id) = self.chosen_model().map(|entry| entry.id.clone()) {
        self.select_model(&id);
    }
}
```

### Code E1 — `crates/app/src/status_bar.rs` test
```rust
#[test]
fn status_text_original() {
    let s = StatusSnapshot {
        latency: Duration::from_millis(52),
        original: true,
        inference: Duration::ZERO,
        block: Duration::ZERO,
        ratio: 0.0,
        provider: String::new(),
        late_blocks: 0,
        underruns: 0,
    };
    assert_eq!(
        status_text(&s),
        "Latency 52 ms · Original voice (no conversion) · underruns 0"
    );
    assert!(!latency_warn(&s));
}
```

### Code E2 — `crates/app/src/tabs/voice.rs` test
```rust
#[test]
fn status_line_running_original() {
    let info = SessionInfo {
        input_rate: 48000,
        cable_rate: 48000,
        monitor_rate: None,
    };
    assert_eq!(
        status_line(Some(info), true),
        "Running · original voice · input 48 kHz · cable 48 kHz · monitor off"
    );
    assert_eq!(status_line(None, true), "Stopped");
}
```

### Code E3 — `VcApp::status_snapshot`
```rust
/// What the status bar shows; `None` unless running.
fn status_snapshot(&self) -> Option<StatusSnapshot> {
    self.session_source?;
    let latency = Duration::from_micros(self.shared.latency_us.load(Relaxed));
    let underruns = self.shared.underruns.load(Relaxed);
    if !self.is_converting() {
        // The inference stats are left over from the last converted session.
        return Some(StatusSnapshot {
            latency,
            original: true,
            inference: Duration::ZERO,
            block: Duration::ZERO,
            ratio: 0.0,
            provider: String::new(),
            late_blocks: 0,
            underruns,
        });
    }
    Some(StatusSnapshot {
        latency,
        original: false,
        inference: self.stats.inference(),
        block: self.stats.block(),
        ratio: self.stats.ratio(),
        provider: self
            .model
            .info()
            .map(|info| provider_label(info.requested, info.active))
            .unwrap_or_default(),
        late_blocks: self.stats.late_blocks(),
        underruns,
    })
}
```

### Code F1 — `crates/app/src/tabs/voice.rs` test
```rust
#[test]
fn model_state_original_and_model() {
    assert_eq!(ORIGINAL_VOICE, "Original voice (no model)");
    let slot = "Ana · 40 kHz · DirectML (GPU 0)".to_string();
    assert_eq!(
        model_state(false, slot.clone()),
        "Original voice · no conversion"
    );
    assert_eq!(model_state(true, slot.clone()), slot);
}
```

### Code F2 — `crates/app/src/tabs/voice.rs`
Imports: `use vc_audio::SessionInfo;`, `use crate::source::{ORIGINAL_VOICE, VoiceSource};`.
```rust
/// Shown under the sliders while the original voice is selected.
const CONVERTED_ONLY: &str = "Pitch and noise gate apply to converted voices only.";

/// The text beside the Model list: the model's state, or that nothing is converted.
pub fn model_state(model_chosen: bool, slot_label: String) -> String {
    if model_chosen {
        slot_label
    } else {
        "Original voice · no conversion".to_string()
    }
}

/// What was clicked in the Model list.
enum Pick {
    Original,
    Model(String),
}
```
In `show`, from `model_picker(ui, app);` down to the Noise gate slider:
```rust
model_picker(ui, app);

let model_chosen = app.model_chosen();
let hint = if app.is_running() {
    None
} else {
    app.start_blocked()
};
ui.horizontal(|ui| {
    if app.is_running() {
        if ui.button("■ Stop").clicked() {
            app.stop();
        }
    } else if ui
        .add_enabled(hint.is_none(), egui::Button::new("▶ Start"))
        .clicked()
    {
        app.start();
    }
    ui.add_enabled(
        model_chosen,
        egui::Checkbox::new(&mut app.bypass, "Bypass: send my original voice"),
    );
});
if let Some(hint) = hint {
    ui.label(hint);
}

let pitch = ui.add_enabled(
    model_chosen,
    egui::Slider::new(&mut app.state.voice.pitch, -24..=24)
        .text("Pitch")
        .suffix(" st"),
);
if pitch.changed() {
    app.pitch_changed();
}
let voice = &mut app.state.voice;
ui.add(
    egui::Slider::new(&mut voice.input_db, -20.0..=20.0)
        .text("Input gain")
        .suffix(" dB"),
);
ui.add(
    egui::Slider::new(&mut voice.output_db, -20.0..=20.0)
        .text("Output gain")
        .suffix(" dB"),
);
ui.add_enabled(
    model_chosen,
    egui::Slider::new(&mut voice.gate_db, -90.0..=-20.0)
        .text("Noise gate")
        .suffix(" dB"),
);
if !model_chosen {
    ui.label(CONVERTED_ONLY);
}
```
The picker:
```rust
/// The Model list: "Original voice (no model)" first, then the library entries whose file
/// exists; beside it the model's state, below it a pending switch. Picking an entry switches
/// to it, also while running.
fn model_picker(ui: &mut egui::Ui, app: &mut VcApp) {
    let active = app
        .chosen_model()
        .map(|e| (e.id.clone(), e.name.clone()));
    let selected = active
        .as_ref()
        .map_or(ORIGINAL_VOICE, |(_, name)| name.as_str());
    let choices: Vec<(String, String)> = app
        .library
        .entries()
        .iter()
        .filter(|e| app.library.file_exists(e))
        .map(|e| (e.id.clone(), e.name.clone()))
        .collect();
    let mut picked = None;
    ui.horizontal(|ui| {
        egui::ComboBox::from_label("Model")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(active.is_none(), ORIGINAL_VOICE)
                    .clicked()
                {
                    picked = Some(Pick::Original);
                }
                for (id, name) in &choices {
                    let is_active = active.as_ref().is_some_and(|(a, _)| a == id);
                    if ui.selectable_label(is_active, name).clicked() {
                        picked = Some(Pick::Model(id.clone()));
                    }
                }
            });
        ui.label(model_state(active.is_some(), app.model.label()));
    });
    if app.library.entries().is_empty() {
        ui.horizontal(|ui| {
            ui.label("No models yet.");
            if ui.button("Open Models").clicked() {
                app.tab = Tab::Models;
            }
        });
    }
    if let Some(pending) = &app.pending {
        ui.label(format!("Switching to {}…", pending.name));
    }
    match picked {
        Some(Pick::Original) => app.select_original(),
        Some(Pick::Model(id)) => app.select_model(&id),
        None => {}
    }
}
```
Module doc of `voice.rs`: "Voice tab: the Model list with the original voice, Start/Stop and Bypass, live voice controls, level meters, status and the banner."

### Code F3 — `VcApp::select_original`
```rust
/// Makes the original voice the active choice. The loaded pipeline stays in the slot, so
/// picking its model again is instant.
pub(crate) fn select_original(&mut self) {
    self.prefs.last_model = None;
    self.bypass = false;
    self.pending = None;
}
```

### Code F4 — `crates/app/src/tabs/models.rs`, in `delete`
```rust
// After "Original voice" the slot may still hold this model; its file must not be in use.
let loaded = app.model.info().is_some_and(|info| info.id == id);
if is_active || loaded {
    app.model = ModelSlot::Empty;
    app.pending = None;
}
```
Doc comment of `delete`: "Deletes a model and its file. The active model can only go while stopped; a slot that holds the model is emptied first so the file is not in use."

### Code G1 — `crates/app/src/tray.rs` tests
```rust
#[test]
fn tray_state_bypass() {
    let bypassed = tray_state(true, false, Some("Original voice"), true, true);
    assert!(bypassed.bypass && bypassed.bypass_enabled);
    assert_eq!(bypassed.tooltip, "VoiceChanger: Running (Original voice)");
    let converting = tray_state(true, false, Some("Ana"), false, true);
    assert!(!converting.bypass && converting.bypass_enabled);
    assert_eq!(converting.tooltip, "VoiceChanger: Running (Ana)");
    // No model chosen: Bypass is greyed and unticked.
    let original = tray_state(true, false, Some("Original voice"), false, false);
    assert!(!original.bypass && !original.bypass_enabled);
    assert_eq!(original.tooltip, "VoiceChanger: Running (Original voice)");
}

#[test]
fn tick_after_flips_once_per_click() {
    use TrayCommand::{Bypass, Open, Start};
    assert!(!tick_after(false, &[]));
    assert!(tick_after(false, &[Bypass]));
    assert!(!tick_after(true, &[Bypass]));
    assert!(!tick_after(false, &[Bypass, Open, Bypass]));
    assert!(tick_after(true, &[Open, Start]));
}
```
In `menu_ids_map_to_commands` add:
```rust
assert_eq!(BYPASS_ID, "bypass");
assert_eq!(command_for_menu(BYPASS_ID), Some(TrayCommand::Bypass));
```
In `tooltip_text` add:
```rust
assert_eq!(
    tooltip(true, Some("Original voice")),
    "VoiceChanger: Running (Original voice)"
);
```

### Code G2 — `crates/app/src/tray.rs`
```rust
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem};

/// The tray for the app's state. `start_blocked` is whether the Start button has a hint;
/// `sent` is what a running session sends: the model's name or "Original voice".
pub fn tray_state(
    running: bool,
    start_blocked: bool,
    sent: Option<&str>,
    bypass: bool,
    model_chosen: bool,
) -> TrayState {
    TrayState {
        running,
        start_enabled: !running && !start_blocked,
        bypass,
        bypass_enabled: model_chosen,
        tooltip: tooltip(running, sent),
    }
}

/// The Bypass tick the menu shows after `commands`: Windows flips it itself on every click.
pub fn tick_after(shown: bool, commands: &[TrayCommand]) -> bool {
    commands
        .iter()
        .filter(|c| **c == TrayCommand::Bypass)
        .fold(shown, |tick, _| !tick)
}
```
In `Tray::new`:
```rust
let shown = tray_state(false, true, None, false, false);
// ...
let bypass = CheckMenuItem::with_id(
    MenuId::new(BYPASS_ID),
    "Bypass",
    shown.bypass_enabled,
    shown.bypass,
    None,
);
let exit = item(EXIT_ID, "Exit", true);
let items: [&dyn IsMenuItem; 5] = [&open, &start, &stop, &bypass, &exit];
let menu = Menu::with_items(&items).map_err(|e| e.to_string())?;
```
`poll` and the addition to `sync` (after `self.stop.set_enabled(state.running);`):
```rust
/// The commands clicked since the last call.
pub fn poll(&mut self) -> Vec<TrayCommand> {
    let commands: Vec<TrayCommand> = self.rx.try_iter().collect();
    // Keeps `shown` true to the menu, so `sync` puts the tick back if the app refused the flip.
    self.shown.bypass = tick_after(self.shown.bypass, &commands);
    commands
}

self.bypass.set_enabled(state.bypass_enabled);
self.bypass.set_checked(state.bypass);
```
Module doc of `tray.rs` and the doc of `sync`: mention the ticked Bypass item.

### Code G3 — `crates/app/src/app.rs`, step 7
```rust
/// Flips Bypass from the tray. Bypass only exists for a chosen model.
fn toggle_bypass(&mut self) {
    if self.model_chosen() {
        self.bypass = !self.bypass;
    }
}
```
`poll_tray`: `let commands = self.tray.as_mut().map(Tray::poll).unwrap_or_default();` and the arm `TrayCommand::Bypass => self.toggle_bypass(),`.
`sync_tray`: `tray_state(self.is_running(), self.start_blocked().is_some(), sent, self.bypass, self.model_chosen())`.

### Code H1 — `CHANGELOG.md`
```markdown
## [Unreleased]

### Added
- Original voice and Bypass: the Voice tab's Model list now starts with **Original voice (no model)**, so Start works without a voice model and sends your microphone to the virtual cable unchanged. With a voice selected, **Bypass: send my original voice** (Voice tab and tray menu) flips a running voice to your original voice and back without unloading the model; the audio restarts with a short gap. The status line, the status bar and the tray tooltip say when the original voice is being sent.
```

### Code H2 — `README.md`
```markdown
## Original voice and Bypass

The Voice tab's Model list starts with **Original voice (no model)**. With it selected, Start sends your microphone to the virtual cable unchanged, so other apps hear your real voice. No voice model and no GPU are used, and it is what a fresh install starts on.

With a voice selected, **Bypass: send my original voice** switches a running voice to your original voice and back. It is on the Voice tab and in the tray menu. The model stays loaded; the audio restarts underneath, so there is a short gap. Pitch and the noise gate apply to converted voices only. Bypass is not saved: it is off each time VoiceChanger starts.
```

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo build --release
bash .github/scripts/test-version.sh
```
CI runs the same on the PR against `develop`, with `VC_REQUIRE_ASSETS=1` after `cargo xtask fetch-models`, plus the installer test. The runners have no audio devices; the tests below drive the worker over ring buffers.

The tests that prove the feature:
- `cargo test -p vc-audio original_voice_reaches_cable_and_monitor` — the original voice is heard on the cable and the monitor.
- `cargo test -p vc-audio passthrough_follows_the_input_rate`
- `cargo test -p voicechanger bypass_round_trip_keeps_the_converter` — the Bypass flip keeps the model.
- `cargo test -p voicechanger source::` — never falls back to the original voice; a model that is not ready never restarts the session.
- `cargo test -p voicechanger voice::` — `start_hint` order and the Original-voice case, `status_line`, `model_state`.
- `cargo test -p voicechanger status_bar::`, `tray::`, `model::`, `switch::`.

## Design requirement → step
| Requirement | Step |
|---|---|
| `Processor::out_rate(in_rate)`, unit `Passthrough`, session uses the input rate | 1 |
| Worker-thread tone test (44.1 kHz stereo → 48 kHz cable + monitor) | 1 |
| Bypass round trip on real worker threads | 2 |
| `ModelInfo::id`, `ModelSlot::is_in_session()`, "already loaded" by slot ID, switch plan asks about the session, `poll_switch` restarts only a model session, `recover` only in a session | 3 |
| `VoiceSource`, `wanted_source`, `start_source`, `source_restart`; `start()` builds `Passthrough` or `EngineProcessor`; restart rule; provider/GPU rule; `start_hint` "The model is not loaded"; hotkey and tray Start share the rule | 4 |
| Status line, status bar, `StatusSnapshot::original`, inference stats and Settings warning only while converting | 5 |
| Tray tooltip names what is sent | 4 (text), 7 (tests) |
| Model list entry, `select_original`, Bypass checkbox, greyed Pitch/Noise gate with the note, empty-library row, `ORIGINAL_VOICE` | 6 |
| Bypass not saved, off at launch, kept across Stop/Start; picking Original voice clears it | 4 (field), 6 |
| Saved model missing from the library is treated as Original voice | 4 (`chosen_model`), 6 (list) |
| Deleting the active model stays blocked while running, including in Bypass | 6 (unchanged `delete_blocked`) |
| Tray Bypass item, `TrayCommand::Bypass`, `TrayState::{bypass, bypass_enabled}` | 7 |
| Changelog and README | 8 |
| No new settings field (`last_model` absent = Original voice) | 4, 6 (nothing added to `config.rs`) |
