# VC-6 — M4: Model library and persisted settings — implementation plan

## Goal
Users import RVC v2 `.onnx` voices into a library under `%APPDATA%\VoiceChanger\models`, by file picker or drag-and-drop. They manage the voices on the Models tab and switch between them from the Voice tab, even while running. Devices, the last model, the pitch for each model and every control are saved in `%APPDATA%\VoiceChanger\config\settings.toml`. A global hotkey toggles Start/Stop. The app can start minimised, writes rotating logs and has an About box.

## Architecture
- **`vc-audio`** moves to cpal 0.17. It identifies devices by endpoint ID and falls back to the name. Every cpal call runs on a new long-lived `vc-audio-ctl` thread (`AudioControl`), so the UI thread can be STA and window drag-and-drop works again.
- **`vc-engine`**:
  - `inspect_model` validates a model and reads its `sr` metadata for import.
  - `Generator::warm_up` and `Pipeline::swap_generator` let the app replace only the generator, then do a quick Stop → swap → Start.
- **`vc-models`** is a pure-filesystem library: a TOML index, atomic writes and `.bad` backups.
- **`app`**:
  - `paths`, `config` (TOML, debounced save), `import` (a background import queue), `switch` (pure switch planning), `hotkey` (`global-hotkey`) and `files` (pickers and Explorer).
  - The Models, Convert and Settings → General UI.
  - `tracing-appender` logging.

## Facts checked (repo at `bbf5657`)
**Workspace**
- Members are `crates/*` and `xtask`. Edition 2024, `clippy::all = warn`, and CI runs `clippy -D warnings`.
- The packages are `voicechanger` (app), `vc-audio`, `vc-engine`, `vc-models` (empty `src/lib.rs`, no dependencies) and `vc-convert`.

**Already in `Cargo.lock`**
- `cpal 0.16.0`, `serde 1.0.229`, `toml 0.9.12`, `image 0.25.10`, `eframe`/`egui 0.32.3`.
- `dirs`, `global-hotkey`, `tracing-appender` and `egui_extras` are new.

**cpal 0.17 API** (it matches 0.18.2 in the local registry)
- `DeviceTrait::id() -> Result<DeviceId, _>` and `description() -> Result<DeviceDescription, _>` (`.name()`).
- `HostTrait::device_by_id(&DeviceId) -> Option<Device>`.
- `DeviceId` implements `Display`/`FromStr` (`"wasapi:{…}"`).
- `DeviceTrait::name()` is deprecated, so it must be replaced to keep clippy clean.

**Where the code stands**
- `crates/audio/src/devices.rs`: `DeviceList { inputs: Vec<String>, outputs, default_input: Option<String>, default_output }`, `Selection { input: String, cable: String, monitor: Option<String>, monitor_enabled }`, plus `reconcile`, `start_blocker`, `feedback_warnings` and `list_devices`.
- `session.rs`:
  - `AudioSession::start(&Selection, Arc<Shared>, Box<dyn Processor>) -> Result<AudioSession, StartFailure>`, with `StartFailure = (StartError, Option<Box<dyn Processor>>)`;
  - `stop(self) -> Option<Box<dyn Processor>>`;
  - `info() -> SessionInfo`.
- `crates/app/src/app.rs`:
  - `VcApp` holds `session: Option<AudioSession>`, `model: ModelSlot<Pipeline>`, `model_path` and `pick_rx`.
  - `update` restarts on a selection change or `restart_requested`, and reloads on a provider change.
  - `AppState { devices, selection, monitor_db, notes, perf: PerfSettings, voice: VoiceSettings }`.
- `crates/app/src/model.rs`: `ModelSlot<P>` (Empty/Loading/Ready/InSession/Failed), `spawn_load(path, provider, settings)` on `vc-model-loader`, `pick_model()` (rfd on `vc-file-dialog`) and `model_name`.
- `crates/app/src/main.rs` sets `.with_drag_and_drop(false)`, plus a stderr `tracing_subscriber`.
- `crates/engine/src/generator.rs`: `Generator::load(path, provider, sr_override)`, `load_from_memory`, `validate_signature`, `resolve_sr` (rates 32000/40000/48000) and `run(&GenInput)`. Tests use `testutil::{assets_or_skip, temp_path, onnx::{model, rvc_inputs, rvc_output, F32, F16}}` (all `#[cfg(test)]`).
- `crates/engine/src/pipeline.rs`: `Pipeline { hubert, rmvpe, generator, warmup }` and `pub fn frames_for(len)`. `f0::coarse` is public.
- `crates/engine/tests/e2e.rs` has the ignored tests that need `VC_TEST_MODEL`.
- eframe 0.32 with default features (glow): `fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>)`.

