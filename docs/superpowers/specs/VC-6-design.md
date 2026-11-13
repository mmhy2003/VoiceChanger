# VC-6 — M4: Model library and persisted settings (import, hot-switch, hotkey)

## Goal
The user builds a library of RVC v2 `.onnx` voices. They import voices by file picker or by drag-and-drop, then manage them on the Models tab. They can switch voices from the Voice tab, even while running, and hear the new voice within about 1 s. Everything is remembered between launches: devices (by Windows endpoint ID), the last model, the pitch for each model and every control. A global hotkey toggles Start/Stop. The app can start minimised, writes rotating log files and has an About box. The layout is mockup v2.

## Decisions
| Topic | Choice | Rejected alternatives |
|---|---|---|
| Drag-and-drop vs the COM conflict | **One dedicated audio-control thread** (multithreaded COM) owns every cpal call. This lets us turn window drag-and-drop back on. *(human)* | Import by file picker only, with drag-and-drop in a later task. |
| Switching models while running | Load and warm up only the new **generator** in the background while the old voice keeps playing, then do a **quick stop → swap → start**. The gap is about 0.1–0.3 s and any change of output rate is handled. *(human)* | Swapping live inside the worker: no gap, but it adds code to the real-time path. |
| `.pth`/`.zip` dropped or picked on the Models tab | A notice offers **Open in Convert**. Accepting switches to the Convert tab and fills in the file there. Convert shows "Converting PyTorch models arrives in M5". *(human)* | Show a message only. |
| Hotkey | **Off by default.** Settings → General has **Set…** (records the next key combination; at least one modifier is required, Esc cancels) and **Clear**. If the combination can't be registered, an error says so. *(human)* | A fixed Ctrl+Alt+V; a default combination the user can change. |
| Layout | Mockup v2 is approved. *(human)* | — |
| Settings format | **TOML.** The `toml` and `serde` crates are already in `Cargo.lock`. | JSON |
| Device IDs | **Upgrade cpal 0.16 → 0.17**, which has `DeviceTrait::id()`, `HostTrait::device_by_id` and `description()`. | Calling `IMMDevice::GetId` ourselves through the `windows` crate. |
| AppName | `VoiceChanger`, which matches the window title. | — |
| F0 | It is always "yes": the engine already rejects models without F0. So only the **sample rate** can be missing, and only that is asked. | — |

## Architecture

### `vc-audio`
- **cpal 0.17.** `DeviceList` entries become `DeviceInfo { id: String, name: String }` (the name comes from `description()`).
- `Selection` holds a `DeviceRef { id, name }` for the input, the cable and the monitor.
- Matching tries the ID first and falls back to the name. `reconcile` and `start_blocker` use this matching, and the existing notes say which saved device is missing and what replaced it (PRD §5.5).
- `AudioSession::start` opens devices with `device_by_id` and falls back to the name.
- **New `control.rs`: `AudioControl`.** It spawns the long-lived `vc-audio-ctl` thread. The UI sends it commands over an `mpsc` channel and reads the replies from a reply channel:
  - `List → DeviceList`
  - `Start { selection, processor } → Result<SessionInfo, (StartError, processor)>`
  - `Stop → Option<Box<dyn Processor>>`
  - `Shutdown`, which is sent on exit.

  The `AudioSession` and its `cpal::Stream`s live only on that thread. The UI never calls cpal, so `main.rs` turns `with_drag_and_drop` back on. The `Shared` atomics (meters, status, latency) are unchanged.

### `vc-engine`
- **`inspect_model(path) -> Result<ModelMeta { sample_rate: Option<u32>, f0: bool }>`.** It opens a session on the CPU, runs `validate_signature` and reads the `sr` metadata. It reuses the existing error texts ("fp16 export", "RVC v1 (256-dim)", …).
- **`Generator::warm_up(frames)`** runs one silent call.
- **`Pipeline::swap_generator(Generator) -> Generator`** replaces the generator and keeps ContentVec and RMVPE loaded. The generator is loaded with the entry's `sr_override` from the library.

