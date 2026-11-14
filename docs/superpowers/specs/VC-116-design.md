# VC-116: App logo and icon

## Goal
VoiceChanger gets the new neon logo (`voice-changer-logo.png`, attached to the task) as its icon in every place Windows shows one: the `.exe` in Explorer, the window title bar, the taskbar, Alt-Tab, Start menu and desktop shortcuts, the installer, and Apps & features. The README opens with the logo.

## Decisions
- **Full logo at every size**, high-quality downscale (Lanczos3). You chose this. The other options were a tighter crop of just the microphone and mask for sizes up to 32 px, or a hand-made small icon in a later task. The logo will be soft at 16–24 px; that's accepted.
- **One committed source, generated outputs committed too.** A new `cargo xtask gen-icon` creates the icon files from the source PNG, and they are committed. This is the same pattern as `cargo xtask gen-onnx`. Generating the icons in `build.rs` on every build was rejected: it adds image work and build dependencies to every build for files that almost never change. A hand-made `.ico` was rejected: it can quietly drift away from the logo.
- **Trim, then square.** The generator crops the transparent margin around the artwork, centres it on a square transparent canvas, then scales it. This makes the artwork as large as possible at small sizes.
- **No new image-encoding dependency.** The xtask uses the `image` crate the workspace already has (0.25, png feature). It writes the `.ico` container itself: a 6-byte header, one 16-byte directory entry per size, and PNG-compressed images. This is about 30 lines and is supported by Windows 10/11 and by `rc.exe`/`winresource`.

## Architecture
| Piece | Change |
|---|---|
| `assets/logo/voicechanger.png` | **New.** The source logo, downloaded byte-for-byte from the task attachment. |
| `xtask/src/icon.rs` + `main.rs` | **New** `gen-icon` command (added to `USAGE`). It reads the source and writes `assets/icon.ico` and `assets/icon-256.png`. The pure function `build(source_png) -> Result<Icons, String>` makes them testable. |
| `assets/icon.ico` | **Generated, committed.** Sizes 16, 20, 24, 32, 40, 48, 64, 128 and 256, all stored as 32-bit RGBA PNG. |
| `assets/icon-256.png` | **Generated, committed.** The 256 px PNG, used for the window icon and the README. |
| `crates/app/build.rs` | `res.set_icon("../../assets/icon.ico")` plus `cargo:rerun-if-changed`; the module doc mentions the icon. |
| `crates/app/src/main.rs` | `ViewportBuilder::with_icon(...)`. The icon data comes from `eframe::icon_data::from_png_bytes(include_bytes!(.../assets/icon-256.png))`. Without this call, eframe shows egui's own default icon. Loading it lives in a small `app_icon()` function so it can be tested. |
| `installer/setup.iss` | `SetupIconFile={#Root}\assets\icon.ico` for the Setup .exe. Shortcuts and `UninstallDisplayIcon` already point at `voicechanger.exe`, so they pick up the embedded icon automatically. |
| `README.md` | Centred logo above the title: `<p align="center"><img src="assets/icon-256.png" width="128" alt="VoiceChanger logo"></p>`. A short note under Building: "the icon is generated from `assets/logo/voicechanger.png` with `cargo xtask gen-icon`". |
| `CHANGELOG.md` | An entry under [Unreleased] → Added: "App logo and icon". |

## Error handling and edge cases
- **Source checks.** `gen-icon` fails with a clear message if the source is missing, isn't a PNG, is smaller than 256 px, or is fully transparent. The message names the path.
- **Transparency.** The task preview shows a black background, which may be transparency drawn as black. The generator checks for an alpha channel with transparent pixels. If the attached file turns out to be opaque black, the implementer stops and asks on the task: that would make the icon a black square, so they must not guess.
- **Build safety.** A broken committed icon can't ship silently: the tests below catch it, and `build.rs` fails the build if the `.ico` is missing.

## Testing (all automated, run in CI by `cargo test --workspace`)
- **xtask unit tests:**
  - `build` on a small synthetic RGBA PNG produces an ICO with exactly the 9 sizes in order, and each entry decodes to a square RGBA image of its declared size.
  - The transparent margin is trimmed and the artwork is centred.
  - An opaque source, a too-small source and a non-PNG source each give an error.
- **Drift guard:** a test regenerates the icons from `assets/logo/voicechanger.png` in memory and asserts the result is byte-identical to the committed `assets/icon.ico` and `assets/icon-256.png`. If someone changes the logo without running `gen-icon`, CI fails with "run `cargo xtask gen-icon`".
- **App test:** `app_icon()` decodes to 256×256 RGBA.
- **Release build:** the existing CI release build and the installer build cover the `build.rs` icon embedding and `SetupIconFile`. No manual steps.

## Out of scope
- A simplified hand-made small icon (possible follow-up).
- Installer wizard banner images (`WizardImageFile` / `WizardSmallImageFile`).
- Using the logo inside the app UI, such as an About box or the header.
- A tray icon (the app has none).