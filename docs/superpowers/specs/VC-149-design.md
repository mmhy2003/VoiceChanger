# VC-149: Fix the reorder arrow buttons on the Models tab

## Goal
The two reorder buttons at the start of each row on the Models tab show a solid up triangle and a solid down triangle instead of empty squares. A test stops this kind of bug (a symbol the font cannot draw) from coming back anywhere in the app.

## Cause
`crates/app/src/tabs/models.rs` (lines 110 and 116) labels the buttons `▲` (U+25B2) and `▼` (U+25BC). The app uses egui 0.32.3 with its built-in fonts and never installs its own. Those fonts do not contain these two characters, so egui draws its replacement glyph `◻`, which is the square in the screenshot. The buttons themselves work: clicks, enabling and reordering are not affected.

## Decisions
- **Look: small solid triangles `⏶` (U+23F6) and `⏷` (U+23F7)**, chosen by the human. egui's built-in fonts contain them, and egui uses them itself as the symbols of the arrow keys. Mockup v2 shows the result.
- Rejected: arrows with a stem `⬆ ⬇` (heavier look), and triangles painted by the app without a font (about 25 lines of custom widget, and not checkable by a simple glyph test).
- **Scope: check every symbol, fix any that is missing**, chosen by the human. The test covers all non-ASCII characters in the workspace's source, and a symbol found missing is replaced in this task.
- No font is bundled and egui is not upgraded.

## Architecture
**1. The fix** in `crates/app/src/tabs/models.rs`, function `rows`: `egui::Button::new("▲")` becomes `egui::Button::new("⏶")`, and `"▼"` becomes `"⏷"`. Nothing else in the row changes (size, order, enabled rules, actions).

**2. A glyph guard**, a new test-only module `crates/app/src/glyphs.rs`, declared in `main.rs` as `#[cfg(test)] mod glyphs;` next to `testutil`. It has two parts:
- A pure function that takes the text of one source file and returns each non-ASCII character with its line number. Lines whose first non-blank characters are `//` (this includes `///` and `//!`) are skipped, so comments do not count.
- A test, `ui_symbols_have_glyphs`, that:
  - walks every `.rs` file under `crates/*/src` (from `env!("CARGO_MANIFEST_DIR")`), so texts from `vc-engine`, `vc-audio`, `vc-models` and `vc-convert` that can reach the UI as messages are covered too;
  - creates `egui::Context::default()`, runs one empty frame with `ctx.run` (fonts are valid only after that), and asks `ctx.fonts(|f| f.has_glyph(&font_id, c))` for the Proportional and the Monospace family;
  - fails with one line per missing character: `file:line 'c' U+XXXX`.

The guard scans the source instead of keeping a hand-written list of symbols, so a symbol added later is checked without anyone remembering to list it.

**3. Fixing whatever the guard finds.** Characters in use today outside comments: `⏶ ⏷` (new), `■ ▶` (Voice tab), `⚠ ★` (Settings), `✕` (update banner, `crates/app/src/update/ui.rs:19`), `∞` (level meters), `… · –`, `→` (one engine error text), and `é É` (test strings only).
- `■ ▶ ★ ∞ ⏶ ⏷` are in egui's documented glyph list, and egui uses `⚠` itself. They are expected to pass.
- `✕` could not be confirmed from the read-only checkout. If the guard reports it, the banner's close button gets the first of `✖`, `×` that passes, and the doc comment on `banner` is updated to match.
- Any other character reported gets the nearest look-alike that passes; the pull request lists each replacement.

## Data and interfaces
None. No settings, files or public interfaces change.

## Error handling and edge cases
- A single model: both buttons are disabled and show dimmed triangles (as in the screenshot, with triangles instead of squares).
- A model whose file is missing keeps both buttons disabled, as today.
- Names typed or imported by users (model names, device names) are not source text and are not covered; that is unchanged behaviour.
- A trailing comment with a non-ASCII character on a code line is checked like code. None exists today; if one is added with a character the font lacks, the test names the file and line.

## Testing
All automated, run by `cargo test --workspace` in CI; no manual checks.
- Unit test of the scanning function: it finds non-ASCII characters with the right line numbers, and it skips `//`, `///` and `//!` lines.
- `ui_symbols_have_glyphs` as described above. It is written first and must fail on the current code, naming `models.rs` with `▲` and `▼`; it passes after the fix.
- A control that proves the check can fail: `has_glyph` is false for a character no font provides (a supplementary private-use code point such as U+10FFFD).
- The existing quality gate stays green: `cargo fmt --all -- --check`, clippy with `-D warnings`, `cargo test --workspace`, `cargo deny check`.

## Changelog
Under `[Unreleased]`, a `### Fixed` entry: the reorder buttons on the Models tab show up and down triangles again instead of empty squares. If another symbol is replaced, the entry names it.

## Out of scope
- Bundling a font or changing egui's font setup.
- Upgrading egui.
- Tooltips, keyboard shortcuts or drag-and-drop for reordering.
- Any other change to the Models tab layout.
- The old design documents under `docs/` that mention `▲/▼`.