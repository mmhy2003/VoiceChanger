//! Drawing the help: hover tips, ? headings, the tab-bar button, F1 and the Help window.

use eframe::egui;

use super::{HelpState, Id, Item, Topic};
use crate::app::Tab;

/// Widest tip, in points.
const TIP_WIDTH: f32 = 300.0;
const WINDOW_WIDTH: f32 = 340.0;
const WINDOW_HEIGHT: f32 = 380.0;

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
        if ui
            .small_button("?")
            .on_hover_text(format!("Open Help on {}", topic.title()))
            .clicked()
        {
            help.open_on(topic);
        }
    });
}

/// F1 with no modifiers, unless Settings is recording a hotkey.
pub fn f1_pressed(ctx: &egui::Context, recording_hotkey: bool) -> bool {
    !recording_hotkey && ctx.input(|i| i.modifiers.is_none() && i.key_pressed(egui::Key::F1))
}

/// "? Help" at the right end of the tab bar.
pub fn tab_bar_button(ui: &mut egui::Ui, help: &mut HelpState, tab: Tab) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .selectable_label(help.open, "? Help")
            .on_hover_text("Open the guide (F1)")
            .clicked()
        {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_draws_every_topic() {
        for topic in Topic::ALL {
            let ctx = egui::Context::default();
            let mut h = HelpState { open: true, topic };
            for _ in 0..2 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| tab_bar_button(ui, &mut h, Tab::Voice));
                    window(ctx, &mut h);
                });
            }
            assert!(h.open, "{topic:?}");
            assert_eq!(h.topic, topic);
        }
    }

    /// Runs one frame with `events` and `modifiers`, and returns what `f1_pressed` said.
    fn f1_in_frame(events: Vec<egui::Event>, modifiers: egui::Modifiers, recording: bool) -> bool {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events,
            modifiers,
            ..Default::default()
        };
        let mut pressed = false;
        let _ = ctx.run(input, |ctx| pressed = f1_pressed(ctx, recording));
        pressed
    }

    fn f1(modifiers: egui::Modifiers, recording: bool) -> bool {
        let event = egui::Event::Key {
            key: egui::Key::F1,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        };
        f1_in_frame(vec![event], modifiers, recording)
    }

    #[test]
    fn tip_on_enabled_and_disabled_widgets() {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.interaction.tooltip_delay = 0.0);
        // Draws one frame and returns the rects of Start and Pitch.
        let frame = |input: egui::RawInput| {
            let mut rects = (egui::Rect::NOTHING, egui::Rect::NOTHING);
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let start = ui.button("Start");
                    let start_id = start.id;
                    let start = tip(start, Id::StartStop);
                    assert_eq!(start.id, start_id);
                    let pitch = ui.add_enabled(false, egui::Button::new("Pitch"));
                    let pitch_id = pitch.id;
                    let pitch = tip(pitch, Id::Pitch);
                    assert_eq!(pitch.id, pitch_id);
                    rects = (start.rect, pitch.rect);
                });
            });
            rects
        };
        let (start, pitch) = frame(egui::RawInput::default());
        for target in [pitch, start] {
            for _ in 0..2 {
                frame(egui::RawInput {
                    events: vec![egui::Event::PointerMoved(target.center())],
                    ..Default::default()
                });
            }
        }
    }

    #[test]
    fn heading_question_opens_topic() {
        let ctx = egui::Context::default();
        let mut h = HelpState::at_startup(false);
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| heading(ui, &mut h, "Voice", Topic::Voice));
            });
        }
        assert!(!h.open);
    }

    #[test]
    fn f1_toggles_without_modifiers_only() {
        assert!(f1(egui::Modifiers::NONE, false));
        assert!(!f1(egui::Modifiers::CTRL, false));
        assert!(!f1(egui::Modifiers::NONE, true));
        assert!(!f1_in_frame(Vec::new(), egui::Modifiers::NONE, false));
    }
}
