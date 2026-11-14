# VC-132: Show device names next to the type

## Goal
Every audio device is named **"Type (Device name)"**, for example `Microphone (Realtek(R) Audio)`, instead of only `Microphone`. This is the same text Windows Sound settings shows, so two microphones can be told apart in the Settings lists. The same full name appears wherever the app names a device.

## Decisions
- **Format A, "Type (Device name)".** You picked this over "type, then the name in grey" and "device name first".
- **Everywhere, not only in the Settings lists.** You picked this. The device's `name` itself becomes the full text, so the three drop-downs, the Refresh notes, the Voice tab banner and the Start errors all show it. The rejected alternative (a second field shown only in the lists) needed extra plumbing through `DeviceRef` and the saved settings, and left the messages ambiguous.
- **Build the name from the two parts cpal gives.** In cpal 0.17.3 on Windows, `description().name()` is the short type ("Microphone") and `description().driver()` is the device's own name ("Realtek(R) Audio"). The app joins them as `{type} ({device})`. I rejected reading cpal's "extended" lines, because they are an unlabelled list and their order is not a promise.
- **The Settings layout does not change.** The labels, the `★ … (recommended)` mark, the monitor checkbox, the slider, the meter and Refresh stay as they are (mockup v2). The drop-downs only get wider with the longer text.

## Why the names are missing today
The code and its tests were written for full names such as `Microphone (Realtek(R) Audio)` and `Line 1 (Virtual Audio Cable)`. Since the move to cpal 0.17, `description().name()` returns only the type. Besides the unclear lists, this has one hidden effect: a Virtual Audio Cable line arrives as just `Line 1`, so `is_virtual_cable` does not recognise it and it is neither marked ★ nor pre-selected. Full names fix that as well.

## Architecture
All changes are in the `vc-audio` crate. `crates/app/src/tabs/settings.rs` needs no change, because it already draws `DeviceInfo.name` and `DeviceRef.name`.

- `crates/audio/src/devices.rs`
  - New pure function `full_name(name: &str, device: Option<&str>) -> String`:
    - returns `"{name} ({device})"` when `device` is present and not blank;
    - returns `name` unchanged when `device` is missing, blank, or already contained in `name` (cpal falls back to the full friendly name when Windows gives no type, and this avoids `Speakers (X) (X)`).
  - New crate-internal helper `device_name(d: &cpal::Device) -> Option<String>`: reads `d.description()` and returns `full_name(desc.name(), desc.driver())`.
  - `device_info` uses `device_name` instead of `description().name()`.
- `crates/audio/src/session.rs`
  - `find` (line 93), the fallback that matches a device by name when its ID is unknown, compares against `device_name(d)` instead of `desc.name()`. This keeps the list and the session using the same text.

Nothing else changes: `is_virtual_cable`, `feedback_warnings`, `reconcile`, `start_blocker`, `banner_text` and the saved-settings format (`SavedDevice { id, name }`) all work on `name` and pick up the full text automatically.

Outside the code: a `CHANGELOG.md` entry under `[Unreleased]` in a new `### Changed` section, for example "Audio devices are named as in Windows, e.g. `Microphone (Realtek(R) Audio)`, in Settings and in messages. Virtual Audio Cable lines are recognised as virtual cables again."

## Error handling and edge cases
- **Saved settings from an older version** hold the short name. Devices are matched by Windows endpoint ID first, so the selection is kept and its name is updated silently on the next start or Refresh.
- **Saved device without an ID** (the ID could not be read when it was saved): the short name no longer matches, so the device falls back to its default with the usual note ("… is no longer available; switched to …"). This is rare and needs one re-pick.
- **No device name from Windows**: the type alone is shown, as today.
- **A device without any name** is still left out of the list, as today.
- **Long names in a narrow window**: the drop-down grows with its text, as it already does for the cable row. No truncation is added.

## Testing
Unit tests in `crates/audio/src/devices.rs`, following the existing style:
- `full_name("Microphone", Some("Realtek(R) Audio"))` gives `Microphone (Realtek(R) Audio)`.
- `None`, `Some("")` and `Some("  ")` give `Microphone`.
- A name that already contains the device name is returned unchanged.
- `is_virtual_cable(&full_name("Line 1", Some("Virtual Audio Cable")))` is true, and it is false for `full_name("Speakers", Some("Realtek(R) Audio"))`.

The existing tests already use full names and must pass unchanged. `list_devices_does_not_panic` keeps covering the real enumeration. The full CI gate must be green: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo build --release`.

## Out of scope
- Any change to the Settings layout, labels or the `(recommended)` wording.
- Showing a GPU name next to the Provider in Performance.
- Truncating or wrapping long names.
- Migrating saved names in `settings.toml` (they update themselves through the ID match).
- The wording of notes and banners ("Input", "Cable output") beyond the device name inside them.