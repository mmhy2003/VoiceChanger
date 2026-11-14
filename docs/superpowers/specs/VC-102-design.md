# VC-102: Rename the Audio devices row labels in Settings

## Goal
The **Audio devices** grid on the Settings tab gets new row labels: **Microphone**, **Virtual mic** and **Monitor**. Nothing else on the screen changes.

## Decisions
- **No Status column.** You decided this: "discard the status column, just fix the field names." The status function, its tests and the question about what "Working" means are all dropped.
- **Only the labels change.** The grid stays at 2 columns. The combo widths stay as they are. The cable combo keeps showing `★ {name} (recommended)`. The monitor checkbox ("Monitor (hear yourself)"), its combo and the volume slider stay where they are. The input level meter, Refresh, the VB-CABLE warning, feedback warnings, notes and the start blocker are untouched.
- **Mapping:**
  - Input → **Microphone**
  - Cable output → **Virtual mic**
  - Monitor stays **Monitor**

## Architecture
One file changes: `crates/app/src/tabs/settings.rs`, in `show()`.
- `ui.label("Input")` (line 26) becomes `ui.label("Microphone")`.
- `ui.label("Cable output")` (line 46) becomes `ui.label("Virtual mic")`.

Changes outside the code:
- `CHANGELOG.md`: add a `### Changed` entry under `[Unreleased]`, for example "Settings → Audio devices: the rows are now labelled Microphone, Virtual mic and Monitor."

## Testing
- These are two string literals inside egui drawing code. No pure function is involved, so no new unit test is added. Testing the labels would mean pulling them out into constants just to assert on them.
- The CI gate must pass: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo build --release`.

## Out of scope
- The Status column and any device-health display.
- Wider combos and showing plain device names in the cable combo.
- Moving the monitor checkbox or the volume slider.
- Other texts that still use the old words. You can ask for any of these in this task:
  - the Refresh reconcile notes in `vc-audio` (`Input "…" is no longer available…`, `Cable output "…"`);
  - the start blocker ("Pick an input device / a cable output in Settings.");
  - the device-lost banner ("the input device …");
  - the Voice tab "Input" level meter.