### `vc-models` (currently empty)
**`Library`**: the model library under `%APPDATA%\VoiceChanger\models`.
- The index is `library.toml`. It holds an ordered `Vec<ModelEntry { id, name, file, sample_rate, f0, default_pitch, icon: Option<String> }>`.
- Operations:
  - `import(src, meta, sr)`: copies the file to a unique name.
  - `rename`, `remove` (deletes the model file and its icon), `move_up`/`move_down`, `set_default_pitch`.
  - `set_icon(src)`: copies a PNG or JPEG to `icons/<id>.<ext>`.
- Writes are atomic (temporary file, then rename).
- Pure filesystem code with no ort dependency. Dependencies: `serde` and `toml`.

### `app`
- **`paths.rs`** gives the config, models and logs folders, based on `dirs::config_dir()` and `dirs::data_local_dir()`.
- **`config.rs`**: `Config` (serde, `#[serde(default)]` on every field, with a `version` field), stored in `config\settings.toml`. It holds:
  - the devices (`DeviceRef`s, monitor on/off and volume);
  - `perf` (provider, GPU, block, context and crossfade);
  - `voice` (input and output gain, gate);
  - `last_model` (an ID) and `pitch_by_model: HashMap<id, i32>`;
  - `hotkey: Option<String>` and `start_minimised`.

  It is loaded before `run_native`, which needs `start_minimised` for `ViewportBuilder::with_minimized`. It is saved when the config differs from the last saved copy and has not changed for 1 s, and again in `on_exit`.
- **Model selection.** The Voice tab's "Model…" button becomes a drop-down of library models. The Models tab sets the active model by clicking a row.
  - When nothing is loaded: full `Pipeline::new`.
  - When a pipeline exists, a `vc-gen-loader` thread loads and warms up the new generator. When it is ready:
    - if the app is stopped, `swap_generator` runs directly;
    - if it is running, the app sends Stop to the audio-control thread, swaps the generator and sends Start.
  - The pitch becomes `pitch_by_model[id]`, else the model's `default_pitch`. Moving the slider updates `pitch_by_model`.
  - A provider or GPU change still reloads everything.
  - On launch, `last_model` loads in the background. The app never starts streaming by itself.
- **`tabs/models.rs`** follows mockup v2:
  - **Import…** opens the `rfd` file picker; dropped files arrive through `ctx.input(|i| i.raw.dropped_files)`.
  - Import runs on a background thread: `inspect_model`, then a notice asking for the sample rate (32/40/48 kHz) if it is missing, then `Library::import`.
  - Notices appear in the tab for a missing sample rate, the `.pth` hand-off and errors.
  - Each row has ▲▼, an icon (a letter avatar if there is none), a default-pitch field, Icon…, Rename and Delete (with confirmation). The active model is highlighted.
  - "Open folder" opens the library folder.
- **Icons** use `egui_extras` image loaders with the `image` crate (png and jpeg only).
- **`tabs/convert.rs`** shows `app.convert_file` and the M5 note.
- **Settings → General** has the hotkey recorder, Start minimised, Open logs folder (`explorer.exe <dir>`) and About.
  - About is an `egui::Window` with the name, the version, the licence (`CARGO_PKG_LICENSE`, MIT), a repository link (`CARGO_PKG_REPOSITORY`) and the models, config and logs folders.
- **`hotkey.rs`** wraps `global-hotkey`:
  - It parses and formats strings such as `"Ctrl+Alt+V"` and maps egui keys to `global_hotkey` codes.
  - `register`/`unregister` report clashes as errors.
  - Its event handler calls `ctx.request_repaint()`, so the hotkey works while the window is minimised.
  - Pressing the hotkey toggles Start/Stop. If Start is blocked (no model, or a device blocker), the banner shows why.
- **Logging (`main.rs`).** `tracing-appender` writes daily rotating files to `%LOCALAPPDATA%\VoiceChanger\logs\voicechanger.log.*`, keeping the last 7, through a non-blocking writer whose guard is held in `main`. Debug builds also log to stderr.

