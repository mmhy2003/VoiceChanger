# VC-150 plan: in-app help (Help window + hover tips)

## Goal
Every control explains itself on hover (short text plus range and default), and a Help window holds the full guide. On a fresh install the guide opens once by itself.

## Architecture
A new module `crates/app/src/help/` is built like `update/`. `help/mod.rs` holds the data and state with no egui: `Topic`, `Id`, `Item`, `items`, `item`, `range_text` and `HelpState`. `help/content.rs` holds the static texts. `help/ui.rs` does the drawing: `tip`, `heading`, `tab_bar_button`, `window` and `f1_pressed`. Ranges and defaults in the help text are built from the same constants the sliders use. The tabs and the status bar run each control's `Response` through `help::ui::tip`. `main.rs` works out `first_run` before `config::load`. `VcApp::new` opens Help on a first run and writes `settings.toml` straight away.

## Facts checked in the code
- Package name `voicechanger` (`crates/app/Cargo.toml`), egui/eframe 0.32. To run one crate's tests: `cargo test -p voicechanger <filter>`.
- Slider ranges that are typed in the code today: `tabs/voice.rs` Pitch `-24..=24`, gains `-20.0..=20.0`, Noise gate `-90.0..=-20.0`. `tabs/settings.rs` Monitor volume `-40.0..=0.0`, GPU `DragValue .range(0..=7)`, Block/Extra context/Crossfade `50..=1000`, `0..=2000`, `10..=200`.
- Existing constants: `vc_models::PITCH_RANGE: RangeInclusive<i32> = -24..=24`, plus `vc_engine::stream::settings::{BLOCK_MS, CONTEXT_MS, CROSSFADE_MS}` (`RangeInclusive<u32>`) and `GATE_DB_RANGE` (`RangeInclusive<f32>`). Note that `vc_engine::stream::settings::PITCH_RANGE` is f32; use the `vc_models` one.
- Defaults: `VoiceSettings::default()` (`app.rs`) gives pitch 0, gains 0, gate -50. `StreamSettings::default()` gives 250 / 500 / 50 ms. `PerfSettings::default().gpu` is 0. `vc_audio::DEFAULT_MONITOR_DB` is -6. A new library entry has `default_pitch` 0, but that value is a literal in `vc_models`.
- `config::load` returns defaults with no note when the file is missing. It renames a corrupt or newer file to `.bad`, so `first_run` must be read before `load`.
- `VcApp::write_config(&mut self, cfg)` already saves the settings, logs a failure and marks the copy as saved.
- `VcApp::draw` (`app.rs` ~line 1032) draws the tab bar in `TopBottomPanel::top("tabs")`, then the status bar, the central panel and `update::ui::window`. It is not called while in the tray.
- `recording_hotkey: bool` lives on `VcApp`. Key recording reads `egui::Event::Key` in `tabs/settings.rs::general`.
- `status_bar::parts` returns `Vec<(String, bool)>` and `show` draws it. Its existing tests must stay unchanged.
- `widgets::meter(ui, label, shown)` returns nothing today.
- Glyph guard: `crates/app/src/glyphs.rs` is `#[cfg(test)]`, and its private `rust_files(dir, out)` walks the `.rs` files.
- The workspace clippy runs `all = warn` with `-D warnings`. A non-test item that nothing uses fails the build, so the steps are cut so that every new item is used in the step that adds it. The only exception is `Id::ALL`, which is `#[cfg(test)]`.
- Commit style: `feat(app): VC-150 ...`, `test(app): ...`, `docs: VC-150 ...`, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Don't touch `.megoocolab/` or `docs/`.

