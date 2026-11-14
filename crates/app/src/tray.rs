//! The system tray icon: its menu, its two icons, and the rules for what a click and a close do.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the tray icon in the next VC-121 step")
)]

use image::imageops::{self, FilterType};
use image::{ImageFormat, Rgba, RgbaImage};
use tray_icon::{MouseButton, MouseButtonState};

/// What the user asked for from the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    Start,
    Stop,
    Exit,
}

const OPEN_ID: &str = "open";
const START_ID: &str = "start";
const STOP_ID: &str = "stop";
const EXIT_ID: &str = "exit";

/// The command of the menu item with `id`.
pub fn command_for_menu(id: &str) -> Option<TrayCommand> {
    match id {
        OPEN_ID => Some(TrayCommand::Open),
        START_ID => Some(TrayCommand::Start),
        STOP_ID => Some(TrayCommand::Stop),
        EXIT_ID => Some(TrayCommand::Exit),
        _ => None,
    }
}

/// Releasing the left button on the icon opens the window; the right button opens the menu,
/// which the tray library does itself.
pub fn command_for_click(button: MouseButton, state: MouseButtonState) -> Option<TrayCommand> {
    (button == MouseButton::Left && state == MouseButtonState::Up).then_some(TrayCommand::Open)
}

/// What a close request does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    Exit,
    Hide,
}

/// The window's close button hides the window to the tray. The app exits when it is quitting
/// (tray Exit, the updater, Windows or an installer), when the window is already hidden (the
/// request came from outside, e.g. `taskkill`), and when there is no tray icon to bring it back.
pub fn close_action(quitting: bool, in_tray: bool, tray_available: bool) -> CloseAction {
    if quitting || in_tray || !tray_available {
        CloseAction::Exit
    } else {
        CloseAction::Hide
    }
}

/// What the tray shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayState {
    /// The green dot, and Stop is enabled.
    pub running: bool,
    pub start_enabled: bool,
    pub tooltip: String,
}

/// The tray for the app's state. `start_blocked` is whether the Start button has a hint;
/// `model` is the loaded model's name.
pub fn tray_state(running: bool, start_blocked: bool, model: Option<&str>) -> TrayState {
    TrayState {
        running,
        start_enabled: !running && !start_blocked,
        tooltip: tooltip(running, model),
    }
}

/// Windows keeps at most 127 UTF-16 units of a tray tooltip.
const TOOLTIP_MAX: usize = 127;

/// "VoiceChanger: Running (<model>)" or "VoiceChanger: Stopped", cut to what Windows shows.
pub fn tooltip(running: bool, model: Option<&str>) -> String {
    let mut text = match (running, model) {
        (false, _) => "VoiceChanger: Stopped".to_string(),
        (true, Some(name)) => format!("VoiceChanger: Running ({name})"),
        (true, None) => "VoiceChanger: Running".to_string(),
    };
    if text.encode_utf16().count() > TOOLTIP_MAX {
        while text.encode_utf16().count() > TOOLTIP_MAX - 1 {
            text.pop();
        }
        text.push('…');
    }
    text
}

/// Width and height of the tray icon in pixels.
pub const ICON_SIZE: u32 = 32;
/// The running dot.
pub const DOT_COLOUR: Rgba<u8> = Rgba([34, 197, 94, 255]);
const DOT_CENTRE: i32 = 25;
const DOT_RADIUS: i32 = 6;

/// Whether pixel (x, y) of the icon is inside the running dot, at the bottom right.
fn in_dot(x: u32, y: u32) -> bool {
    let (dx, dy) = (x as i32 - DOT_CENTRE, y as i32 - DOT_CENTRE);
    dx * dx + dy * dy <= DOT_RADIUS * DOT_RADIUS
}

