# VC-150: In-app help (Help window + hover tips)

## Goal
Every control in VoiceChanger explains itself: hovering it shows a short tip with its range and default, and a Help window holds the full guide (how to get started, what each option does, what to change when something sounds wrong). A new user sees the guide once, by itself, on a fresh install.

Mockup v2 ("D final: Help window + hover tips") shows the result and is part of this design.

## Decisions
- **Option D: Help window + hover tips**, chosen by the human from four mockups. Rejected: A, a Help tab (help and setting are never visible together); B, the Help window alone (no hint on the control itself); C, hover tips and ? popups alone (no home for Getting started and Troubleshooting, and hover is easy to miss).
- **The Help window opens by itself once, on a fresh install only**, chosen by the human. "Fresh install" means the app starts and `settings.toml` does not exist yet. People who update from an earlier version never see it open by itself. Rejected: once for everyone (needs a new saved setting), and never.
- **One set of texts.** A tip is the first part of its guide entry. Ranges and defaults are not typed into the texts: they are formatted from the same constants the sliders use, so help and behaviour cannot drift apart.
- **Texts live in the Rust source**, as plain text. No markdown renderer, no new dependency, no bundled font. The existing glyph guard (`crates/app/src/glyphs.rs`) then checks every character of the help automatically.
- No always-visible help text is added to the tabs: the Settings tab has no scroll area and already fills the default 720 x 520 window.

## Architecture
A new module `crates/app/src/help/`, built like `update/` (data and state apart from drawing):