## File map
| File | Change | Responsibility |
|---|---|---|
| `crates/app/src/app.rs` | modify | `GAIN_DB_RANGE`, `MONITOR_DB_RANGE`, `GPU_RANGE`; `VcApp.help`, `Startup.first_run`, F1, tab-bar button, Help window, first-run save |
| `crates/app/src/help/mod.rs` | create | `Topic`, `Id`, `Item`, `items`, `item`, `range_text`, `HelpState`, tests (content, ranges, state, coverage) |
| `crates/app/src/help/content.rs` | create | the texts (Appendix A) |
| `crates/app/src/help/ui.rs` | create | `tip`, `heading`, `tab_bar_button`, `window`, `f1_pressed`, headless drawing tests |
| `crates/app/src/main.rs` | modify | `mod help;`, `first_run` into `Startup` |
| `crates/app/src/config.rs` | modify | `pub fn first_run(path) -> bool` and tests |
| `crates/app/src/tabs/voice.rs` | modify | constants, tips, heading |
| `crates/app/src/widgets.rs` | modify | `meter` returns the row's `Response` |
| `crates/app/src/status_bar.rs` | modify | each part carries `Option<Id>`; `show` attaches the tip |
| `crates/app/src/tabs/models.rs`, `tabs/convert.rs` | modify | tips, headings |
| `crates/app/src/tabs/settings.rs`, `update/ui.rs` | modify | constants (step 1), tips, headings |
| `crates/app/src/glyphs.rs` | modify | `rust_files` becomes `pub(crate)` for the coverage test |
| `CHANGELOG.md`, `README.md` | modify | Added entry, Help paragraph |

## Shared contracts (used by several steps)

```rust
// help/mod.rs
use std::ops::RangeInclusive;
use vc_engine::StreamSettings;
use vc_engine::stream::settings::{BLOCK_MS, CONTEXT_MS, CROSSFADE_MS, GATE_DB_RANGE};
use crate::app::{GAIN_DB_RANGE, GPU_RANGE, MONITOR_DB_RANGE, PerfSettings, Tab, VoiceSettings};

mod content;
pub mod ui;

/// The longest tip, in characters.
pub const SHORT_MAX: usize = 220;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic { Start, Voice, Models, Convert, Devices, Performance, General, Troubleshooting }

impl Topic {
    pub const ALL: [Topic; 8] = [Topic::Start, Topic::Voice, Topic::Models, Topic::Convert,
        Topic::Devices, Topic::Performance, Topic::General, Topic::Troubleshooting];
    /// "Getting started", "Voice", "Models", "Convert", "Audio devices", "Performance", "General", "Troubleshooting".
    pub fn title(self) -> &'static str { /* match */ }
    /// Voice > Voice, Models > Models, Convert > Convert, Settings > Devices.
    pub fn for_tab(tab: Tab) -> Topic { /* match */ }
}

/// One explained control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    // Voice tab and status bar
    Model, StartStop, Bypass, Pitch, InputGain, OutputGain, NoiseGate, Meters,
    Latency, Inference, LateBlocks, Underruns,
    // Models tab
    Import, OpenFolder, Reorder, ModelName, DefaultPitch, IconRenameDelete,
    // Convert tab
    ChooseFile, ConvertForm, ConvertButton, AfterConvert,
    // Settings > Audio devices
    Microphone, VirtualMic, Monitor, MonitorVolume, Refresh,
    // Settings > Performance
    Provider, Gpu, Block, ExtraContext, Crossfade,
    // Settings > General
    Hotkey, StartInTray, Updates, OpenLogs, About,
}

impl Id {
    /// Every entry, in the order above (37).
    #[cfg(test)]
    pub const ALL: [Id; 37] = [ /* all variants in declaration order */ ];
}

/// A guide entry. `short` is the tip; the guide shows `short` then `more`.
/// Getting started and Troubleshooting entries have `id: None`.
#[derive(Debug)]
pub struct Item {
    pub id: Option<Id>,
    pub name: &'static str,
    pub short: &'static str,
    pub more: &'static str,
}

/// The entries of `topic`, in display order.
pub fn items(topic: Topic) -> &'static [Item] {
    match topic {
        Topic::Start => content::START, Topic::Voice => content::VOICE,
        Topic::Models => content::MODELS, Topic::Convert => content::CONVERT,
        Topic::Devices => content::DEVICES, Topic::Performance => content::PERFORMANCE,
        Topic::General => content::GENERAL, Topic::Troubleshooting => content::TROUBLESHOOTING,
    }
}

/// The range and default of `id`, from the same constants the controls use; `None` when it has none.
pub fn range_text(id: Id) -> Option<String> {
    let voice = VoiceSettings::default();
    let stream = StreamSettings::default();
    Some(match id {
        Id::Pitch => span(&vc_models::PITCH_RANGE, "st", Some(voice.pitch)),
        Id::DefaultPitch => span(&vc_models::PITCH_RANGE, "st", None),
        Id::InputGain => span(&GAIN_DB_RANGE, "dB", Some(voice.input_db)),
        Id::OutputGain => span(&GAIN_DB_RANGE, "dB", Some(voice.output_db)),
        Id::NoiseGate => span(&GATE_DB_RANGE, "dB", Some(voice.gate_db)),
        Id::MonitorVolume => span(&MONITOR_DB_RANGE, "dB", Some(vc_audio::DEFAULT_MONITOR_DB)),
        Id::Gpu => span(&GPU_RANGE, "", Some(PerfSettings::default().gpu)),
        Id::Block => span(&BLOCK_MS, "ms", Some(stream.block_ms)),
        Id::ExtraContext => span(&CONTEXT_MS, "ms", Some(stream.context_ms)),
        Id::Crossfade => span(&CROSSFADE_MS, "ms", Some(stream.crossfade_ms)),
        _ => return None,
    })
}

/// "-20 to +20 dB · default 0". A range that goes below zero shows a + on its positive numbers.
fn span<T: Into<f64> + Copy>(range: &RangeInclusive<T>, unit: &str, default: Option<T>) -> String {
    let plus = (*range.start()).into() < 0.0;
    let num = |v: T| {
        let v: f64 = v.into();
        if plus && v > 0.0 { format!("+{v}") } else { format!("{v}") }
    };
    let unit = if unit.is_empty() { String::new() } else { format!(" {unit}") };
    let text = format!("{} to {}{unit}", num(*range.start()), num(*range.end()));
    match default {
        Some(d) => format!("{text} · default {}", num(d)),
        None => text,
    }
}

/// Whether the Help window is open and on which topic. Not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpState { pub open: bool, pub topic: Topic }

impl HelpState {
    /// Open on Getting started on a first run, closed otherwise.
    pub fn at_startup(first_run: bool) -> Self { Self { open: first_run, topic: Topic::Start } }
    /// Closes the window if it is open; otherwise opens it on `tab`'s topic.
    pub fn toggle(&mut self, tab: Tab) {
        if self.open { self.open = false } else { self.open_on(Topic::for_tab(tab)) }
    }
    pub fn open_on(&mut self, topic: Topic) { self.open = true; self.topic = topic; }
}
```

