# VC-3 — M1: Audio skeleton (egui tabs, device selection, passthrough, meters)

## Goal
`voicechanger.exe` opens an egui window with Voice / Models / Convert / Settings tabs. In Settings the user picks a mic, a virtual cable output and an optional monitor. Pressing **Start** passes the raw mic through to the cable (and monitor), with live level meters. If a device is unplugged, audio stops cleanly and a message is shown. There is no voice model yet. This is the audio path that M2/M3 will plug inference into.

## Decisions
- **Monitor output is in M1** (your answer): a second playback device with on/off and volume, fed the same processed audio. This also makes the "cable and monitor are the same device" warning meaningful.
- **Processing worker thread** (your answer, option A): input callback → ring → worker (downmix, resample, meters) → one ring per output → output callbacks. M3 drops the RVC pipeline into the worker, so this path isn't rewritten later. Rejected: inline linear resampling in the callbacks (lower quality, rewritten in M3), and forcing 48 kHz (some devices wouldn't work).
- **Layout approved as mockup v1.**
- **No persistence in M1.** Device choices reset to defaults at each launch. Saved settings are M4 in the PRD.
- **Warnings are advisory.** Feedback warnings don't block Start; only a missing input or cable output disables Start.
- Devices are identified by name in M1, because `cpal` doesn't expose WASAPI endpoint IDs. Endpoint IDs come with persistence in M4.

## Architecture

### `crates/audio` (`vc-audio`): no UI code
- `devices.rs`
  - `list_devices() -> DeviceList { inputs, outputs, default_input, default_output }` uses the cpal WASAPI host. It holds names only.
  - `is_virtual_cable(name)` matches case-insensitively on `CABLE`, `VoiceMeeter` and `Virtual Audio Cable`.
  - Outputs are sorted with virtual cables first. The default cable output is the first cable found, otherwise the Windows default output.
  - `feedback_warnings(&Selection) -> Vec<Warning>`, a pure function returning two variants:
    - `MonitorSameAsCable`;
    - `InputIsCableRecordingSide`, when the input name contains `CABLE Output` or is any virtual cable's recording side.
- `ring.rs`: `ringbuf` (`HeapRb<f32>`) producers and consumers, preallocated at stream start (about 1 s of capacity each).
- `meter.rs`: `LevelMeter`, an `AtomicU32` holding f32 bits for peak and RMS per block. The audio threads write it with `Relaxed` stores; the UI reads it and applies its own decay.
- `worker.rs`
  - A thread that owns the input consumer and the output producers.
  - Loop:
    1. Pop the available frames.
    2. Downmix to mono f32.
    3. Update the input meter.
    4. Resample to each output's rate with `rubato`; there are at most two rates.
    5. Apply the monitor volume, an atomic read.
    6. Update the output meter.
    7. Push to each output ring.
  - It parks on a short sleep or condvar when the ring is empty and exits on a stop flag.
  - All buffers and resamplers are created before the loop. The processing step is a pure `process_block(&mut State, &[f32], ...)` so it can be tested without devices.
- `session.rs`
  - `AudioSession::start(Selection, Shared) -> Result<AudioSession, StartError>` opens 2 or 3 cpal streams. Each stream uses the device's default config, and samples are converted to f32 in the callback.
  - It then spawns the worker and plays the streams.
  - Dropping the session (or calling `stop()`) pauses and drops the streams, sets the stop flag and joins the worker.
  - The session lives on the UI thread, because cpal `Stream` is not `Send`.
- **Callbacks**
  - The input callback converts and interleaves into the input ring. If the ring is full it drops the samples and bumps an overflow counter.
  - The output callbacks pop from their ring, write each sample to all channels and fill any shortfall with silence (counting an underrun).
  - Callbacks never lock, allocate, log or block.
- **Errors**: each stream's error callback stores a `StreamStatus` (e.g. `DeviceLost { which: Input | Cable | Monitor }`) in an atomic. It does nothing else.