/// The tray icon: the logo scaled to 32 px, with a green dot while running.
pub fn icon_rgba(running: bool) -> RgbaImage {
    let logo = image::load_from_memory_with_format(crate::ICON_PNG, ImageFormat::Png)
        .expect("assets/icon-256.png is a valid PNG")
        .into_rgba8();
    let mut icon = imageops::resize(&logo, ICON_SIZE, ICON_SIZE, FilterType::Lanczos3);
    if running {
        for (x, y, pixel) in icon.enumerate_pixels_mut() {
            if in_dot(x, y) {
                *pixel = DOT_COLOUR;
            }
        }
    }
    icon
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_action_every_combination() {
        for quitting in [false, true] {
            for in_tray in [false, true] {
                for tray_available in [false, true] {
                    let expected = if !quitting && !in_tray && tray_available {
                        CloseAction::Hide
                    } else {
                        CloseAction::Exit
                    };
                    assert_eq!(
                        close_action(quitting, in_tray, tray_available),
                        expected,
                        "quitting {quitting}, in_tray {in_tray}, tray {tray_available}"
                    );
                }
            }
        }
    }

    #[test]
    fn tray_state_stopped_running_blocked() {
        let stopped = tray_state(false, false, Some("Ana"));
        assert!(!stopped.running && stopped.start_enabled);
        assert_eq!(stopped.tooltip, "VoiceChanger: Stopped");
        let running = tray_state(true, false, Some("Ana"));
        assert!(running.running && !running.start_enabled);
        assert_eq!(running.tooltip, "VoiceChanger: Running (Ana)");
        let blocked = tray_state(false, true, None);
        assert!(!blocked.running && !blocked.start_enabled);
        assert_eq!(blocked.tooltip, "VoiceChanger: Stopped");
        // A running session's Start stays greyed whatever the hint says.
        assert!(!tray_state(true, true, Some("Ana")).start_enabled);
    }

    #[test]
    fn tooltip_text() {
        assert_eq!(tooltip(false, None), "VoiceChanger: Stopped");
        assert_eq!(tooltip(false, Some("Ana")), "VoiceChanger: Stopped");
        assert_eq!(tooltip(true, Some("Ana")), "VoiceChanger: Running (Ana)");
        assert_eq!(tooltip(true, None), "VoiceChanger: Running");
        let long = tooltip(true, Some(&"é".repeat(300)));
        assert_eq!(long.encode_utf16().count(), 127);
        assert!(long.starts_with("VoiceChanger: Running (é") && long.ends_with('…'));
    }

    #[test]
    fn menu_ids_map_to_commands() {
        assert_eq!(command_for_menu(OPEN_ID), Some(TrayCommand::Open));
        assert_eq!(command_for_menu(START_ID), Some(TrayCommand::Start));
        assert_eq!(command_for_menu(STOP_ID), Some(TrayCommand::Stop));
        assert_eq!(command_for_menu(EXIT_ID), Some(TrayCommand::Exit));
        assert_eq!(command_for_menu("1001"), None);
    }

    #[test]
    fn left_click_release_opens() {
        use MouseButton::{Left, Middle, Right};
        use MouseButtonState::{Down, Up};
        assert_eq!(command_for_click(Left, Up), Some(TrayCommand::Open));
        for (button, state) in [(Left, Down), (Right, Up), (Right, Down), (Middle, Up)] {
            assert_eq!(command_for_click(button, state), None);
        }
    }

    fn scaled_logo() -> RgbaImage {
        let logo = image::load_from_memory_with_format(crate::ICON_PNG, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        imageops::resize(&logo, ICON_SIZE, ICON_SIZE, FilterType::Lanczos3)
    }

    #[test]
    fn icons_are_32_px() {
        assert_eq!(ICON_SIZE, 32);
        for running in [false, true] {
            assert_eq!(icon_rgba(running).dimensions(), (32, 32));
        }
    }

    #[test]
    fn stopped_icon_is_the_scaled_logo() {
        let icon = icon_rgba(false);
        assert!(icon == scaled_logo());
        assert!(icon.pixels().any(|p| p[3] > 0), "the icon is not empty");
    }

    #[test]
    fn running_icon_adds_only_a_green_dot() {
        let (stopped, running) = (icon_rgba(false), icon_rgba(true));
        let mut dot_pixels = 0;
        for (x, y, pixel) in running.enumerate_pixels() {
            if in_dot(x, y) {
                dot_pixels += 1;
                assert_eq!(*pixel, DOT_COLOUR, "({x}, {y}) is in the dot");
            } else {
                assert_eq!(
                    pixel,
                    stopped.get_pixel(x, y),
                    "({x}, {y}) is outside the dot"
                );
            }
        }
        assert!(dot_pixels > 50, "the dot covers {dot_pixels} pixels");
        let [r, g, b, a] = DOT_COLOUR.0;
        assert!(g > r && g > b && a == 255, "the dot is opaque green");
        assert!(running != stopped);
    }
}