### New dependencies
All need a `cargo deny` check:
- `cpal 0.17`
- `serde` (derive) and `toml`
- `dirs`
- `global-hotkey`
- `tracing-appender`
- `egui_extras` and `image` (png and jpeg only)

## Data and interfaces
- `%APPDATA%\VoiceChanger\models\library.toml`, `*.onnx`, `icons\*`
- `%APPDATA%\VoiceChanger\config\settings.toml`
- `%LOCALAPPDATA%\VoiceChanger\logs\`
- A new `AudioControl` command/reply API, `vc_engine::inspect_model`, `Pipeline::swap_generator` and `Generator::warm_up`.

## Error handling and edge cases
- **Unsupported model:** "Could not import "x.onnx": unsupported model: <reason>". Nothing is copied.
- **Copy failure** (for example, the disk is full): the error is shown and any partial file is removed.
- **Bad config or index:**
  - A `settings.toml` that is corrupt or from a newer version is renamed to `settings.toml.bad`. The app uses defaults and shows a note.
  - The same applies to `library.toml`. Model files are never deleted automatically.
- **Index entry whose file is gone:** the row shows "file missing" and can only be deleted.
- **Deleting the active model:** blocked while running. When stopped, the model is unloaded.
- **Failed generator load during a switch:** the old voice keeps playing and the banner shows the error.
- **Second switch request while one is loading:** the newest request wins and the older result is dropped.
- **Saved device missing on launch:** falls back to the default, with a note in Settings and on the Voice banner.
- **Device change while running:** restarts the streams through `AudioControl` without reloading the model (this already works today and is kept).
- **Hotkey:**
  - A combination without a modifier is refused.
  - A registration failure keeps the hotkey off and shows "already used by another app".
  - A hotkey from the config that fails to register on launch shows the same note.
- **Audio-control thread dies:** shown as a banner. Start stays disabled.

## Testing
Tests are written first; the existing quality gate applies.
- **`vc-models`** (temporary folders): import copies the file and gives unique names; rename; remove deletes the model file and its icon; reorder at the edges; default pitch; icon copy; the index round-trips; a corrupt index is backed up; an entry whose file is missing.
- **`config`:** defaults; a round-trip; missing fields take their defaults; a corrupt file is backed up; the save trigger is debounced (a pure function of state and time).
- **`vc-audio` devices:** matching by ID with the name as fallback, `reconcile` notes and `start_blocker` with `DeviceRef`. **`AudioControl`:** listing goes through the thread without panicking; Stop with no session returns `None`; Shutdown joins the thread.
- **`vc-engine`:** `inspect_model` on the tiny ONNX models from `testutil`: valid; no `sr` gives `None`; fp16, v1 and no-pitch models are rejected. These are skipped when `onnxruntime.dll` is missing, as today.
- **Generator swap and warm-up** are tested on the tiny generator. **`Pipeline::swap_generator`** is tested in the e2e test, which needs the base models.
- **`hotkey`:** parse and format round-trip, the modifier is required, egui keys map to `global_hotkey` codes.
- **`app`:** a pure function chooses the pitch for a model (saved pitch, else the default); a pure function plans a switch (load everything, swap only, or swap with a restart); the file-drop classifier (`.onnx` → import, `.pth`/`.zip` → convert offer, anything else → error).
- **Manual checks:**
  - Drop an `.onnx` file while running and confirm audio keeps working.
  - Switch between a 40 kHz and a 48 kHz model while running; the new voice is heard in about 1 s or less.
  - Restart the app: devices, last model, pitch per model and controls come back.
  - Use the hotkey while the window is minimised.
  - A log file is created and "Open logs folder" opens it.
  - Start minimised works.
- Update the changelog under [Unreleased]/Added and the README.

## Out of scope
- Converting `.pth` files (M5).
- Models without F0.
- Live switching with no gap.
- Starting with Windows, and a tray icon.
- Feature-retrieval `.index` files.
- Migrating settings from older versions (this is the first config version).
- Speaker ID selection.
