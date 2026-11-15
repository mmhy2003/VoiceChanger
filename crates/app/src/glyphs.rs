//! Test-only guard: every non-ASCII character in the workspace's source, outside
//! comment lines, has a glyph in egui's built-in fonts, so no label draws a square.

use std::path::{Path, PathBuf};

use eframe::egui;

/// Each non-ASCII character in `text` with its 1-based line number. Lines whose
/// first non-blank characters are `//` (comments, `///` and `//!` docs) are skipped.
fn non_ascii(text: &str) -> Vec<(usize, char)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .flat_map(|(i, line)| {
            line.chars()
                .filter(|c| !c.is_ascii())
                .map(move |c| (i + 1, c))
        })
        .collect()
}

/// Every `.rs` file under `dir`, recursively, appended to `out`.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("read dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A context with egui's default fonts; fonts exist only after the first pass.
fn default_fonts() -> egui::Context {
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    ctx
}

/// True if both the Proportional and the Monospace family draw `c`.
fn has_glyph(ctx: &egui::Context, c: char) -> bool {
    ctx.fonts(|f| {
        [
            egui::FontId::proportional(14.0),
            egui::FontId::monospace(14.0),
        ]
        .iter()
        .all(|id| f.has_glyph(id, c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_reports_line_numbers() {
        assert_eq!(
            non_ascii("a\nb \u{25B2} c\n\"\u{E9}\u{2026}\""),
            vec![(2, '\u{25B2}'), (3, '\u{E9}'), (3, '\u{2026}')]
        );
    }

    #[test]
    fn non_ascii_skips_comment_lines() {
        let text = "// \u{25B2}\n    /// \u{25BC}\n//! \u{2192}\n  // \u{B7}\nlet s = \"\u{2715}\"; // \u{2605}";
        assert_eq!(non_ascii(text), vec![(5, '\u{2715}'), (5, '\u{2605}')]);
    }

    #[test]
    fn missing_glyph_is_detected() {
        let ctx = default_fonts();
        assert!(!has_glyph(&ctx, '\u{10FFFD}'));
        assert!(has_glyph(&ctx, 'A'));
    }

    #[test]
    fn ui_symbols_have_glyphs() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/ folder");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(crates).expect("read crates/") {
            let src = entry.expect("read dir entry").path().join("src");
            if src.is_dir() {
                rust_files(&src, &mut files);
            }
        }
        files.sort();
        assert!(files.len() > 20, "found only {} source files", files.len());
        let ctx = default_fonts();
        let mut missing = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("read source file");
            let rel = file.strip_prefix(crates).unwrap_or(file);
            for (line, c) in non_ascii(&text) {
                if !has_glyph(&ctx, c) {
                    missing.push(format!("{}:{line} '{c}' U+{:04X}", rel.display(), c as u32));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "no glyph in egui's fonts:
{}",
            missing.join(
                "
"
            )
        );
    }
}
