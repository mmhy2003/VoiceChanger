# VC-102: Rename the Audio devices row labels in Settings — plan

## Goal
The Audio devices grid on the Settings tab shows the row labels **Microphone**, **Virtual mic** and **Monitor**.

## Architecture
This changes two string literals in `crates/app/src/tabs/settings.rs::show()` and adds a CHANGELOG entry. Layout, combos, the ★ label, the monitor checkbox and slider, the meter, Refresh, warnings, notes and the start blocker all stay as they are.

## File map
- `crates/app/src/tabs/settings.rs`, in `show()`:
  - line 26: `ui.label("Input");` becomes `ui.label("Microphone");`
  - line 46: `ui.label("Cable output");` becomes `ui.label("Virtual mic");`
  - line 62: `ui.label("Monitor");` stays as it is.
- `CHANGELOG.md`: add a `### Changed` bullet under `## [Unreleased]`. That section already exists at line 13, so append to it rather than adding a second heading.

Do **not** touch the other places that still say "Input" or "Cable output". They are out of scope:
- the reconcile notes in `crates/audio/src/devices.rs:206` and `:214`;
- the Voice tab meter in `crates/app/src/tabs/voice.rs:71`;
- `docs/`.

## Steps

### 1. Rename the Microphone and Virtual mic labels and add a CHANGELOG entry
- **No new test** (see the design). These are literals inside egui drawing code, with no pure function to test.
- **Code edits** in `crates/app/src/tabs/settings.rs`:
  - change `ui.label("Input");` (line 26) to `ui.label("Microphone");`
  - change `ui.label("Cable output");` (line 46) to `ui.label("Virtual mic");`
- **CHANGELOG.md**: under `## [Unreleased]` → the existing `### Changed` (after the `/RELAUNCH` bullet), add:
  `- Settings → Audio devices: the rows are now labelled Microphone, Virtual mic and Monitor.`
- **Check the edits**: `git grep -n 'ui.label' crates/app/src/tabs/settings.rs` should show `"Microphone"`, `"Virtual mic"` and `"Monitor"` among the first matches, with no `"Input"` and no `"Cable output"`.
- **Gate**: run the full gate under Final verification. It must pass with no new warnings.
- **Commit**: `feat(settings): label audio device rows Microphone, Virtual mic and Monitor (VC-102)`, ending with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never commit anything from `.megoocolab/`.

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo build --release
```
`git diff main --stat` should list only `crates/app/src/tabs/settings.rs` and `CHANGELOG.md`.