`item` is added in step 4, together with its only user, `tip`:
```rust
/// The entry of `id`. Every `Id` has exactly one (tested).
pub fn item(id: Id) -> &'static Item {
    Topic::ALL.iter().flat_map(|t| items(*t)).find(|i| i.id == Some(id))
        .expect("every Id has an entry")
}
```

Expected `range_text` results (tested): Pitch `-24 to +24 st · default 0`, DefaultPitch `-24 to +24 st`, InputGain and OutputGain `-20 to +20 dB · default 0`, NoiseGate `-90 to -20 dB · default -50`, MonitorVolume `-40 to 0 dB · default -6`, Gpu `0 to 7 · default 0`, Block `50 to 1000 ms · default 250`, ExtraContext `0 to 2000 ms · default 500`, Crossfade `10 to 200 ms · default 50`. Every other `Id` gives `None`.

```rust
// help/ui.rs
use eframe::egui;
use super::{HelpState, Id, Item, Topic};
use crate::app::Tab;

/// Widest tip, in points.
const TIP_WIDTH: f32 = 300.0;
const WINDOW_WIDTH: f32 = 340.0;
const WINDOW_HEIGHT: f32 = 380.0;

/// F1 with no modifiers, unless Settings is recording a hotkey.
pub fn f1_pressed(ctx: &egui::Context, recording_hotkey: bool) -> bool {
    !recording_hotkey && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::F1))
}

/// "? Help" at the right end of the tab bar.
pub fn tab_bar_button(ui: &mut egui::Ui, help: &mut HelpState, tab: Tab) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui.selectable_label(help.open, "? Help").on_hover_text("Open the guide (F1)").clicked() {
            help.toggle(tab);
        }
    });
}

/// The Help window: topic row, then the topic's entries in a scroll area (one per topic).
pub fn window(ctx: &egui::Context, help: &mut HelpState) {
    let screen = ctx.screen_rect();
    let mut open = help.open;
    egui::Window::new("Help")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(WINDOW_WIDTH)
        .default_height(WINDOW_HEIGHT)
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos(egui::pos2(screen.right() - 12.0, screen.top() + 56.0))
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for t in Topic::ALL {
                    ui.selectable_value(&mut help.topic, t, t.title());
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt(help.topic.title())
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for item in super::items(help.topic) {
                        entry(ui, item);
                        ui.add_space(6.0);
                    }
                });
        });
    help.open = open;
}

/// One guide entry: name, range, short text, then the rest in weak text.
fn entry(ui: &mut egui::Ui, item: &Item) {
    ui.label(egui::RichText::new(item.name).strong());
    if let Some(range) = item.id.and_then(super::range_text) {
        ui.weak(range);
    }
    ui.label(item.short);
    if !item.more.is_empty() {
        ui.weak(item.more);
    }
}

// added in step 4:
/// Attaches `id`'s tip, also to a greyed-out widget.
pub fn tip(response: egui::Response, id: Id) -> egui::Response {
    response
        .on_hover_ui(|ui| tip_body(ui, id))
        .on_disabled_hover_ui(|ui| tip_body(ui, id))
}

fn tip_body(ui: &mut egui::Ui, id: Id) {
    ui.set_max_width(TIP_WIDTH);
    ui.label(super::item(id).short);
    if let Some(range) = super::range_text(id) {
        ui.weak(range);
    }
}

/// A heading with a small "?" that opens Help on `topic`.
pub fn heading(ui: &mut egui::Ui, help: &mut HelpState, text: &str, topic: Topic) {
    ui.horizontal(|ui| {
        ui.heading(text);
        if ui.small_button("?").on_hover_text(format!("Open Help on {}", topic.title())).clicked() {
            help.open_on(topic);
        }
    });
}
```