### `crates/app` (`voicechanger`)
- `main.rs` runs `eframe::run_native` with the title `VoiceChanger {version}`.
- `app.rs` holds `VcApp { tab, devices, selection, session: Option<AudioSession>, shared, banner }`.
- `tabs/voice.rs`:
  - Start/Stop button and input and output meters;
  - an error banner that can be dismissed;
  - a status line showing the rates and whether the monitor is on or off.
- `tabs/settings.rs`:
  - an input drop-down with its own live meter (shown while running);
  - a cable output drop-down with ★ recommended entries;
  - the monitor checkbox, drop-down and volume slider (-40 to 0 dB, default -6 dB);
  - a Refresh button;
  - the no-cable warning with a VB-CABLE link (`ctx.open_url`, only when clicked);
  - the feedback warnings.
- `tabs/models.rs` and `tabs/convert.rs`: placeholder text ("Coming in M4" / "Coming in M5").
- **Each frame**:
  - Poll `StreamStatus`. On `DeviceLost`, drop the session, set the banner ("Audio stopped: the input device "X" was disconnected…") and refresh the device list.
  - While running, `request_repaint_after(~33 ms)` so the meters animate.
- **Changing a device while running** restarts the session. Changing the monitor volume doesn't, because it is an atomic.

### Dependencies
- `eframe` and `egui` in the app.
- `cpal`, `ringbuf`, `rubato`, `thiserror` and `anyhow` in audio.
- egui's bundled fonts may need `OFL-1.1` and the Ubuntu font licence in `deny.toml`. If so, that is added with a reason in the PR, as the file requires.

## Data and interfaces
- `Selection { input: String, cable: String, monitor: Option<String>, monitor_enabled: bool }`.
- `Shared { input_meter, output_meter, monitor_gain: AtomicU32, status: AtomicU8, overflows, underruns }` is held in an `Arc` by the UI, the worker and the callbacks.
- `StartError` (thiserror) has these variants:
  - `DeviceNotFound(name)`
  - `UnsupportedConfig { device, reason }`
  - `BuildStream(..)`

  The UI shows these in the banner. No config files are added in M1.

## Error handling and edge cases
- Unplugging a device mid-run stops everything cleanly (no panic, worker joined) and shows a banner naming the device. Start stays usable once a valid device is picked.
- If a selected device disappears on Refresh, the selection falls back to the default and a note is shown.
- If there are no input devices or no output devices, Start is disabled with an explanation.
- If no virtual cable is found, the user sees a warning and a link, but can still choose any output (useful for testing on speakers).
- Device sample formats i16, u16 and f32 are all converted to f32. Any channel count is downmixed to mono on input and duplicated to every channel on output.
- Underruns are filled with silence and overflows drop samples. Both are counted (shown later in M3 stats). Neither ever blocks.
- Start errors appear in the banner and leave the app in the Stopped state.

## Testing
- **Unit tests in `vc-audio`** (run in CI, no hardware):
  - virtual cable detection and output ordering;
  - default selection;
  - `feedback_warnings` cases;
  - downmix;
  - meter peak and RMS;
  - `process_block` resampling 44.1 → 48 kHz, checking the length and that a sine stays a sine;
  - ring overflow and underrun behaviour.
- `cargo fmt`, `clippy -D warnings`, `cargo deny check` and the release build stay green.
- **Manual checklist in the PR** (Windows with VB-CABLE):
  - passthrough heard via the CABLE Output in Windows Sound "Listen" or Discord;
  - the monitor in headphones, with the volume working;
  - the meters move;
  - Refresh picks up a newly plugged USB mic;
  - unplugging the USB mic and the headphones mid-run shows the banner without a crash;
  - each warning appears in the right case.
- `CHANGELOG.md` gets an Unreleased "Added" line.

## Out of scope
- Saving devices or settings, and endpoint-ID matching (M4).
- Voice models, inference, pitch, gains and the noise gate (M2/M3).
- The latency and inference status bar beyond rates and monitor state (M3).
- The Performance and General settings groups, including the hotkey (M4).
- The Models and Convert tab contents.