**Quality gate (G)** — run before every commit:
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo build --release
```
- Tests are written first, and you watch them fail before you implement. Use `#[expect(dead_code, reason = "…")]` rather than `#[allow]`.
- Commits are conventional, end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`, and never rewrite history.
- Nothing from `.megoocolab/` is committed.

## File map
| File | Change | Responsibility |
|---|---|---|
| `crates/audio/Cargo.toml` | modify | cpal 0.17 |
| `crates/audio/src/devices.rs` | modify | `DeviceInfo`, `DeviceRef`; match by ID, then by name; `reconcile`, `start_blocker` and warnings use refs |
| `crates/audio/src/session.rs` | modify | open devices by `device_by_id`, falling back to the name |
| `crates/audio/src/control.rs` | create | `AudioControl`: the `vc-audio-ctl` thread that owns the session |
| `crates/audio/src/lib.rs` | modify | re-exports |
| `crates/engine/src/generator.rs` | modify | `ModelMeta`, `inspect_model`, `Generator::warm_up` |
| `crates/engine/src/pipeline.rs` | modify | `Pipeline::swap_generator` |
| `crates/engine/src/lib.rs` | modify | re-export `Generator`, `ModelMeta`, `inspect_model` |
| `crates/engine/tests/e2e.rs` | modify | ignored `swaps_generator` |
| `crates/models/Cargo.toml` | modify | serde, toml, thiserror |
| `crates/models/src/lib.rs` | modify | modules and re-exports |
| `crates/models/src/fsutil.rs` | create | `write_atomic`, `back_up_bad` |
| `crates/models/src/library.rs` | create | `Library`, `ModelEntry`, `StagedModel`, `copy_model`, `discard_staged`, `slug`, `LibraryError` |
| `crates/models/src/testutil.rs` | create (test) | `TempDir` |
| `crates/app/Cargo.toml` | modify | vc-models, serde, toml, dirs, tracing-appender, egui_extras, image, global-hotkey |
| `crates/app/src/paths.rs` | create | `AppDirs` (config, models, logs) |
| `crates/app/src/config.rs` | create | `Config` and its parts, `Prefs`, `load`/`save`, `SaveTracker` |
| `crates/app/src/testutil.rs` | create (test) | `TempDir` |
| `crates/app/src/files.rs` | create | `pick_file`, `open_folder` |
| `crates/app/src/import.rs` | create | `classify_drop`, `DropAction`, `Notice`, `Importer` |
| `crates/app/src/switch.rs` | create | `SwitchPlan`, `plan_switch`, `pitch_for`, `PendingSwitch` |
| `crates/app/src/hotkey.rs` | create | `Combo`, `key_code`, `record_key`, `Hotkeys` |
| `crates/app/src/model.rs` | modify | `spawn_load` (name and SR), `spawn_generator_load`, `ModelSlot::ready_mut`; `pick_model`/`model_name` removed |
| `crates/app/src/app.rs` | modify | AudioControl, config, library, import, switching, hotkey toggle, drops, `on_exit` |
| `crates/app/src/main.rs` | modify | logging, config load, `with_minimized`, drag-and-drop on |
| `crates/app/src/tabs/{voice,models,convert,settings}.rs` | modify | UI |
| `CHANGELOG.md`, `README.md` | modify | docs |

## Steps

### 1. audio: upgrade cpal to 0.17
- In `crates/audio/Cargo.toml`, set `cpal = "0.17"`. Run `cargo update -p cpal`.
- Replace every deprecated `name()` with the description name:
  - `session.rs` `find`: `ds.find(|d| d.description().is_ok_and(|desc| desc.name() == name))`;
  - `devices.rs` `list_devices`: `d.description().ok().map(|desc| desc.name().to_string())`, for the lists and for both defaults.
- `session.rs` `open`: in 0.17 `SampleRate` is a plain `u32`, so `rate: config.sample_rate()`. If it is still a newtype, keep `.0`.
- Adapt `status_for_backend_error` and other code to any renamed error types the compiler reports. Behaviour does not change.
- **Tests:** there is no new test. The existing tests (`list_devices_does_not_panic`, `start_unknown_input_is_device_not_found`, …) are the safety net. Run `cargo test -p vc-audio` and expect a pass, then run the gate. `cargo deny check` must pass for the new cpal tree. If it doesn't, stop and report.
- **Commit:** `build(audio): upgrade cpal to 0.17`.

### 2. audio: device IDs with name fallback
See **Appendix A**. In `devices.rs`:
- Add `DeviceInfo { id, name }` and `DeviceRef { id, name }` (with `is_empty`, `same`, `From<&DeviceInfo>`).
- `DeviceList { inputs: Vec<DeviceInfo>, outputs: Vec<DeviceInfo>, default_input: Option<DeviceRef>, default_output: Option<DeviceRef> }`, built by `DeviceList::new(...)`, which sorts cables first.
- `from_names(...)` stays for tests; it uses each name as the ID.
- Add `find_input`/`find_output` (ID first, then name), and `default_input_device`/`default_cable`, which return `Option<&DeviceInfo>` (rename from `default_input_name`).
- `Selection { input: DeviceRef, cable: DeviceRef, monitor: Option<DeviceRef>, monitor_enabled }`.
- `reconcile` refreshes a found ref silently (with the new ID and name). A missing ref falls back to the default with the same note texts as today (using names).
- `start_blocker` and `feedback_warnings` use `find_*` and `same`.
- `list_devices` fills IDs from `d.id()` (`to_string()`, empty on error).
- In `session.rs`, `find(host, devices, &DeviceRef)` parses `r.id` as `cpal::DeviceId`, then tries `host.device_by_id`, then the name match. Errors carry `r.name`.
- In the app, `banner_text` uses `.name`. The Settings combos iterate `DeviceInfo`s and select with `ui.selectable_label(sel.input.same(&d.into()), &d.name)`, assigning `d.into()`. The app tests use `.input.name`.

**Tests first** (`devices.rs`): adapt the existing tests with a helper `r(name)` (id = name), then add:
- `find_matches_id_then_name`: the ID wins over a changed name, the name is used when the ID is unknown, and an empty ref gives `None`.
- `reconcile_refreshes_renamed_device_silently`
- `reconcile_by_name_when_id_changed`
- `monitor_same_as_cable_by_id`

Run `cargo test -p vc-audio devices` and expect a compile failure, then implement, run `cargo test --workspace` and expect a pass. Run the gate.

**Commit:** `feat(audio): select devices by endpoint ID with name fallback`.

### 3. audio: AudioControl thread
- Create `crates/audio/src/control.rs` (**Appendix B**). Export `pub mod control;` and `pub use control::AudioControl;`, and export `session::StartFailure`.
- The `vc-audio-ctl` thread owns `Option<AudioSession>` and serves the `List`, `Start`, `Stop` and `Shutdown` requests over `mpsc`, with one reply channel per request.
- A dead thread works like this:
  - `list()` gives `None`;
  - `start()` gives back the processor recovered from the `SendError`;
  - `stop()` gives `None`.
- `shutdown()` is idempotent and is also called on `Drop`.

**Tests first** (`control.rs`):
- `list_goes_through_thread`: `Some`, no panic.
- `stop_without_session_is_none`
- `start_unknown_device_returns_processor`: `DeviceNotFound` with `Some` processor.
- `shutdown_joins_and_kills`: afterwards `!is_alive()`, `list()` is `None`, and `start` returns `BuildStream` with `Some` processor.
- `shutdown_twice_is_fine`

Run `cargo test -p vc-audio control` and expect a failure, implement, then expect a pass. Run the gate.

**Commit:** `feat(audio): run every cpal call on a dedicated audio-control thread`.

### 4. app: drive audio through AudioControl and re-enable drag-and-drop
**`app.rs`**
- Replace `session: Option<AudioSession>` with `audio: AudioControl` (`AudioControl::spawn(shared.clone())`) and `session_info: Option<SessionInfo>`.
- `is_running()` is `session_info.is_some()`.
- `refresh_devices` and `new` call `self.audio.list().unwrap_or_default()`.
- `start`:
  - `self.audio.start(&self.state.selection, Box::new(processor))`;
  - `Ok(info)` sets `session_info`;
  - `Err((e, back))` calls `recover(back)` and sets the banner as today.
- `stop`: `if self.session_info.take().is_some() { let back = self.audio.stop(); self.recover(back); }`.
- Add `const AUDIO_DEAD: &str = "The audio thread stopped unexpectedly. Restart VoiceChanger.";`. When `!self.audio.is_alive()`, set the banner (once).
- Implement `on_exit(&mut self, _gl: Option<&eframe::glow::Context>)`: stop, then `self.audio.shutdown()`.

**`voice.rs`**
- Add `pub fn start_hint(audio_alive: bool, blocker: Option<&'static str>, model_ready: bool) -> Option<&'static str>`. It is checked in that order: `AUDIO_DEAD` (make it `pub(crate)`), then the blocker, then "Pick a model first".
- Start is enabled only when `start_hint` is `None`, and the hint is shown.
- `status_line(app.session_info)`.

**`main.rs`**: remove `.with_drag_and_drop(false)` and its comment. Also update the `pick_model` doc comment: rfd runs on its own thread because the UI thread is now STA, which OLE drag-and-drop needs.

**Tests first** (`voice.rs`): `start_hint_order` (dead audio wins, then the blocker, then no model, then `None`). Expect a failure, implement, expect a pass. Run the gate.

**Manual check:** devices list, Start/Stop, a device change while running restarts, and dropping a file on the window does not break audio.

**Commit:** `feat(app): move audio control off the UI thread and re-enable drag-and-drop`.

### 5. engine: inspect_model for import
- In `generator.rs` (**Appendix C**), add `ModelMeta { sample_rate: Option<u32>, f0: bool }` and `pub fn inspect_model(path: &Path) -> Result<ModelMeta>`. It opens a CPU session, runs `validate_signature` and parses the `sr` metadata:
  - an unparseable or missing value gives `None`;
  - a value not in `RATES` is an `UnsupportedModel` error with the `resolve_sr` text.
- Extract `fn io_specs(&Session) -> (Vec<IoSpec>, Vec<IoSpec>)` and use it in `Generator::new` and in `inspect_session`.
- Re-export in `lib.rs`: `pub use generator::{Generator, ModelMeta, inspect_model};`.

**Tests first** (generator tests, gated by `assets_or_skip(&[ORT_DLL])`; the bytes go to `temp_path(...)` and are removed afterwards):
- `inspect_valid_reads_sr`: `Some(40000)`, `f0`.
- `inspect_without_sr_is_none`
- `inspect_rejects_fp16_v1_no_pitch`: the exact reasons.
- `inspect_unsupported_sr_rejected`: `"44100"`.
- `inspect_not_onnx_fails`: `EngineError::RuntimeInit`.

Run `cargo test -p vc-engine inspect` and expect a failure, then a pass. Run the gate.

**Commit:** `feat(engine): inspect a model's signature and sample rate for import`.

### 6. engine: generator warm-up and swap
- `Generator::warm_up(&mut self, frames: usize) -> Result<()>` makes one call:
  - `frames.max(1)` frames;
  - zero `phone`;
  - `pitch = f0::coarse(0.0)`;
  - zero `pitchf`;
  - speaker 0.
- `Pipeline::swap_generator(&mut self, generator: Generator) -> Generator` uses `std::mem::replace`. Its doc says ContentVec and RMVPE stay loaded.

**Tests first**:
- `tiny_warm_up_runs` (generator.rs): `warm_up(4)` is `Ok`, `warm_up(0)` is `Ok`, and `run` still works afterwards.
- Ignored `swaps_generator` (e2e.rs):
  1. Build a pipeline from `VC_TEST_MODEL`.
  2. `Generator::load(model, Provider::Cpu, test_model_sr())`, then `warm_up(vc_engine::pipeline::frames_for(settings.window_16k()))`.
  3. Swap. Check `old.sample_rate() == pipeline.sample_rate()`.
  4. `pipeline.convert(&vec![0.0; 16_000], 0, &Params::default())`. Check its length is `sr` and every sample is finite.

Run `cargo test -p vc-engine warm_up`, then the gate. Run the e2e manually if `VC_TEST_MODEL` is set: `cargo test -p vc-engine --test e2e -- --ignored swaps_generator`.

**Commit:** `feat(engine): warm up and hot-swap the generator`.

### 7. models: library index and import
- In `crates/models/Cargo.toml`: `serde = { version = "1", features = ["derive"] }`, `toml = "0.9"`, `thiserror = "2"`.
- Create `fsutil.rs` and `library.rs` (**Appendix D**), and `testutil.rs` (`TempDir`, `#[cfg(test)]`). `lib.rs` declares the modules and re-exports `Library`, `ModelEntry`, `StagedModel`, `LibraryError`, `copy_model`, `discard_staged`, `slug`, `write_atomic`, `back_up_bad` and `INDEX_FILE`.
- `copy_model` copies to `<id>.onnx.part`, then renames. On any error it removes the partial file.
- Every mutation clones the entries, edits the clone, writes the index atomically, then commits it to `self`.

**Tests first**:
- `slug_rules`
- `open_missing_index_is_empty`
- `import_copies_file_and_indexes`: the bytes match, the fields are right, and reopening round-trips.
- `import_gives_unique_names`: `ana-v2`, then `ana-v2-2`.
- `copy_failure_leaves_no_file`
- `discard_staged_removes_file`
- `corrupt_index_backed_up`: `.bad` exists, the note is `Some`, and the model file survives.
- `newer_index_backed_up`
- `missing_file_entry`: `file_exists` is false after reopening.
- `write_atomic_replaces`

Run `cargo test -p vc-models` and expect a failure, implement, expect a pass. Run the gate.

**Commit:** `feat(models): add the model library with atomic TOML index`.

### 8. models: rename, remove, reorder, default pitch, icon
See **Appendix D** (edits):
- `rename` (trimmed; empty gives `EmptyName`);
- `remove` (removes the entry, deletes the model file and the icon, ignoring `NotFound`);
- `move_up`/`move_down` (no-op at the edges);
- `set_default_pitch` (clamped to −24..=24);
- `set_icon` (png/jpg/jpeg, otherwise `IconType`; copies to `icons/<id>.png|jpg` and deletes an old icon with another name).

An unknown ID gives `NotFound`.

**Tests first**:
- `rename_trims_and_rejects_empty`
- `remove_deletes_model_and_icon`
- `remove_unknown_is_not_found`
- `reorder_edges`
- `default_pitch_clamped_and_saved`
- `set_icon_copies_and_replaces`
- `set_icon_rejects_other_types`
- `edits_persist_after_reopen`

Expect a failure, then a pass. Run the gate.

**Commit:** `feat(models): rename, delete, reorder, default pitch and icons`.

### 9. app: paths and config
- Add to the app's dependencies: `vc-models = { path = "../models" }`, `serde = { version = "1", features = ["derive"] }`, `toml = "0.9"`, `dirs = "6"`.
- Create `paths.rs` and `config.rs` (**Appendix E**), and `testutil.rs` (`TempDir`). Declare `mod config; mod paths; #[cfg(test)] mod testutil;` in `main.rs`.
- Derive `Serialize`/`Deserialize` on `ProviderChoice` (`rename_all = "lowercase"`) and on `VoiceSettings` (`#[serde(default)]`).
- Add `PerfConfig::{from_settings, to_settings}`.
- `load`/`save` use `vc_models::{write_atomic, back_up_bad}`.

**Tests first**:
- `dirs_layout`
- `config_defaults`
- `config_round_trip`
- `missing_fields_take_defaults`
- `corrupt_settings_backed_up`
- `newer_settings_backed_up`
- `perf_config_round_trip`
- `save_tracker_debounces`: the exact timeline in Appendix E.

Use `#[expect(dead_code, reason = "wired into the app in the next step")]` on items that are not used yet. Expect a failure, then a pass. Run the gate.

**Commit:** `feat(app): add settings file with debounced save`.

### 10. app: load and save settings
**`main.rs`**: `let dirs = AppDirs::system(); let (config, note) = config::load(&dirs.settings_file());`, then `.with_minimized(config.start_minimised)`, then `VcApp::new(cc, Startup { dirs, config, notes })`.

**`app.rs`**
- `AppState::from_config(devices, &Config)`:
  - the selection comes from the saved refs (missing ones give an empty `DeviceRef`);
  - the notes come from `selection.reconcile(&devices)`;
  - `monitor_db`, `perf` and `voice` are restored.
- Remove `AppState::new`; the tests use `from_config(list, &Config::default())`.
- `AppState::to_config(&self, prefs: &Prefs) -> Config`.
- `VcApp` gains `dirs: AppDirs`, `prefs: Prefs` (`Prefs::from_config`) and `save: SaveTracker`.
- The startup notes (config `.bad` note plus device notes) are joined with a space into `banner`. The device notes also stay in `state.notes` for Settings.

**Saving**
- At the end of `update`: `let cfg = self.state.to_config(&self.prefs); if self.save.poll(&cfg, Instant::now()) { self.write_config(cfg) }`.
- If `save.is_pending()`, call `request_repaint_after(250 ms)`.
- `write_config` logs a warning on error and always calls `mark_saved`.
- `on_exit` saves when `save.needs_save(&cfg)`.

**Tests first** (`app.rs`):
- `state_from_config_restores`: a renamed saved device, no notes, perf, voice and monitor restored.
- `state_from_config_missing_device_notes`
- `to_config_round_trip`

Run the gate. **Manual check:** change controls, wait 1 s, find `settings.toml`, restart and see them restored.

**Commit:** `feat(app): remember devices and controls between launches`.

### 11. app: rotating log files
- Add `tracing-appender = "0.2"`.
- In `main.rs`, add `fn init_logging(dir: &Path) -> Option<WorkerGuard>`:
  - `Builder::new().rotation(Rotation::DAILY).filename_prefix("voicechanger.log").max_log_files(7).build(dir)`;
  - `tracing_appender::non_blocking`, `with_ansi(false)`;
  - debug builds use `.with_writer(file.and(std::io::stderr))` (`tracing_subscriber::fmt::writer::MakeWriterExt`);
  - if the folder or the appender fails, fall back to stderr only.
- `main` holds `let _guard = init_logging(&AppDirs::system().logs);` before loading the config, and logs `info!(version = env!("CARGO_PKG_VERSION"), "VoiceChanger starting")`.
- No unit test: this setup is a global subscriber. Run the gate.
- **Manual check:** `%LOCALAPPDATA%\VoiceChanger\logs\voicechanger.log.YYYY-MM-DD` appears.
- **Commit:** `feat(app): write daily rotating log files`.

### 12. app: import models by picker or drop; Convert hand-off
- Create `files.rs` (**Appendix F**): `pick_file(title, filter, exts)` (moved from `model::pick_model`, which now calls it) and `open_folder(dir)`.
- Create `import.rs` (**Appendix F**).
- `VcApp` gains:
  - `library: Library` (opened at startup; its note goes to the banner);
  - `importer: Importer` (`inspect_model` mapped to a `String` error);
  - `notices: Vec<Notice>`, `convert_file: Option<PathBuf>`, `import_pick_rx`.
- `handle_drops(ctx)` reads `ctx.input(|i| i.raw.dropped_files.clone())`, calls `receive_file(path)` for each file and switches to the Models tab.
- `poll_import` turns `Ok(id)` into `Notice::Info("Imported \"{name}\".")` and `Err(m)` into `Notice::Error(m)`. Repaint every 100 ms while the importer is busy.
- **Models tab** (`show(ui, app)`):
  - **Import…** (onnx/pth/zip) and **Open folder**;
  - the sample-rate notice (32/40/48 kHz, Cancel);
  - notices with Dismiss, and **Open in Convert**, which sets `convert_file` and `tab = Tab::Convert`;
  - a plain list: name, kHz, and "file missing".
- **Convert tab** (`show(ui, app)`): the file name if one is set, then "Converting PyTorch models arrives in M5."

**Tests first** (`import.rs`):
- `classify_drop_cases`
- `import_with_metadata_rate`
- `import_without_rate_waits_for_choice`
- `cancel_rate_skips_file`
- `unsupported_model_message`
- `queue_processes_in_order`
- `offer_convert_text`

Expect a failure, then a pass. Run the gate.

**Manual check:** drop an `.onnx`, an SR-less `.onnx`, a `.pth` and a `.txt`.

**Commit:** `feat(app): import models by picker or drag-and-drop`.

### 13. app: choose and hot-switch library models
- Create `switch.rs` (**Appendix G**).
- `model.rs`:
  - `spawn_load(name, path, sr_override, provider, settings)`;
  - `spawn_generator_load(name, path, sample_rate, provider, settings)` on `vc-gen-loader`, which loads and warms up with `frames_for(settings.window_16k())`;
  - `ModelSlot::ready_mut`;
  - remove `model_name` and `pick_model`.
- `app.rs`:
  - add `select_model(id)`, `activate(entry)`, `poll_switch()` and `pitch_changed()` (Appendix G);
  - remove `model_path`, `pick_rx`, `pick_model` and `load_model`;
  - a provider change while stopped gives `model = Empty; pending = None; select_model(last_model)`;
  - on launch, `select_model(last_model)` if it is in the library (it never starts by itself);
  - repaint while `pending` is set;
  - `MODEL_LOST` becomes "The model was unloaded; choose it again from the Model list."
- Voice tab: a **Model** ComboBox of library entries whose file exists, plus the slot label and "Switching to {name}…". With an empty library, show a button that opens the Models tab. When the pitch slider changes, call `pitch_changed()`.
- Models tab: clicking a row name calls `select_model` (the active row is highlighted).

**Tests first**:
- `plan_switch_cases`
- `pitch_for_saved_else_default`
- `ready_mut_only_when_ready`

Expect a failure, then a pass. Run the gate.

**Manual check:** switch 40 kHz to 48 kHz while running; the new voice arrives in about 1 s or less.

**Commit:** `feat(app): switch library models while running`.

### 14. app: manage models on the Models tab
- Add `egui_extras = { version = "0.32", features = ["image", "file"] }` and `image = { version = "0.25", default-features = false, features = ["png", "jpeg"] }`.
- In `VcApp::new`, call `egui_extras::install_image_loaders(&cc.egui_ctx)`.
- Rows (**Appendix H**):
  - ▲▼ (disabled at the edges);
  - the icon (24 px `file://` image, else the `avatar_letter`);
  - the name (selectable);
  - kHz;
  - a default-pitch `DragValue` (−24..=24);
  - **Icon…** (png/jpg/jpeg picker, then `forget_image(uri)`);
  - **Rename** (inline);
  - **Delete** with confirmation.
- A row whose file is missing shows "file missing" and only Delete.
- Delete is blocked by `delete_blocked`. Deleting the active model while stopped empties the slot and clears `pending`, `prefs.last_model` and its `pitch_by_model` entry.
- Errors become `Notice::Error`.

**Tests first**:
- `delete_blocked_cases`
- `avatar_letter_cases`

Run the gate, including `cargo deny check` for the new dependencies.

**Commit:** `feat(app): rename, delete, reorder, pitch and icons on the Models tab`.

### 15. app: global hotkey parsing and registration
- Add `global-hotkey = "0.7"`.
- Create `hotkey.rs` (**Appendix I**): `Combo`, `ComboError`, `KEYS`, `key_code`, `record_key`, `RecordOutcome` and `Hotkeys`. Use `#[expect(dead_code)]` until step 16.

**Tests first**:
- `parse_format_round_trip`
- `modifier_required`
- `unknown_key_rejected`
- `egui_keys_map`
- `combo_to_hotkey`
- `record_key_cases`

Expect a failure, then a pass. Run the gate, including deny.

**Commit:** `feat(app): parse, record and register a global hotkey`.

### 16. app: Settings → General, hotkey toggle and About
- `VcApp` gains `hotkeys: Hotkeys`, `recording_hotkey`, `hotkey_error: Option<String>` and `show_about`.
- At launch, a saved hotkey that fails to parse or register sets `hotkey_error` and the banner. `prefs.hotkey` is kept.
- In `update`, `if self.hotkeys.take_pressed() { self.toggle_from_hotkey() }`:
  - stop if running;
  - otherwise, if there is a `start_hint`, set the banner to `"Hotkey: cannot start. {hint}"`;
  - otherwise start.
- Settings → **General** (after Performance): the hotkey (current text or "Off"), **Set…**/**Clear**, the recorder and its error, **Start minimised**, **Open logs folder** and **About…** (an `egui::Window` built from `about_lines`).

**Tests first** (`settings.rs`): `about_lines_has_licence_and_folders`. Run the gate.

**Manual check:** the hotkey works while the window is minimised, a clash shows the error, and Start minimised works.

**Commit:** `feat(app): add hotkey, start minimised, logs folder and About`.

### 17. docs: changelog, README and final verification
- `CHANGELOG.md` gets an [Unreleased]/Added line.
- `README.md` gets the Status line and a "Model library and settings" section.
- Run the full gate and the manual checklist (see Final verification).
- **Commit:** `docs: model library, saved settings and hotkey`.

---

## Appendix A — devices (step 2)
```rust
/// An endpoint as WASAPI lists it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo { pub id: String, pub name: String }

/// A chosen device, matched by `id` first and then by `name`. Both empty = nothing selected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceRef { pub id: String, pub name: String }

impl DeviceRef {
    pub fn is_empty(&self) -> bool { self.id.is_empty() && self.name.is_empty() }
    /// Same endpoint: by ID when both have one, else by non-empty name.
    pub fn same(&self, other: &DeviceRef) -> bool {
        if !self.id.is_empty() && !other.id.is_empty() { self.id == other.id }
        else { !self.name.is_empty() && self.name == other.name }
    }
}
impl From<&DeviceInfo> for DeviceRef { fn from(d: &DeviceInfo) -> Self { Self { id: d.id.clone(), name: d.name.clone() } } }

fn find_device<'a>(list: &'a [DeviceInfo], r: &DeviceRef) -> Option<&'a DeviceInfo> {
    if r.is_empty() { return None; }
    list.iter().find(|d| !r.id.is_empty() && d.id == r.id)
        .or_else(|| list.iter().find(|d| !r.name.is_empty() && d.name == r.name))
}
```
- `DeviceList::new(inputs: Vec<DeviceInfo>, outputs: Vec<DeviceInfo>, default_input: Option<DeviceRef>, default_output: Option<DeviceRef>)` sorts the outputs with `!is_virtual_cable(&d.name)`. `from_names` wraps each name as `DeviceInfo { id: name.clone(), name }`, and the defaults the same way.
- `default_input_device`: `find_input(default)`, else the first input.
- `default_cable`: the first cable, else `find_output(default_output)`, else the first output.
- `Selection::defaults`: `monitor = default_output` found in the outputs and `!same(cable)`.
- `reconcile_field(field: &mut DeviceRef, found: Option<&DeviceInfo>, default: Option<&DeviceInfo>, label, notes)`:
  - found: `*field = found.into()`, no note;
  - not found: same as today (a note only if `!field.is_empty()`, using `field.name` and the default's `name`), and `*field = default.map(Into::into).unwrap_or_default()`.
- Monitor: found refreshes it. Otherwise the note is `Monitor "{name}" is no longer available; monitor turned off.`, `None`, off.
- `feedback_warnings`: `MonitorSameAsCable` if `monitor_enabled && !cable.is_empty() && monitor.as_ref().is_some_and(|m| m.same(&cable))`. The input check uses `input.name`.
- `start_blocker`: `find_input(&sel.input).is_none()` → "Pick an input device in Settings."; same for the cable.

`session.rs`:
```rust
fn find<I: Iterator<Item = cpal::Device>>(host: &cpal::Host, devices: Result<I, cpal::DevicesError>, r: &DeviceRef) -> Result<cpal::Device, StartError> {
    if let Ok(id) = r.id.parse::<cpal::DeviceId>() && let Some(d) = host.device_by_id(&id) { return Ok(d); }
    devices.ok()
        .and_then(|mut ds| ds.find(|d| d.description().is_ok_and(|x| x.name() == r.name)))
        .ok_or_else(|| StartError::DeviceNotFound(r.name.clone()))
}
```
`Opened.name` and the error names use `r.name`. The `UnsupportedConfig` device name uses `sel.input.name`.

## Appendix B — AudioControl (step 3)
```rust
//! The audio-control thread: every cpal call and the running session live here, off the UI thread.
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

enum Request {
    List(Sender<DeviceList>),
    Start { selection: Selection, processor: Box<dyn Processor>, reply: Sender<Result<SessionInfo, StartFailure>> },
    Stop(Sender<Option<Box<dyn Processor>>>),
    Shutdown,
}

pub struct AudioControl { tx: Sender<Request>, thread: Option<JoinHandle<()>> }

impl AudioControl {
    /// Spawns `vc-audio-ctl`. If it cannot be spawned, the control is dead from the start.
    pub fn spawn(shared: Arc<Shared>) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new().name("vc-audio-ctl".into())
            .spawn(move || run(rx, shared))
            .inspect_err(|e| tracing::error!(%e, "could not start the audio thread")).ok();
        Self { tx, thread }
    }
    pub fn is_alive(&self) -> bool { self.thread.as_ref().is_some_and(|t| !t.is_finished()) }
    pub fn list(&self) -> Option<DeviceList> { let (tx, rx) = mpsc::channel(); self.tx.send(Request::List(tx)).ok()?; rx.recv().ok() }
    pub fn start(&self, selection: &Selection, processor: Box<dyn Processor>) -> Result<SessionInfo, StartFailure> {
        let (reply, rx) = mpsc::channel();
        let req = Request::Start { selection: selection.clone(), processor, reply };
        if let Err(SendError(Request::Start { processor, .. })) = self.tx.send(req) { return Err((dead(), Some(processor))); }
        rx.recv().unwrap_or_else(|_| Err((dead(), None)))
    }
    pub fn stop(&self) -> Option<Box<dyn Processor>> { let (tx, rx) = mpsc::channel(); self.tx.send(Request::Stop(tx)).ok()?; rx.recv().ok().flatten() }
    /// Stops any session and joins the thread. Idempotent.
    pub fn shutdown(&mut self) { let _ = self.tx.send(Request::Shutdown); if let Some(t) = self.thread.take() { let _ = t.join(); } }
}
impl Drop for AudioControl { fn drop(&mut self) { self.shutdown(); } }

fn dead() -> StartError { StartError::BuildStream { device: "audio control thread".into(), reason: "not running".into() } }

fn run(rx: Receiver<Request>, shared: Arc<Shared>) {
    let mut session: Option<AudioSession> = None;
    while let Ok(req) = rx.recv() {
        match req {
            Request::List(reply) => { let _ = reply.send(list_devices()); }
            Request::Start { selection, processor, reply } => {
                let result = if session.is_some() {
                    Err((StartError::BuildStream { device: "audio session".into(), reason: "already running".into() }, Some(processor)))
                } else {
                    AudioSession::start(&selection, shared.clone(), processor).map(|s| { let info = s.info(); session = Some(s); info })
                };
                let _ = reply.send(result);
            }
            Request::Stop(reply) => { let _ = reply.send(session.take().and_then(AudioSession::stop)); }
            Request::Shutdown => break,
        }
    }
} // dropping `session` stops the streams
```
In the tests, `start` uses `Box::new(Passthrough::new(48_000))` and `Selection { input: DeviceRef { id: String::new(), name: "No Such Device 7f3a".into() }, ..Default::default() }`. cpal's WASAPI host initialises COM as MTA on this fresh thread itself, so no `windows` calls are needed.

## Appendix C — inspect_model (step 5)
```rust
/// What import needs to know about a generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelMeta { pub sample_rate: Option<u32>, pub f0: bool }

/// Opens `path` on the CPU, checks the RVC v2 signature and reads the `sr` metadata.
pub fn inspect_model(path: &Path) -> Result<ModelMeta> {
    let (session, _) = runtime::open_session(path, Provider::Cpu)?;
    let (inputs, outputs) = io_specs(&session);
    validate_signature(&inputs, &outputs)?;
    let sr = session.metadata().map_err(inference)?.custom("sr");
    let sample_rate = match sr.as_deref().and_then(|m| m.trim().parse::<u32>().ok()) {
        Some(sr) => Some(resolve_sr(None, Some(sr))?),
        None => None,
    };
    // validate_signature already rejects models without F0.
    Ok(ModelMeta { sample_rate, f0: true })
}
```
In the tests, `std::fs::write(&path, tiny(...))` writes the model before `inspect_model(&path)` runs, and `std::fs::remove_file` removes it afterwards.

## Appendix D — vc-models (steps 7–8)
`fsutil.rs`:
```rust
/// Writes `bytes` to `path` through `path.tmp` and a rename, creating the parent folder.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>
/// Renames `path` to `path.bad` (replacing an older backup) and returns the backup path.
pub fn back_up_bad(path: &Path) -> io::Result<PathBuf>
```
`library.rs`:
```rust
pub const INDEX_FILE: &str = "library.toml";
pub const INDEX_VERSION: u32 = 1;
const ICONS: &str = "icons";
pub const PITCH_RANGE: RangeInclusive<i32> = -24..=24;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String, pub name: String, pub file: String, pub sample_rate: u32,
    #[serde(default = "yes")] pub f0: bool,
    #[serde(default)] pub default_pitch: i32,
    #[serde(default)] pub icon: Option<String>,
}
#[derive(Serialize, Deserialize)] struct Index { #[serde(default)] version: u32, #[serde(default)] models: Vec<ModelEntry> }

/// A model copied into the library folder but not indexed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedModel { pub id: String, pub name: String, pub file: String }

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{0}")] Io(#[from] std::io::Error),
    #[error("no model with id \"{0}\"")] NotFound(String),
    #[error("the icon must be a PNG or JPEG image")] IconType,
    #[error("the name cannot be empty")] EmptyName,
}

pub struct Library { root: PathBuf, entries: Vec<ModelEntry> }
impl Library {
    /// Reads `root/library.toml`. If it is missing, the library is empty. If it is corrupt or from a newer version, it is renamed to
    /// `library.toml.bad` and the library is empty, with a note. Model files are never touched.
    pub fn open(root: PathBuf) -> (Self, Option<String>);
    pub fn root(&self) -> &Path;
    pub fn entries(&self) -> &[ModelEntry];
    pub fn get(&self, id: &str) -> Option<&ModelEntry>;
    pub fn ids(&self) -> Vec<String>;
    pub fn model_path(&self, e: &ModelEntry) -> PathBuf;            // root/file
    pub fn icon_path(&self, e: &ModelEntry) -> Option<PathBuf>;     // root/icons/icon
    pub fn file_exists(&self, e: &ModelEntry) -> bool;
    pub fn add(&mut self, staged: StagedModel, sample_rate: u32, f0: bool) -> Result<ModelEntry, LibraryError>; // appended at the end, default_pitch 0
    pub fn import(&mut self, src: &Path, sample_rate: u32, f0: bool) -> Result<ModelEntry, LibraryError>;     // copy_model + add
    // step 8
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), LibraryError>;
    pub fn remove(&mut self, id: &str) -> Result<(), LibraryError>;
    pub fn move_up(&mut self, id: &str) -> Result<(), LibraryError>;
    pub fn move_down(&mut self, id: &str) -> Result<(), LibraryError>;
    pub fn set_default_pitch(&mut self, id: &str, pitch: i32) -> Result<(), LibraryError>;
    pub fn set_icon(&mut self, id: &str, src: &Path) -> Result<(), LibraryError>;
}
/// Copies `src` into `root` as `<id>.onnx`. `id` is `slug(stem)`, made unique against `taken` and the files that already exist
/// (`base`, `base-2`, `base-3`, …). `name` is the file stem. A failed copy leaves no file behind.
pub fn copy_model(root: &Path, src: &Path, taken: &[String]) -> Result<StagedModel, LibraryError>;
/// Deletes a staged file that was never added.
pub fn discard_staged(root: &Path, staged: &StagedModel);
/// Lowercase ASCII letters and digits; other runs become one `-`, which is trimmed at the ends; empty gives "model". "Ana v2" gives "ana-v2".
pub fn slug(name: &str) -> String;
```
- Notes:
  - `The model library index was unreadable (saved as library.toml.bad); the library starts empty. Model files were kept.`
  - For a newer version: `The model library was saved by a newer VoiceChanger (saved as library.toml.bad); the library starts empty. Model files were kept.`
- The index is written with `toml::to_string_pretty(&Index { version: INDEX_VERSION, models })` and then `write_atomic`.
- `TempDir(PathBuf)` creates `temp_dir()/vc-models-test-{pid}-{name}`, fresh, and removes it in `Drop`.

## Appendix E — paths and config (step 9)
```rust
// paths.rs
pub const APP_NAME: &str = "VoiceChanger";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDirs { pub config: PathBuf, pub models: PathBuf, pub logs: PathBuf }
impl AppDirs {
    /// roaming/VoiceChanger/{config,models}, local/VoiceChanger/logs
    pub fn under(roaming: &Path, local: &Path) -> Self;
    /// %APPDATA% and %LOCALAPPDATA% (dirs::config_dir / dirs::data_local_dir; the temp folder if unknown).
    pub fn system() -> Self;
    pub fn settings_file(&self) -> PathBuf; // config/settings.toml
}
```
```rust
// config.rs
pub const CONFIG_VERSION: u32 = 1;
pub const SAVE_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct Config {
    pub version: u32,                         // default CONFIG_VERSION
    pub devices: DevicesConfig,
    pub perf: PerfConfig,
    pub voice: VoiceSettings,
    pub last_model: Option<String>,
    pub pitch_by_model: BTreeMap<String, i32>, // a map from id to pitch; BTreeMap so the file is written in a stable order
    pub hotkey: Option<String>,
    pub start_minimised: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct DevicesConfig { pub input: Option<SavedDevice>, pub cable: Option<SavedDevice>, pub monitor: Option<SavedDevice>, pub monitor_enabled: bool, pub monitor_db: f32 /* DEFAULT_MONITOR_DB */ }
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)] #[serde(default)]
pub struct SavedDevice { pub id: String, pub name: String }   // From<&DeviceRef> and Into<DeviceRef>
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)] #[serde(default)]
pub struct PerfConfig { pub provider: ProviderChoice, pub gpu: i32, pub block_ms: u32, pub context_ms: u32, pub crossfade_ms: u32 } // default from PerfSettings::default()
/// The parts of the config that live outside AppState.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs { pub last_model: Option<String>, pub pitch_by_model: BTreeMap<String, i32>, pub hotkey: Option<String>, pub start_minimised: bool }
impl Prefs { pub fn from_config(c: &Config) -> Self }

