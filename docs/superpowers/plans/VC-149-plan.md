# VC-149 plan: fix the reorder arrow buttons on the Models tab

## Goal
The Models tab's reorder buttons show `⏶`/`⏷` instead of missing-glyph squares, and a test fails whenever any non-comment source text uses a character that egui's built-in fonts cannot draw.

## Architecture
A new test-only module `crates/app/src/glyphs.rs` scans every `.rs` file under `crates/*/src` for non-ASCII characters on non-comment lines. It asks egui 0.32.3's default fonts (`egui::Context::default()` after one `ctx.run`, then `ctx.fonts(|f| f.has_glyph(&font_id, c))`) whether each one has a glyph in both the Proportional and the Monospace family. The fix itself is two string literals in `crates/app/src/tabs/models.rs`, plus a replacement for any other character the guard reports.

## Facts checked (branch `megoo/VC-149-fix-arrow-buttons`, at `f5801eb`)
- egui/epaint is **0.32.3** (Cargo.lock). The app crate imports egui as `use eframe::egui;`, and its package name is `voicechanger`.
- `epaint::text::Fonts::has_glyph(&self, font_id: &FontId, c: char) -> bool` exists (epaint `src/text/fonts.rs:538`). It returns false when the glyph would be the replacement glyph.
- `egui::Context::fonts` panics with "No fonts available until first call to Context::run()", so the test must run one pass first.
- `crates/app/src/main.rs` declares `#[cfg(test)] mod testutil;` at lines 19–20. Put `#[cfg(test)] mod glyphs;` directly before it, so the declarations stay in alphabetical order.
- The workspace crates are `crates/{app,audio,convert,engine,models}`, and `env!("CARGO_MANIFEST_DIR")` of the app crate is `crates/app`.
- The buttons are at `crates/app/src/tabs/models.rs:110` (`egui::Button::new("▲")`) and `:116` (`egui::Button::new("▼")`).
- The close button is at `crates/app/src/update/ui.rs:19` (`ui.small_button("✕")`). Its doc comment, at line 9, reads `/// One line above the tabs while an update is offered, with View… and ✕.`
- Non-ASCII characters on non-comment lines today: `▲ ▼` (models.rs), `■ ▶` (tabs/voice.rs:35,39), `⚠` (tabs/settings.rs:106,296,361), `★` (settings.rs:317), `✕` (update/ui.rs:19), `∞` (widgets.rs:41), `…` (many), `·` (many), `–` (status_bar.rs:33,160), `→` (engine/src/dsp/resample.rs:20), `é` (app.rs:1461-1462, tray.rs:342-344, convert/src/pickle.rs:870-871), `É` (models/src/library.rs:431).
- `CHANGELOG.md` has an empty `## [Unreleased]` section at line 8.
- Workspace lints: `clippy::all = warn`, and CI runs with `-D warnings`.

## File map
| File | Change |
|---|---|
| `crates/app/src/glyphs.rs` | **new**, test-only: `non_ascii`, `rust_files`, `default_fonts`, `has_glyph`, and the tests |
| `crates/app/src/main.rs` | add `#[cfg(test)] mod glyphs;` |
| `crates/app/src/tabs/models.rs` | `"▲"` → `"⏶"`, `"▼"` → `"⏷"` |
| `crates/app/src/update/ui.rs` | only if the guard reports `✕`: replacement character, and the doc comment on line 9 updated |
| other files reported by the guard | nearest look-alike that passes (see step 2) |
| `CHANGELOG.md` | `### Fixed` entry under `[Unreleased]` |

## Reference code for `crates/app/src/glyphs.rs`
```rust
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
        [egui::FontId::proportional(14.0), egui::FontId::monospace(14.0)]
            .iter()
            .all(|id| f.has_glyph(id, c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_reports_line_numbers() {
        assert_eq!(non_ascii("a\nb ▲ c\n\"é…\""), vec![(2, '▲'), (3, 'é'), (3, '…')]);
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
        assert!(missing.is_empty(), "no glyph in egui's fonts:\n{}", missing.join("\n"));
    }
}
```
Note: the `tests` submodule inside an already `#[cfg(test)]` module follows the crate's usual `mod tests` layout. The helper functions live outside it so that the module's doc comment describes them. If clippy objects to the nested `#[cfg(test)]` (`clippy::all` does not include a lint for that), keep it as is. Run `cargo fmt` after pasting the code.

