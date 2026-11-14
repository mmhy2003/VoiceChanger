# VC-116 plan: app logo and icon

## Goal
Use the attached neon logo as VoiceChanger's icon everywhere Windows shows one (the .exe, the window, the taskbar, Alt-Tab, shortcuts, the installer, Apps & features) and put it at the top of the README.

## Architecture
The logo is committed once as `assets/logo/voicechanger.png`. A new `cargo xtask gen-icon` command trims the logo's transparent margin, centres it on a square canvas, scales it with Lanczos3 to 9 sizes and writes `assets/icon.ico` (PNG-compressed entries, in a container the command writes itself) and `assets/icon-256.png`. Both outputs are committed, and a test fails if they drift from the source. `crates/app/build.rs` embeds the `.ico` in the exe through winresource. `main.rs` sets the 256 px PNG as the window icon. `installer/setup.iss` uses the `.ico` for Setup.exe.

## File map
| File | Change | Responsibility |
|---|---|---|
| `xtask/Cargo.toml` | modify | add `image = { version = "0.25", default-features = false, features = ["png"] }` (same version as the app crate, so `Cargo.lock` gains no new crate) |
| `xtask/src/icon.rs` | new | `build(source_png) -> Result<Icons, String>`, trim/square, ICO writer, unit tests and the drift test |
| `xtask/src/main.rs` | modify | `mod icon;`, the `gen-icon` command, `USAGE` |
| `assets/logo/voicechanger.png` | new | the source logo, byte-for-byte from the task attachment |
| `assets/icon.ico`, `assets/icon-256.png` | new, generated | written by `cargo xtask gen-icon` |
| `crates/app/build.rs` | modify | `set_icon`, a missing-file check, `rerun-if-changed`, module doc |
| `crates/app/src/main.rs` | modify | `app_icon()`, `.with_icon(app_icon())`, a test |
| `installer/setup.iss` | modify | `SetupIconFile` |
| `README.md`, `CHANGELOG.md` | modify | logo header, Building note, changelog entry |

## Reference code