/// A missing file gives the defaults with no note. A file that is unreadable, corrupt or from a newer version gives the defaults and a note;
/// corrupt and newer files are renamed to settings.toml.bad.
pub fn load(path: &Path) -> (Config, Option<String>);
impl Config { pub fn save(&self, path: &Path) -> io::Result<()> } // toml::to_string_pretty + write_atomic

pub struct SaveTracker { saved: Config, pending: Option<(Config, Instant)> }
impl SaveTracker {
    pub fn new(saved: Config) -> Self;
    /// True when `current` differs from the saved copy and has not changed for SAVE_DELAY.
    pub fn poll(&mut self, current: &Config, now: Instant) -> bool;
    pub fn mark_saved(&mut self, saved: Config);
    pub fn is_pending(&self) -> bool;
    pub fn needs_save(&self, current: &Config) -> bool;
}
```
- Notes:
  - `Settings could not be read ({e}); using defaults.`
  - `The settings file was unreadable (saved as settings.toml.bad); using defaults.`
  - `The settings were saved by a newer VoiceChanger (saved as settings.toml.bad); using defaults.`
- `poll` works like this:
  1. If `current == saved`, it clears `pending` and returns false.
  2. If `pending` is not `current`, it sets `pending = (current, now)`.
  3. It returns `now - since >= SAVE_DELAY`.
- The `save_tracker_debounces` timeline (`t0 = Instant::now()`):
  1. Unchanged → false.
  2. Change A at t0 → false.
  3. A at t0+0.5 s → false.
  4. Change B at t0+0.8 s → false.
  5. B at t0+1.5 s → false.
  6. B at t0+1.8 s → true.
  7. `mark_saved(B)`, then B at t0+3 s → false.

## Appendix F — files and import (step 12)
```rust
// files.rs
/// Native picker on a "vc-file-dialog" thread; None when cancelled; a spawn failure gives a closed channel.
pub fn pick_file(title: &'static str, filter: &'static str, exts: &'static [&'static str]) -> Receiver<Option<PathBuf>>;
/// Opens `dir` in Explorer (`explorer.exe <dir>`), creating it first; errors are logged.
pub fn open_folder(dir: &Path);
```
```rust
// import.rs
pub type InspectFn = Arc<dyn Fn(&Path) -> Result<ModelMeta, String> + Send + Sync>;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction { Import(PathBuf), OfferConvert(PathBuf), Reject(String) }
/// .onnx (any case) → Import; .pth/.zip → OfferConvert; anything else →
/// Reject("Could not import \"{file}\": only RVC v2 .onnx models can be imported here.")
pub fn classify_drop(path: &Path) -> DropAction;
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice { OfferConvert(PathBuf), Error(String), Info(String) }
/// "\"{file}\" is a PyTorch model. Convert it to ONNX first."
pub fn offer_convert_text(path: &Path) -> String;