**`help/mod.rs`: data model and state, no egui.**
- `Topic`: `Start, Voice, Models, Convert, Devices, Performance, General, Troubleshooting`, with `ALL`, `title()` and `Topic::for_tab(Tab)` (Voice > Voice, Models > Models, Convert > Convert, Settings > Devices).
- `Id`: one variant per explained control, with `ALL` (list below).
- `Item { id: Option<Id>, name, short, more }`. `short` is the tip; `short` + `more` is the guide entry. Getting started (numbered steps) and Troubleshooting (problem, then fix) use the same shape with `id: None`, so one renderer draws every topic.
- `items(topic) -> &'static [Item]`, `item(id) -> &'static Item`, `range_text(id) -> Option<String>` (for example `50 to 1000 ms · default 250`).
- `HelpState { open, topic }` with `at_startup(first_run)`, `toggle(tab)` (closes if open; otherwise opens on the tab's topic) and `open_on(topic)`.

**`help/content.rs`: the texts**, one static table per topic. The texts in mockup v2 are the starting point.

**`help/ui.rs`: drawing.**
- `tip(response, id) -> Response`: attaches the tip (short text, then the range line in weak text, at most about 300 points wide) with both `on_hover_ui` and `on_disabled_hover_ui`, so greyed-out controls (Pitch and Noise gate with the original voice, Provider while running) explain themselves too.
- `heading(ui, help, text, topic)`: the heading followed by a small `?` button that calls `open_on(topic)`.
- `tab_bar_button(ui, help, tab)`: `? Help`, right-aligned in the tab bar; calls `toggle(tab)`.
- `window(ctx, help)`: `egui::Window::new("Help")`, not collapsible, movable, resizable, closed by its title-bar button. Topics are a wrapped row of selectable labels; below it a vertical scroll area (one per topic, so each topic keeps its place) lists the items: name in strong text, range, short text, `more` in weak text. It opens at the right edge under the tab bar. It takes `&mut HelpState` only, so it can be drawn in a test without the app.

**Existing files touched**
- `main.rs`: `mod help;`, and `first_run` (no `settings.toml` yet), read before `config::load` and passed in `Startup`.
- `app.rs`: `VcApp.help: HelpState` and `Startup.first_run`. `VcApp::new` uses `HelpState::at_startup(first_run)` and, on a first run, writes the settings file straight away so the next start is no longer a first run. `draw` adds the tab-bar button, calls `help::ui::window`, and handles **F1** (no modifiers, ignored while a hotkey is being recorded) as `toggle(tab)`.
- `tabs/voice.rs`, `tabs/models.rs`, `tabs/convert.rs`, `tabs/settings.rs`, `update/ui.rs` (the update rows in Settings): each control's response goes through `help::ui::tip`; headings become `help::ui::heading`. In grids, both the row label and the widget carry the tip.
- `status_bar.rs`: each part carries an optional `Id`; `show` attaches the tip.
- Slider ranges that are typed in the UI today move to named constants shared with `range_text`: Pitch uses `vc_models::PITCH_RANGE`, Noise gate, Block, Extra context and Crossfade use the engine's constants, and new constants cover the gains (-20 to +20 dB), Monitor volume (-40 to 0 dB) and GPU (0 to 7). No range or default changes.
- `CHANGELOG.md` (`[Unreleased]` > `### Added`) and a short Help paragraph in `README.md`.

**Entries (`Id`)**
| Topic | Entries |
|---|---|
| Voice | Model, Start/Stop, Bypass, Pitch, Input gain, Output gain, Noise gate, Meters, and the status bar's Latency, Inference, late blocks, underruns |
| Models | Import, Open folder, reorder buttons, model name (click to use), Default pitch, Icon/Rename/Delete |
| Convert | Choose file, the Name/Default pitch/Icon form, Convert, the buttons shown after converting |
| Audio devices | Microphone, Virtual mic, Monitor, Volume, Refresh |
| Performance | Provider, GPU, Block, Extra context, Crossfade |
| General | Hotkey, Start in the tray, the update rows, Open logs folder, About |

The model name and the Convert button are two entries more than mockup v2 draws. Plain Cancel, Dismiss, Save and Clear buttons get no tip.

**Rules for the texts**
- Plain words, short sentences, and "raise it when / lower it when" advice for every value.
- Every factual statement must be backed by the code. Checked so far: the Input meter is measured after Input gain; Crossfade adds its length to the delay; Latency turns orange above 300 ms and Inference above 60% of a block; pitch is remembered per model; Bypass is never saved; changing Block, Extra context or Crossfade while running restarts the audio. Advice that the code cannot back is left out.
- Characters: ASCII punctuation plus the symbols the app already draws. The glyph guard fails the build on anything egui's fonts lack (an arrow, for example).

## Data and interfaces
- No new setting and no change to `settings.toml`. The Help window's state (open, topic, position) is not saved.
- `Startup` gains `first_run: bool`.
- `status_bar` parts gain an optional help `Id`; their texts do not change.

## Error handling and edge cases
- First run with nothing changed: the settings file is still written at startup, so Help opens by itself only once. If that write fails (already logged today), Help opens again on the next start.
- A corrupt `settings.toml` is not a first run. Uninstalling while keeping user data and installing again is not a first run either.
- `--self-test` and a second launch never reach the window, so neither is affected.
- F1 while Settings is recording a hotkey belongs to the recording, not to Help.
- In the tray nothing is drawn, so Help neither opens nor reacts there; it is still open when the window comes back.
- Small window (minimum 520 x 400): egui keeps the Help window inside the app window; it can be moved, resized or closed.
- Help, About and Update available can be open together; each closes on its own.

## Testing
All automated in `cargo test --workspace`; no manual checks.
- **Content**: every `Id` has exactly one item; every topic has items; no empty name or short text; short texts stay under a length limit.
- **Ranges**: `range_text` gives the expected text for every entry that has a range, built from the real constants and defaults.
- **Coverage**: a source scan in the style of the glyph guard: every `Id` variant is used in at least one UI file outside `help/`, so no entry is left without its tip.
- **State**: `HelpState::at_startup`, `toggle` and `open_on`; `Topic::for_tab` for every tab.
- **First run**: the check is true without a settings file and false once one is saved (temp folder).
- **Drawing**: one headless egui frame per topic with the window open, and `tip` on an enabled and on a disabled widget, without a panic.
- **Status bar**: the parts carry the right entries, for a converted voice and for the original voice; the existing text tests stay as they are.
- **Glyph guard**: the existing test covers the new texts with no change.
- The quality gate stays green: fmt, clippy with `-D warnings`, tests, `cargo deny check`, release build.

## Out of scope
- Translations, search inside the help, pictures or screenshots, links to a website or online manual.
- A Help item in the tray menu or in the installer.
- Remembering the Help window's position, size or last topic between runs.
- A scroll area or any other layout change on the tabs.
- Changing what any setting does, its range or its default.
- Custom fonts or a markdown renderer.