### `xtask/src/icon.rs` (step 1)
```rust
//! Generates the app icon (`assets/icon.ico`, `assets/icon-256.png`) from the committed logo.

use std::io::Cursor;

use image::imageops::{self, FilterType};
use image::{ImageFormat, RgbaImage};

/// The committed source logo, relative to the workspace root.
pub const SOURCE: &str = "assets/logo/voicechanger.png";
/// The generated multi-size icon, relative to the workspace root.
pub const ICO: &str = "assets/icon.ico";
/// The generated 256 px PNG (window icon and README), relative to the workspace root.
pub const PNG256: &str = "assets/icon-256.png";
/// The sizes stored in the `.ico`, smallest first.
pub const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

/// The generated icon files.
pub struct Icons {
    pub ico: Vec<u8>,
    pub png256: Vec<u8>,
}

/// Builds the icons from the PNG bytes of the source logo.
pub fn build(source_png: &[u8]) -> Result<Icons, String> {
    if image::guess_format(source_png).ok() != Some(ImageFormat::Png) {
        return Err("is not a PNG".into());
    }
    let img = image::load_from_memory_with_format(source_png, ImageFormat::Png)
        .map_err(|e| format!("is not a valid PNG: {e}"))?
        .into_rgba8();
    let (w, h) = img.dimensions();
    if w < 256 || h < 256 {
        return Err(format!("is {w}x{h} px; it must be at least 256x256"));
    }
    if img.pixels().all(|p| p[3] == 255) {
        return Err("has no transparent pixels; the icon needs a transparent background".into());
    }
    let square = square(&img).ok_or("is fully transparent")?;
    let mut images = Vec::with_capacity(SIZES.len());
    for size in SIZES {
        let scaled = imageops::resize(&square, size, size, FilterType::Lanczos3);
        images.push((size, encode_png(&scaled)?));
    }
    let png256 = images.last().map(|(_, png)| png.clone()).ok_or("no sizes")?;
    Ok(Icons { ico: ico(&images), png256 })
}

/// Crops the transparent margin and centres the artwork on a transparent square canvas.
/// `None` when no pixel is visible.
fn square(img: &RgbaImage) -> Option<RgbaImage> {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[3] > 0 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX {
        return None;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let side = w.max(h);
    let art = imageops::crop_imm(img, x0, y0, w, h).to_image();
    let mut canvas = RgbaImage::new(side, side);
    imageops::overlay(&mut canvas, &art, i64::from((side - w) / 2), i64::from((side - h) / 2));
    Some(canvas)
}

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| format!("encoding PNG: {e}"))?;
    Ok(out)
}

/// An ICO container holding PNG-compressed 32-bit images: a 6-byte header, one 16-byte
/// directory entry per image, then the PNG data.
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let count = u16::try_from(images.len()).expect("fewer than 65536 icon sizes");
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (size, png) in images {
        let dim = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        out.extend_from_slice(&[dim, dim, 0, 0]); // width, height, palette, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += png.len();
    }
    for (_, png) in images {
        out.extend_from_slice(png);
    }
    out
}
```
If clippy objects to the `as` casts (`cast_possible_truncation` is not in the default `clippy::all`, so it shouldn't), use `u8::try_from` / `u32::try_from(...).expect(...)` instead.

### Step 1 tests (in `icon.rs`, `#[cfg(test)] mod tests { use super::*; ... }`)
```rust
/// Encodes a w x h image that is transparent except for an opaque red rectangle.
fn png_with_rect(w: u32, h: u32, rect: Option<(u32, u32, u32, u32)>) -> Vec<u8> {
    let mut img = RgbaImage::new(w, h);
    if let Some((x, y, rw, rh)) = rect {
        for py in y..y + rh {
            for px in x..x + rw {
                img.put_pixel(px, py, image::Rgba([255, 0, 0, 255]));
            }
        }
    }
    encode_png(&img).unwrap()
}

/// Splits an ICO into (declared size, PNG bytes) per directory entry.
fn parse_ico(ico: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes(ico[i..i + 4].try_into().unwrap());
    assert_eq!((u16_at(0), u16_at(2)), (0, 1), "ICO header");
    (0..usize::from(u16_at(4)))
        .map(|n| {
            let e = 6 + 16 * n;
            assert_eq!(ico[e], ico[e + 1], "square entry");
            assert_eq!((u16_at(e + 4), u16_at(e + 6)), (1, 32), "planes and bpp");
            let size = if ico[e] == 0 { 256 } else { u32::from(ico[e]) };
            let (len, off) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
            (size, ico[off..off + len].to_vec())
        })
        .collect()
}
```
Tests:
- `ico_has_every_size_in_order`: `build(&png_with_rect(300, 300, Some((50, 50, 200, 200))))`. `parse_ico` gives sizes equal to `SIZES`. Each entry: `image::guess_format == Png`, decodes to `ColorType::Rgba8`, and its dimensions equal `(size, size)`. `icons.png256` equals the 256 entry's bytes.
- `trims_margin_and_centres_artwork`: rect `(10, 20, 100, 50)` in 300x300. Decode `png256` to rgba8: pixel (128,128) is red with alpha 255; (128,10) and (128,245) have alpha 0 (the band above and below the centred 2:1 art); (3,128) and (252,128) have alpha > 0 (the art fills the full width after trimming).
- `rejects_opaque_source`: an all-opaque 300x300 image fails with an error containing `"transparent"`.
- `rejects_small_source`: 100x100 with a rect fails with an error containing `"256"`.
- `rejects_non_png`: `build(b"not a png")` fails with an error containing `"PNG"`.
- `rejects_fully_transparent_source`: `png_with_rect(300, 300, None)` fails with an error containing `"fully transparent"`.

### `xtask/src/main.rs` (step 1)
```rust
const USAGE: &str = "usage: cargo xtask <fetch-models|fetch-test-models|gen-onnx|gen-icon>";
// match arm:
Some("gen-icon") => gen_icon(),

/// Regenerates `assets/icon.ico` and `assets/icon-256.png` from `assets/logo/voicechanger.png`.
fn gen_icon() -> Result<(), String> {
    let root = root()?;
    let source = root.join(icon::SOURCE);
    let bytes = std::fs::read(&source).map_err(|e| format!("{}: {e}", source.display()))?;
    let icons = icon::build(&bytes).map_err(|e| format!("{}: {e}", source.display()))?;
    for (rel, data) in [(icon::ICO, &icons.ico), (icon::PNG256, &icons.png256)] {
        let dest = root.join(rel);
        std::fs::write(&dest, data).map_err(|e| format!("{}: {e}", dest.display()))?;
        println!("wrote {}", dest.display());
    }
    Ok(())
}
```

### Drift test (step 2, added to the `icon.rs` tests)
```rust
#[test]
fn generated_icons_match_committed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let source = std::fs::read(root.join(SOURCE)).expect("assets/logo/voicechanger.png is missing");
    let icons = build(&source).unwrap();
    for (rel, generated) in [(ICO, &icons.ico), (PNG256, &icons.png256)] {
        let committed = std::fs::read(root.join(rel))
            .unwrap_or_else(|_| panic!("{rel} is missing; run `cargo xtask gen-icon`"));
        assert!(*generated == committed, "{rel} is out of date; run `cargo xtask gen-icon`");
    }
}
```

### `crates/app/build.rs` (step 3)
Module doc: `//! Embeds the Windows resources: the app icon (assets/icon.ico) and the version resource, FileVersion/ProductVersion X.Y.Z.0 from the workspace version.`
At the start of `main`:
```rust
let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
let icon = std::path::Path::new(&manifest).join("../../assets/icon.ico");
assert!(icon.is_file(), "{} is missing; run `cargo xtask gen-icon`", icon.display());
println!("cargo:rerun-if-changed=build.rs");
println!("cargo:rerun-if-changed=../../assets/icon.ico");
```
Then `res.set_icon(icon.to_str().expect("icon path is UTF-8"))` chained before the existing `.set(...)` calls. A version bump still reruns the script, because the package version is part of the unit's metadata hash.

### `crates/app/src/main.rs` (step 3)
```rust
/// The window and taskbar icon, generated by `cargo xtask gen-icon`.
fn app_icon() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png"))
        .expect("assets/icon-256.png is a valid PNG")
}
```
Add `.with_icon(app_icon())` to the `ViewportBuilder` chain. Test at the end of the file:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_icon_is_256_rgba() {
        let icon = app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
```

## Steps

### 1. xtask: `gen-icon` generator with unit tests
See step detail.

### 2. Commit the logo and the generated icons, with a drift guard
See step detail.

### 3. Embed the icon in the exe and set the window icon
See step detail.

### 4. Installer icon, README and CHANGELOG
See step detail.

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace            # CI sets VC_REQUIRE_ASSETS=1 after cargo xtask fetch-models
cargo deny check
cargo build --release
cargo xtask gen-icon; git status --porcelain assets   # prints nothing: the outputs are reproducible
```
CI also builds the installer with `SetupIconFile` and runs `installer\test-install.ps1`. Open the PR against `develop`.