enum Step { Idle, Inspecting { src: PathBuf, rx: Receiver<Result<ModelMeta, String>> }, NeedRate { src: PathBuf, f0: bool },
            Copying { src: PathBuf, rx: Receiver<Result<StagedModel, String>>, sample_rate: u32, f0: bool } }
pub struct Importer { inspect: InspectFn, queue: VecDeque<PathBuf>, step: Step }
impl Importer {
    pub fn new(inspect: InspectFn) -> Self;
    pub fn enqueue(&mut self, src: PathBuf);
    pub fn is_busy(&self) -> bool;                 // not Idle, or the queue is not empty
    pub fn needs_rate(&self) -> Option<&Path>;
    pub fn choose_rate(&mut self, sample_rate: u32, library: &Library); // starts Copying
    pub fn cancel_rate(&mut self);                  // drops the file, goes Idle
    /// Advances; returns Ok(id) per import and Err(message) per failure.
    pub fn poll(&mut self, library: &mut Library) -> Vec<Result<String, String>>;
}
```
- Inspection and copying run on `vc-import` threads. The copy thread runs `copy_model(root, src, &taken)`, with `taken = library.ids()` captured at start.
- Messages:
  - inspect error: `Could not import "{file}": {e}` (for example `…: unsupported model: fp16 export`);
  - copy error: `Could not import "{file}": {io error}`;
  - `add` error: the same text, after `discard_staged`;
  - disconnect: `Could not import "{file}": the import stopped unexpectedly`.
- Only one file is in flight at a time.
- In the tests, the fake `InspectFn` closures return a fixed `ModelMeta` or error, and `drive()` polls every 10 ms for up to 5 s until `!is_busy() || needs_rate().is_some()`.
- Models tab sample-rate prompt: `"{file}" does not say its sample rate. Pick it:` with [32 kHz] [40 kHz] [48 kHz] [Cancel].

## Appendix G — switching (step 13)
```rust
// switch.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchPlan { LoadAll, Swap, SwapWithRestart }
pub fn plan_switch(has_pipeline: bool, running: bool) -> SwitchPlan; // !has → LoadAll; running → SwapWithRestart; else Swap
/// The saved pitch for `id`, else the model's default.
pub fn pitch_for(id: &str, saved: &BTreeMap<String, i32>, default_pitch: i32) -> i32;
pub struct PendingSwitch { pub id: String, pub name: String, pub rx: Receiver<LoadResult<Generator>> }
```
```rust
// app.rs
pub(crate) fn select_model(&mut self, id: &str) {
    let Some(entry) = self.library.get(id).cloned() else { return };
    if !self.library.file_exists(&entry) { self.banner = Some(format!("The model file for \"{}\" is missing.", entry.name)); return; }
    if self.prefs.last_model.as_deref() == Some(id) && self.model.info().is_some() { self.pending = None; return; } // already active
    let (path, perf) = (self.library.model_path(&entry), self.state.perf);
    match plan_switch(self.model.info().is_some(), self.is_running()) {
        SwitchPlan::LoadAll => {
            self.pending = None;
            self.model = match model::spawn_load(entry.name.clone(), path, Some(entry.sample_rate), perf.provider(), perf.stream) {
                Ok(rx) => ModelSlot::loading(entry.name.clone(), rx), Err(e) => ModelSlot::Failed(e.to_string()) };
            self.activate(&entry);
        }
        SwitchPlan::Swap | SwitchPlan::SwapWithRestart => match model::spawn_generator_load(entry.name.clone(), path, entry.sample_rate, perf.provider(), perf.stream) {
            Ok(rx) => self.pending = Some(PendingSwitch { id: entry.id.clone(), name: entry.name.clone(), rx }), // the newest request wins
            Err(e) => self.banner = Some(format!("Could not switch to \"{}\": {e}", entry.name)),
        },
    }
}
fn activate(&mut self, entry: &ModelEntry) { self.prefs.last_model = Some(entry.id.clone()); self.state.voice.pitch = pitch_for(&entry.id, &self.prefs.pitch_by_model, entry.default_pitch); }
```
- `poll_switch` reads `pending.rx.try_recv()`:
  - **Empty:** return.
  - **Disconnected:** the error is "the model loader stopped unexpectedly".
  - **`Err(msg)`:** banner `Could not switch to "{name}": {msg}`. The old voice keeps playing.
  - **`Ok((generator, info))`:**
    1. `let running = self.is_running(); if running { self.stop() }`.
    2. If `self.model.ready_mut()` is `Some((p, slot_info))`, then `drop(p.swap_generator(generator)); *slot_info = info;` and `activate(&entry)` (look up `entry` again by id).
    3. Otherwise the banner is "Could not switch: the model was unloaded."
    4. `if running { self.start() }`.
- `pitch_changed()` runs `if let Some(id) = &self.prefs.last_model { self.prefs.pitch_by_model.insert(id.clone(), self.state.voice.pitch); }`.
- `spawn_generator_load` runs `Generator::load(&path, provider, Some(sample_rate))`, then `warm_up(frames_for(settings.window_16k()))`, then sends `ModelInfo { name, sample_rate: g.sample_rate(), requested: provider, active: g.active_provider() }`.
- `ModelSlot::ready_mut(&mut self) -> Option<(&mut P, &mut ModelInfo)>` returns `Some` only for `Ready`.

## Appendix H — Models tab (step 14)
```rust
#[derive(Default)]
pub struct ModelsUi { pub renaming: Option<(String, String)>, pub confirm_delete: Option<String>, pub icon_pick: Option<(String, Receiver<Option<PathBuf>>)> }
/// Why the model cannot be deleted now.
pub fn delete_blocked(is_active: bool, running: bool) -> Option<&'static str>; // active && running → "Stop the voice before deleting the active model."
/// The first letter or digit, upper-cased; "?" when there is none.
pub fn avatar_letter(name: &str) -> String;
```
- `VcApp.models_ui: ModelsUi`.
- The icon URI is `format!("file://{}", path.display())`. After `set_icon`, call `ctx.forget_image(&uri)`.
- The delete confirmation reads `Delete "{name}"? This removes the model file.` with [Delete] [Cancel].
- Library errors become `Notice::Error(format!("Could not {action} \"{name}\": {e}"))`.

## Appendix I — hotkey (step 15)
```rust
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo { pub ctrl: bool, pub alt: bool, pub shift: bool, pub key: egui::Key }
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]   // thiserror = "2" is added to the app
pub enum ComboError {
    #[error("Press a key combination.")] Empty,
    #[error("Add Ctrl, Alt or Shift to the key.")] NeedsModifier,
    #[error("Unknown key \"{0}\".")] UnknownKey(String),
}
/// The keys that can be used: A–Z, 0–9 (egui Num0–Num9 → Digit0–Digit9), F1–F12, Space, Insert, Delete, Home, End, PageUp, PageDown,
/// and the arrows (named "Up", "Down", "Left", "Right"). Escape is not in the list: it cancels recording.
const KEYS: &[(egui::Key, Code, &str)];
pub fn key_code(key: egui::Key) -> Option<Code>;
impl Combo {
    pub fn new(mods: egui::Modifiers, key: egui::Key) -> Result<Self, ComboError>; // ctrl || alt || shift required
    /// "Ctrl+Alt+V"; modifier names are case-insensitive; the key comes last.
    pub fn parse(s: &str) -> Result<Self, ComboError>;
    pub fn hotkey(&self) -> HotKey;  // HotKey::new(Some(mods), code)
}
impl std::fmt::Display for Combo { /* "Ctrl+Alt+Shift+V", in that order */ }
pub enum RecordOutcome { Cancel, Combo(Combo), Invalid(String) }
pub fn record_key(key: egui::Key, mods: egui::Modifiers) -> RecordOutcome; // Escape → Cancel