## Steps

### 1. Name the slider ranges shared with the help
Files: `crates/app/src/app.rs`, `crates/app/src/tabs/voice.rs`, `crates/app/src/tabs/settings.rs`.

Write this test first in `app.rs` `mod tests`:
```rust
#[test]
fn slider_ranges() {
    assert_eq!(GAIN_DB_RANGE, -20.0..=20.0);
    assert_eq!(MONITOR_DB_RANGE, -40.0..=0.0);
    assert_eq!(GPU_RANGE, 0..=7);
}
```
Run `cargo test -p voicechanger slider_ranges`. It is expected to fail to compile (the constants don't exist yet).

Implement this in `app.rs`, next to `VoiceSettings` (add `use std::ops::RangeInclusive;`):
```rust
/// Input gain and Output gain, in dB.
pub const GAIN_DB_RANGE: RangeInclusive<f32> = -20.0..=20.0;
/// Monitor volume, in dB.
pub const MONITOR_DB_RANGE: RangeInclusive<f32> = -40.0..=0.0;
/// GPU index for DirectML and CUDA.
pub const GPU_RANGE: RangeInclusive<i32> = 0..=7;
```
Use the constants:
- `voice.rs`: Pitch uses `vc_models::PITCH_RANGE`, both gains use `GAIN_DB_RANGE`, Noise gate uses `vc_engine::stream::settings::GATE_DB_RANGE`.
- `settings.rs`: Volume uses `MONITOR_DB_RANGE`, GPU uses `.range(GPU_RANGE)`, and the loop uses `BLOCK_MS`, `CONTEXT_MS` and `CROSSFADE_MS` from `vc_engine::stream::settings`.

No range or default changes. Run `cargo test -p voicechanger` and `cargo clippy --all-targets --all-features -- -D warnings`; both are expected to pass.

Commit: `refactor(app): VC-150 slider ranges as named constants`.

### 2. Help data, texts and the Help window, opened with ? Help and F1
Files: create `crates/app/src/help/mod.rs`, `help/content.rs` and `help/ui.rs`; modify `main.rs` (`mod help;`, alphabetically after `mod glyphs;`) and `app.rs`.

Write the tests first.

`help/mod.rs` `#[cfg(test)] mod tests`:
- `every_id_has_exactly_one_item`: for each `Id::ALL`, count the items over `Topic::ALL` with `id == Some(id)`; the count must be 1.
- `every_topic_has_items`: `items(t)` is not empty. Start and Troubleshooting items all have `id: None`; every item in the other topics has `Some`.
- `texts_are_filled_and_short`: every name and short text is non-empty and trimmed, and `short.chars().count() <= SHORT_MAX`.
- `range_texts`: the exact strings listed under Shared contracts, and `None` for every other `Id::ALL` entry.
- `topic_for_tab`: Voice>Voice, Models>Models, Convert>Convert, Settings>Devices.
- `help_state_at_startup`: `at_startup(true)` gives `{open: true, topic: Start}`; `at_startup(false)` is closed.
- `toggle_opens_on_tab_topic_and_closes`: closed, then `toggle(Tab::Settings)` gives open on Devices; another `toggle` closes it and keeps the topic.
- `open_on_switches_topic`: open on Voice, then `open_on(Topic::General)` gives open on General.

`help/ui.rs` tests:
- `window_draws_every_topic`: for each `Topic::ALL`, with `HelpState{open: true, topic}`, run 2 frames of `egui::Context::default().run(egui::RawInput::default(), |ctx| { egui::CentralPanel::default().show(ctx, |ui| tab_bar_button(ui, &mut h, Tab::Voice)); window(ctx, &mut h); })`, then assert `h.open` and `h.topic == topic`.
- `f1_toggles_without_modifiers_only`: a helper builds a `RawInput` with `events: vec![egui::Event::Key { key: egui::Key::F1, physical_key: None, pressed: true, repeat: false, modifiers }]` and `modifiers`, runs one frame and records `f1_pressed(ctx, recording)`. NONE with recording false gives true; CTRL gives false; NONE with recording true gives false; a frame with no events gives false.

Run `cargo test -p voicechanger help::`. It is expected to fail to compile.

Implement:
- Copy `mod.rs` (everything except `item`) and `ui.rs` (everything except `tip`, `tip_body` and `heading`) from Shared contracts.
- Copy `content.rs` from Appendix A.
- `app.rs`: add the field `pub(crate) help: HelpState` (doc: `/// The Help window: open or not and on which topic.`), set to `help: HelpState::at_startup(false)` (step 3 passes the real value). `draw` gets these changes:
  - At its top: `if help::ui::f1_pressed(ctx, self.recording_hotkey) { self.help.toggle(self.tab); }`
  - Inside the tab-bar `ui.horizontal`, after the loop: `help::ui::tab_bar_button(ui, &mut self.help, self.tab);`
  - After `update::ui::window(ctx, self);`: `help::ui::window(ctx, &mut self.help);`
  - Add the imports `use crate::help::{self, HelpState};`.

Run `cargo test -p voicechanger` (the glyph guard covers the new texts) and clippy; both are expected to pass.

Commit: `feat(app): VC-150 Help window with guide texts, ? Help and F1`.

### 3. Open Help once on a fresh install
Files: `config.rs`, `main.rs`, `app.rs`.

Write these tests first in `config.rs`:
```rust
#[test]
fn first_run_until_settings_exist() {
    let dir = TempDir::new("config-first-run");
    let path = dir.path().join("config").join("settings.toml");
    assert!(first_run(&path));
    Config::default().save(&path).unwrap();
    assert!(!first_run(&path));
}

#[test]
fn corrupt_settings_are_not_a_first_run() {
    let dir = TempDir::new("config-first-run-corrupt");
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "this is = = not toml").unwrap();
    assert!(!first_run(&path));
}
```
Run `cargo test -p voicechanger first_run`. It is expected to fail to compile.

Implement in `config.rs`:
```rust
/// True when no settings file exists yet: a fresh install. Read before `load`, which renames bad files.
pub fn first_run(path: &Path) -> bool {
    !path.exists()
}
```
- `main.rs`: put `let first_run = config::first_run(&dirs.settings_file());` right before `config::load`, and add `first_run` to `Startup`.
- `app.rs`: add `pub first_run: bool` to `Startup` and update its doc (`... and whether this is the first run`). In `VcApp::new`, put `let first_run = startup.first_run;` at the top, then `help: HelpState::at_startup(first_run)`. After the last-model block, before `app`:
```rust
// A fresh install opens Help once; saving now makes the next start a normal one.
if first_run {
    let cfg = app.state.to_config(&app.prefs);
    app.write_config(cfg);
}
```
Run the tests and clippy; both are expected to pass.

Commit: `feat(app): VC-150 open Help once on a fresh install`.

### 4. Tips and ? headings on the Voice tab and the status bar
(Full detail in the steps array.)

### 5. Tips and ? headings on the Models and Convert tabs
(Full detail in the steps array.)

### 6. Tips on the Settings tab and update rows, with the coverage scan
(Full detail in the steps array.)

### 7. Changelog and README
(Full detail in the steps array. The texts are in Appendix B.)

## Final verification
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo xtask fetch-models
VC_REQUIRE_ASSETS=1 cargo test --workspace
cargo deny check
cargo build --release
```
All of these must pass. The `help::` tests show that every entry has text, a range where it should, a tip in the UI (coverage scan), a working state and a headless window. The glyph guard shows that every help character can be drawn.

## Appendix A: `crates/app/src/help/content.rs`

```rust
//! The help texts, one table per topic. A tip is an entry's `short`; the guide adds `more`.
//! Ranges and defaults are never typed here: `range_text` builds them from the code.

use super::{Id, Item};

const fn item(id: Id, name: &'static str, short: &'static str, more: &'static str) -> Item {
    Item { id: Some(id), name, short, more }
}

const fn text(name: &'static str, short: &'static str, more: &'static str) -> Item {
    Item { id: None, name, short, more }
}

pub static START: &[Item] = &[
    text("1. Get a virtual cable",
        "VoiceChanger plays your voice into a virtual audio cable, such as the free VB-CABLE. Other apps then use that cable as their microphone.",
        "When no cable is found, Settings says so and shows a download link. You can still pick any output, such as your speakers, to try the app."),
    text("2. Pick your devices",
        "In Settings, pick your Microphone, and as Virtual mic the cable's playback side, for example CABLE Input. Cables are marked as recommended.",
        "Turn on Monitor to hear yourself in your headphones."),
    text("3. Add a voice",
        "On the Models tab, press Import… or drop an RVC v2 .onnx file on the window.",
        "A PyTorch .pth or .zip model is converted into the library on the Convert tab first."),
    text("4. Press Start",
        "On the Voice tab, pick the voice in the Model list and press Start.",
        "Original voice (no model) sends your microphone unchanged and needs no model. Closing the window keeps the voice running in the system tray."),
    text("5. Use it in other apps",
        "In Discord, OBS, Zoom or a game, choose the cable's recording side, for example CABLE Output, as the microphone.",
        "Do not pick that recording side as VoiceChanger's own Microphone: it would send the voice back into the cable."),
    text("6. Fine-tune",
        "Hover any control to see what it does, with its range and default. Press F1 or ? Help to open this guide, or the ? next to a heading for that part.",
        "Troubleshooting lists what to change when something sounds wrong."),
];

pub static VOICE: &[Item] = &[
    item(Id::Model, "Model",
        "The voice you send. Original voice (no model) sends your microphone unchanged; any other entry converts your voice with that model.",
        "Picking another voice while running switches to it once it has loaded. Only models whose file exists are listed; add more on the Models tab."),
    item(Id::StartStop, "Start / Stop",
        "Starts or stops sending audio to the virtual mic. When Start is greyed out, the line below it says why.",
        "Start needs a microphone, a cable output and, for a voice model, a loaded model. The Start/Stop hotkey and the tray menu do the same."),
    item(Id::Bypass, "Bypass",
        "Sends your original voice while the chosen voice model stays loaded, so you can switch back without loading it again.",
        "The audio restarts underneath, so there is a short gap. Bypass is never saved: it is off each time VoiceChanger starts. The tray menu can flip it too."),
    item(Id::Pitch, "Pitch",
        "Moves the converted voice up or down in semitones; 12 semitones is one octave.",
        "Raise it when the voice sounds too low for the model; lower it when it sounds too high. The value is remembered for each model; a model you have not used yet starts on its Default pitch from the Models tab. Applies to converted voices only."),
    item(Id::InputGain, "Input gain",
        "Makes your microphone louder or quieter before anything else. The Input meter shows the level after this gain.",
        "Raise it when the Input meter barely moves while you speak; lower it when it reaches 0 dB, which distorts the voice."),
    item(Id::OutputGain, "Output gain",
        "Makes the voice sent to the virtual mic louder or quieter. The monitor follows it too.",
        "Raise it when other apps hear you too quietly; lower it when the Output meter reaches 0 dB."),
    item(Id::NoiseGate, "Noise gate",
        "Silences sound quieter than this level, so background noise between words is not converted.",
        "Raise it (towards -20 dB) when noise between words comes through; lower it when quiet words or word endings are cut off. Applies to converted voices only."),
    item(Id::Meters, "Input and Output meters",
        "Show how loud your microphone (after Input gain) and the sent voice are while running.",
        "The bars cover -60 to 0 dB. Speak normally: the Input bar should move clearly and stay below 0 dB."),
    item(Id::Latency, "Latency",
        "The delay from your microphone to the virtual mic, measured while running. It turns orange above 300 ms.",
        "Block and Crossfade add to it. Lower Block to reduce it, as long as Inference does not turn orange."),
    item(Id::Inference, "Inference",
        "How long the model took for the last block, against the block length. It turns orange above 60% of a block.",
        "When it is orange, raise Block, lower Extra context or pick a faster provider; otherwise blocks fall behind and the voice breaks up."),
    item(Id::LateBlocks, "Late blocks",
        "How many blocks of your voice were skipped because conversion fell behind.",
        "If it keeps rising, raise Block, lower Extra context or pick a faster provider in Settings > Performance."),
    item(Id::Underruns, "Underruns",
        "How often the virtual mic ran out of audio and got silence instead; each one is a short gap.",
        "If it keeps rising, follow the advice for Late blocks."),
];

pub static MODELS: &[Item] = &[
    item(Id::Import, "Import…",
        "Adds RVC v2 .onnx voice models to the library. You can also drop files on the window.",
        "Each model is checked and copied into the library. When a model does not say its sample rate, pick 32, 40 or 48 kHz. A .pth or .zip file is offered to the Convert tab instead."),
    item(Id::OpenFolder, "Open folder",
        "Opens the folder where the library keeps the model files and their icons.", ""),
    item(Id::Reorder, "Move up and down",
        "Moves a model up or down in the list. The Voice tab's Model list uses the same order.", ""),
    item(Id::ModelName, "Model name",
        "Click a model's name to use it; the highlighted one is in use.",
        "A model whose file is missing is greyed out and marked as file missing."),
    item(Id::DefaultPitch, "Default pitch",
        "The pitch a model starts on the first time you use it. After that the Voice tab remembers your own pitch for it.",
        "Drag it or type a value; a drag is saved when you let go."),
    item(Id::IconRenameDelete, "Icon…, Rename, Delete",
        "Icon… gives the model a picture (PNG or JPEG), Rename changes its name, and Delete removes the model and its file.",
        "Delete asks first. The model in use can only be deleted while the voice is stopped."),
];

pub static CONVERT: &[Item] = &[
    item(Id::ChooseFile, "Choose file…",
        "Opens a PyTorch voice model (.pth, or a .zip holding one) to convert. You can also drop it on this tab.",
        "The file is read first, and its version, sample rate, pitch support and speakers are shown."),
    item(Id::ConvertForm, "Name, Default pitch and Icon",
        "How the new model appears in your library: its name, the pitch it starts on and an optional picture.",
        "You can change all three later on the Models tab."),
    item(Id::ConvertButton, "Convert",
        "Converts the file into an ONNX model and adds it to your library. It needs a name.",
        "Converting can take a while; Cancel stops it."),
    item(Id::AfterConvert, "After converting",
        "Use now picks the new model, Show in Models opens the library, Export .onnx… saves a copy of the file, and Convert another… starts again.", ""),
];

pub static DEVICES: &[Item] = &[
    item(Id::Microphone, "Microphone",
        "The microphone VoiceChanger listens to. While running, a level bar shows next to it.",
        "Do not pick a cable's recording side (for example CABLE Output) here: it would send the voice back into the cable."),
    item(Id::VirtualMic, "Virtual mic",
        "Where the voice is sent: the playback side of your virtual cable, for example CABLE Input. Cables are marked as recommended.",
        "Other apps then pick the cable's recording side (CABLE Output) as their microphone. Changing a device while running restarts the audio."),
    item(Id::Monitor, "Monitor",
        "Plays the sent voice to a second output, such as your headphones, so you hear yourself.",
        "Do not pick the virtual mic's device here: everything listening to the cable would hear the monitor too."),
    item(Id::MonitorVolume, "Volume",
        "How loud you hear yourself on the monitor. It does not change what other apps hear.", ""),
    item(Id::Refresh, "Refresh",
        "Lists the audio devices again, for example after plugging in a headset or installing a cable.",
        "A chosen device that is gone is replaced by the Windows default, with a note."),
];

pub static PERFORMANCE: &[Item] = &[
    item(Id::Provider, "Provider",
        "Where the voice model runs: DirectML on most graphics cards, CUDA on NVIDIA cards, or CPU. It can only be changed while stopped.",
        "CUDA needs a CUDA build of onnxruntime.dll. If a provider cannot start, the model runs on the CPU instead; the status bar shows which one is used."),
    item(Id::Gpu, "GPU",
        "Which graphics card DirectML or CUDA uses, counted from 0. Only shown when the provider is not CPU.",
        "Change it when the computer has more than one graphics card and the slower one is used."),
    item(Id::Block, "Block",
        "How much audio is converted at a time. Smaller blocks mean less delay; larger blocks give a slow graphics card more time.",
        "Lower it to reduce Latency while Inference is not orange; raise it when Inference turns orange or late blocks rise. Changing it while running restarts the audio."),
    item(Id::ExtraContext, "Extra context",
        "How much of the audio before each block the model also hears. More context gives the model more to go on but takes more inference time.",
        "Lower it when Inference turns orange. Changing it while running restarts the audio."),
    item(Id::Crossfade, "Crossfade",
        "How long each block blends into the next, to hide the joins. Its length is added to the delay.",
        "Raise it when you hear clicks between blocks; lower it to cut delay. Changing it while running restarts the audio."),
];

pub static GENERAL: &[Item] = &[
    item(Id::Hotkey, "Hotkey",
        "A key combination that starts and stops the voice from anywhere, also while VoiceChanger is in the tray. Off by default.",
        "Press Set…, then the keys. It needs Ctrl, Alt or Shift; Esc cancels. Clear turns it off."),
    item(Id::StartInTray, "Start in the tray",
        "Starts VoiceChanger with only its tray icon, without opening the window.",
        "Click the tray icon to open the window. The voice never starts by itself."),
    item(Id::Updates, "Updates",
        "Checks GitHub for a new VoiceChanger every 6 hours, and can download it in the background. Check for updates asks now.",
        "For each new version you choose Update now, Later or Skip this version. Models and settings are kept."),
    item(Id::OpenLogs, "Open logs folder",
        "Opens the folder with VoiceChanger's daily log files. Attach the latest one when you report a problem.", ""),
    item(Id::About, "About…",
        "Shows the version, the licence, the repository and where VoiceChanger keeps its models, settings and logs.", ""),
];

pub static TROUBLESHOOTING: &[Item] = &[
    text("Other apps do not hear me",
        "Check that Virtual mic is the cable's playback side (CABLE Input), that the other app uses its recording side (CABLE Output) as microphone, and that VoiceChanger is running.",
        "Watch the Output meter: when it moves, VoiceChanger is sending."),
    text("The voice breaks up or stutters",
        "Look at Inference in the status bar. When it is orange or late blocks rise, raise Block, lower Extra context or pick a faster provider.", ""),
    text("The voice comes too late",
        "Lower Block, and then Crossfade, a little at a time, while watching that Inference does not turn orange.",
        "Latency in the status bar shows the delay; it turns orange above 300 ms."),
    text("The voice sounds too high or too low",
        "Move Pitch on the Voice tab. 12 semitones is one octave.",
        "Pitch is remembered for each model."),
    text("Background noise comes through",
        "Raise Noise gate until the noise between words is silent, but not so far that quiet words are cut.", ""),
    text("I hear myself twice",
        "Turn Monitor off, or pick headphones for it instead of the cable's device.", ""),
    text("Start is greyed out",
        "The line under Start says why: usually a missing microphone or cable output in Settings, or a voice model that is still loading.",
        "Press Refresh in Settings after plugging a device in."),
    text("A model is marked file missing",
        "Its file was moved or deleted from the models folder. Delete the entry and import the model again.", ""),
];
```
`cargo fmt` reflows these calls; that is expected. The texts use only ASCII plus `…` and the `·` that `range_text` adds, both of which the app already draws.

## Appendix B: docs texts

`CHANGELOG.md`: under `## [Unreleased]`, add a new `### Added` section above `### Fixed`:
```
### Added
- In-app help: every control explains itself when you hover it, with its range and default. The Help window (**? Help** in the tab bar, **F1**, or the **?** next to a heading) has Getting started, a page for each part of the app and Troubleshooting. It opens by itself the first time VoiceChanger starts on a fresh install.
```
`README.md`: add a new section `## Help` right before `## Original voice and Bypass`:
```
## Help

Hover any control to see what it does, with its range and default. **? Help** in the tab bar, **F1**, or the **?** next to a heading opens the Help window: Getting started, a page for each part of the app and Troubleshooting. It opens by itself once, the first time VoiceChanger starts on a fresh install.
```
