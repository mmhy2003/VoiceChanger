//! Test-only guard: every non-ASCII character in the workspace's source, outside
//! comment lines, has a glyph in egui's built-in fonts, so no label draws a square.

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
            non_ascii("a\nb ▲ c\n\"é…\""),
            vec![(2, '▲'), (3, 'é'), (3, '…')]
        );
    }

    #[test]
    fn non_ascii_skips_comment_lines() {
        let text = "// ▲\n    /// ▼\n//! →\n  // ·\nlet s = \"✕\"; // ★";
        assert_eq!(non_ascii(text), vec![(5, '✕'), (5, '★')]);
    }

    #[test]
    fn missing_glyph_is_detected() {
        let ctx = default_fonts();
        assert!(!has_glyph(&ctx, '\u{10FFFD}'));
        assert!(has_glyph(&ctx, 'A'));
    }
}
