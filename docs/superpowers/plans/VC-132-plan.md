# VC-132 plan: show device names next to the type

## Goal
Every audio device is named `Type (Device name)`, for example `Microphone (Realtek(R) Audio)`, everywhere the app names a device.

## Architecture
cpal 0.17.3 splits a device's friendly name into `description().name()` (the type, "Microphone") and `description().driver()` (the device's own name, "Realtek(R) Audio"). A new pure function `full_name` in `crates/audio/src/devices.rs` joins the two. The crate-internal helper `device_name(&cpal::Device)` calls it, and both the device list (`device_info`) and the session's fallback name match (`session::find`) use that helper, so the lists and the session always use the same text. Everything downstream (Settings combos, ★ mark, reconcile notes, banners, Start errors, saved settings) already works on `name` and needs no change.

## Facts checked in the code
- `crates/audio/src/devices.rs:345-349`: `device_info` builds the name with `d.description().ok()?.name().to_string()`.
- `crates/audio/src/session.rs:93`: `find` matches by `d.description().is_ok_and(|desc| desc.name() == r.name)`. `session.rs` imports `crate::devices::{DeviceRef, Selection}` at line 10.
- `crates/audio/src/devices.rs` tests module starts at line 351. Its constants (`MIC = "Microphone (Realtek(R) Audio)"`, `SPEAKERS`, …) already use full names, so the existing tests stay unchanged.
- `CHANGELOG.md`: `## [Unreleased]` (line 8) has only `### Added`. A new `### Changed` section goes after it, before `## [1.1.0]`.
- `crates/app/src/tabs/settings.rs` needs no change.

## File map
| File | Change | Responsibility |
|---|---|---|
| `crates/audio/src/devices.rs` | modify | `full_name`, `device_name`, `device_info` uses `device_name`, new unit tests |
| `crates/audio/src/session.rs` | modify | `find` fallback compares `device_name(d)` |
| `CHANGELOG.md` | modify | `### Changed` entry under `[Unreleased]` |

## Code contract

In `crates/audio/src/devices.rs`, replace `device_info` (lines 344-349) with:

```rust
/// `"{name} ({device})"`, as Windows Sound settings shows it, e.g. `Microphone (Realtek(R) Audio)`.
/// Just `name` when `device` is missing, blank or already part of `name`.
fn full_name(name: &str, device: Option<&str>) -> String {
    match device.map(str::trim) {
        Some(device) if !device.is_empty() && !name.contains(device) => {
            format!("{name} ({device})")
        }
        _ => name.to_string(),
    }
}

/// The device's full name (type and device name); `None` if its description cannot be read.
pub(crate) fn device_name(d: &cpal::Device) -> Option<String> {
    let desc = d.description().ok()?;
    Some(full_name(desc.name(), desc.driver()))
}

/// The device's ID (empty if it cannot be read) and name; `None` without a name.
fn device_info(d: &cpal::Device) -> Option<DeviceInfo> {
    let name = device_name(d)?;
    let id = d.id().map(|i| i.to_string()).unwrap_or_default();
    Some(DeviceInfo { id, name })
}
```

`desc.driver()` returns `Option<&str>` in cpal 0.17.3 (per the design). If the compiler reports a different `Option` type (e.g. `Option<&String>` or `Option<String>`), adapt the call with `.map(String::as_str)` or `.as_deref()`; do not change `full_name`'s signature.

In `crates/audio/src/session.rs`, change line 10 to `use crate::devices::{DeviceRef, Selection, device_name};` (rustfmt decides the final order) and line 93 to:

```rust
        .and_then(|mut ds| ds.find(|d| device_name(d).is_some_and(|n| n == r.name)))
```

New tests, appended inside `mod tests` in `devices.rs` (before `list_devices_does_not_panic`):

```rust
    #[test]
    fn full_name_joins_type_and_device() {
        assert_eq!(full_name("Microphone", Some("Realtek(R) Audio")), MIC);
        assert_eq!(full_name("Speakers", Some("Realtek(R) Audio")), SPEAKERS);
    }

    #[test]
    fn full_name_without_device_is_the_type() {
        assert_eq!(full_name("Microphone", None), "Microphone");
        assert_eq!(full_name("Microphone", Some("")), "Microphone");
        assert_eq!(full_name("Microphone", Some("  ")), "Microphone");
    }

    #[test]
    fn full_name_keeps_a_name_that_already_has_the_device() {
        assert_eq!(full_name(MIC, Some("Realtek(R) Audio")), MIC);
    }

    #[test]
    fn full_name_lets_virtual_cables_be_recognised() {
        assert!(is_virtual_cable(&full_name("Line 1", Some("Virtual Audio Cable"))));
        assert!(!is_virtual_cable(&full_name("Speakers", Some("Realtek(R) Audio"))));
    }
```

## Steps

### 1. Name devices "Type (Device name)" in the list and the session
- **Failing tests first**: add the four tests from "Code contract" to `mod tests` in `crates/audio/src/devices.rs`.
- **Run**: `cargo test -p vc-audio full_name` → fails to compile with `cannot find function full_name`.
- **Implement**: `full_name`, `device_name` and the new `device_info` in `devices.rs`, then the `session.rs` import and line 93 change, exactly as in "Code contract".
- **Run again**: `cargo test -p vc-audio` → all pass, including the existing tests (unchanged) and `list_devices_does_not_panic`.
- **Gate**: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
- **Commit**: `feat(audio): name devices "Type (Device name)" as Windows does (VC-132)` with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

### 2. CHANGELOG entry
- In `CHANGELOG.md`, after the `### Added` bullet under `## [Unreleased]` and before `## [1.1.0] - 2026-11-14`, add (with a blank line before it):
  ```
  ### Changed
  - Audio devices are named as in Windows, e.g. `Microphone (Realtek(R) Audio)`, in Settings and in messages, so two microphones can be told apart. Virtual Audio Cable lines are recognised as virtual cables again.
  ```
- No test (documentation only). Run the full gate below.
- **Commit**: `docs: device names in the changelog (VC-132)` with the same trailer.

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo build --release
```
`git diff main --stat` lists only `crates/audio/src/devices.rs`, `crates/audio/src/session.rs` and `CHANGELOG.md`. Never commit anything from `.megoocolab/` or `docs/`.
