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
    let png256 = images
        .last()
        .map(|(_, png)| png.clone())
        .ok_or("no sizes")?;
    Ok(Icons {
        ico: ico(&images),
        png256,
    })
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
    imageops::overlay(
        &mut canvas,
        &art,
        i64::from((side - w) / 2),
        i64::from((side - h) / 2),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::ColorType;

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

    fn build_err(png: &[u8]) -> String {
        match build(png) {
            Ok(_) => panic!("build succeeded"),
            Err(e) => e,
        }
    }

    #[test]
    fn ico_has_every_size_in_order() {
        let icons = build(&png_with_rect(300, 300, Some((50, 50, 200, 200)))).unwrap();
        let entries = parse_ico(&icons.ico);
        let sizes: Vec<u32> = entries.iter().map(|(s, _)| *s).collect();
        assert_eq!(sizes, SIZES);
        for (size, png) in &entries {
            assert_eq!(image::guess_format(png).ok(), Some(ImageFormat::Png));
            let img = image::load_from_memory_with_format(png, ImageFormat::Png).unwrap();
            assert_eq!(img.color(), ColorType::Rgba8);
            assert_eq!((img.width(), img.height()), (*size, *size));
        }
        let (_, png256) = entries.iter().find(|(s, _)| *s == 256).unwrap();
        assert!(icons.png256 == *png256, "png256 is the 256 entry");
    }

    #[test]
    fn trims_margin_and_centres_artwork() {
        let icons = build(&png_with_rect(300, 300, Some((10, 20, 100, 50)))).unwrap();
        let img = image::load_from_memory_with_format(&icons.png256, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        let centre = img.get_pixel(128, 128);
        assert_eq!((centre[0], centre[3]), (255, 255), "centre is opaque red");
        assert_eq!(img.get_pixel(128, 10)[3], 0, "band above the art");
        assert_eq!(img.get_pixel(128, 245)[3], 0, "band below the art");
        assert!(img.get_pixel(3, 128)[3] > 0, "art reaches the left edge");
        assert!(img.get_pixel(252, 128)[3] > 0, "art reaches the right edge");
    }

    #[test]
    fn rejects_opaque_source() {
        let mut img = RgbaImage::new(300, 300);
        img.pixels_mut()
            .for_each(|p| *p = image::Rgba([0, 0, 255, 255]));
        let err = build_err(&encode_png(&img).unwrap());
        assert!(err.contains("transparent"), "{err}");
    }

    #[test]
    fn rejects_small_source() {
        let err = build_err(&png_with_rect(100, 100, Some((10, 10, 50, 50))));
        assert!(err.contains("256"), "{err}");
    }

    #[test]
    fn rejects_non_png() {
        let err = build_err(b"not a png");
        assert!(err.contains("PNG"), "{err}");
    }

    #[test]
    fn rejects_fully_transparent_source() {
        let err = build_err(&png_with_rect(300, 300, None));
        assert!(err.contains("fully transparent"), "{err}");
    }
}