## Steps

### 1. Add the glyph scanner and its control tests
Create `crates/app/src/glyphs.rs` with the reference code above, but **without** the `ui_symbols_have_glyphs` test for now. That test goes in step 2, so this step stays green. Declare `#[cfg(test)] mod glyphs;` in `crates/app/src/main.rs`, directly before `#[cfg(test)] mod testutil;`.
- First write the three tests with `non_ascii`, `default_fonts` and `has_glyph` stubbed as `todo!()`. Run `cargo test -p voicechanger glyphs`: the tests fail with panics at `todo!()`.
- Then implement the functions as in the reference. `rust_files` is used only from step 2 on: add it in step 2 as well, so no dead-code warning appears.
- Run `cargo test -p voicechanger glyphs`: 3 tests pass. Then run `cargo clippy -p voicechanger --all-targets --all-features -- -D warnings` and `cargo fmt --all -- --check`.
- Commit: `test(app): VC-149 glyph scanner for egui's built-in fonts`.

### 2. Guard every UI symbol and fix the reorder arrows (plus anything else reported)
- Add `rust_files` and `ui_symbols_have_glyphs` from the reference to `crates/app/src/glyphs.rs`.
- Run `cargo test -p voicechanger ui_symbols_have_glyphs`. It **must fail** and list at least `app/src/tabs/models.rs:110 '▲' U+25B2` and `app/src/tabs/models.rs:116 '▼' U+25BC`. Write down every line it reports.
- Fix the arrows in `crates/app/src/tabs/models.rs`, function `rows`: `egui::Button::new("▲")` → `egui::Button::new("⏶")` and `egui::Button::new("▼")` → `egui::Button::new("⏷")`. Change nothing else: not the enabled conditions, the actions or the order.
- For each other reported character, use the first candidate that passes the guard:
  - `✕` (update/ui.rs:19): `✖`, then `×`. Also update the doc comment at update/ui.rs:9 to name the new character.
  - `→`: `⇒`, then `->`.
  - `–`: `-`.
  - `…`: `...`.
  - `·`: `•`, then `-`.
  - `∞`: `inf`.
  - `⚠`: `!`.
  - `★`: `*`.
  - `■`: `◼`, then `Stop` without a symbol.
  - `▶`: `⏵`, then `Start` without a symbol.
  - `é`/`É` in test strings: `ü`/`Ü`. These strings test non-ASCII handling, so they must stay non-ASCII.
  - When you replace a character in a test-expected string, update every assertion that uses it, so the tests still match.
- Run `cargo test -p voicechanger glyphs`: 4 tests pass. Then run `cargo test --workspace`, clippy with `-D warnings`, and `cargo fmt --all -- --check`.
- Commit: `fix(app): VC-149 reorder buttons use ⏶ ⏷, guarded by a glyph test`. Name any other replacement in the commit body.

### 3. Changelog entry
In `CHANGELOG.md`, under `## [Unreleased]` (line 8), add:
```
### Fixed
- Models tab: the reorder buttons show up and down triangles again instead of empty squares.
```
If step 2 replaced other symbols, add one bullet that names each one, e.g. `- The update banner's close button shows ✖ instead of an empty square.`. Run `cargo fmt --all -- --check` and `cargo test --workspace`. Commit: `docs: VC-149 changelog`.

## Final verification
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test --workspace` (in CI with `VC_REQUIRE_ASSETS=1`; locally, 3 vc-convert asset tests need `cargo xtask fetch-models`)
- `cargo deny check`
- `cargo test -p voicechanger glyphs` shows 4 passing tests, including `ui_symbols_have_glyphs`.
- `git grep -n -e '▲' -e '▼' -- crates` finds nothing.