pub struct Hotkeys { manager: Option<GlobalHotKeyManager>, current: Option<HotKey>, pressed: Arc<AtomicBool> }
impl Hotkeys {
    /// Creates the manager on the UI thread. The event handler sets `pressed` and calls ctx.request_repaint() on HotKeyState::Pressed.
    pub fn new(ctx: &egui::Context) -> Self;
    /// Unregisters the current hotkey and registers `combo`. On failure nothing is registered and the error is
    /// "{combo} is already used by another app." (or "Global hotkeys are unavailable." without a manager).
    pub fn set(&mut self, combo: Option<Combo>) -> Result<(), String>;
    pub fn take_pressed(&self) -> bool; // swap(false)
}
```
- The handler is installed with `GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| …))`.
- `about_lines(dirs: &AppDirs) -> Vec<(&'static str, String)>` (step 16) returns, in order:
  - Version: `env!("CARGO_PKG_VERSION")`
  - Licence: `env!("CARGO_PKG_LICENSE")`
  - Repository: `env!("CARGO_PKG_REPOSITORY")`
  - Models folder, Settings folder, Logs folder.

  The About window shows "VoiceChanger" as its heading, renders the Repository line as `ui.hyperlink`, and shows the rest as labels.

## Final verification
1. Run the gate: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo build --release`. All must be green, and the test count must be the 230 baseline plus the new tests.
2. If `VC_TEST_MODEL` is set: `cargo test -p vc-engine --test e2e -- --ignored`.
3. Manual checks with `cargo run --release -p voicechanger`:
   - Drop an `.onnx` while running; audio keeps working.
   - A `.pth` leads to Open in Convert, which shows the file.
   - Switch between a 40 kHz and a 48 kHz model while running; the new voice arrives in about 1 s or less.
   - Restart; devices, the last model, pitch per model and controls come back.
   - Unplug the saved mic and launch; a note appears on the banner and in Settings.
   - Set the hotkey and use it while minimised; a clashing combination shows an error.
   - A log file exists, and "Open logs folder" opens it.
   - Start minimised works.
   - About shows MIT and the folders.
