//! The system tray icon: its menu, its two icons, and the rules for what a click and a close do.

use std::sync::mpsc::{self, Receiver};

use image::imageops::{self, FilterType};
use image::{ImageFormat, Rgba, RgbaImage};
use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::window::Waker;

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

/// The tray icon and its menu. It lives on the UI thread, whose message loop delivers the clicks.
pub struct Tray {
    icon: TrayIcon,
    start: MenuItem,
    stop: MenuItem,
    stopped_icon: Icon,
    running_icon: Icon,
    rx: Receiver<TrayCommand>,
    /// What the tray shows now, so `sync` only calls Windows when it changes.
    shown: TrayState,
}

fn to_icon(image: RgbaImage) -> Result<Icon, String> {
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).map_err(|e| e.to_string())
}

impl Tray {
    /// Adds the icon to the system tray. A click queues a command and wakes the app.
    pub fn new(waker: Waker) -> Result<Self, String> {
        let shown = tray_state(false, true, None);
        let item = |id: &str, text: &str, enabled: bool| {
            MenuItem::with_id(MenuId::new(id), text, enabled, None)
        };
        let open = item(OPEN_ID, "Open VoiceChanger", true);
        let start = item(START_ID, "Start", shown.start_enabled);
        let stop = item(STOP_ID, "Stop", shown.running);
        let exit = item(EXIT_ID, "Exit", true);
        let menu = Menu::with_items(&[&open, &start, &stop, &exit]).map_err(|e| e.to_string())?;
        let stopped_icon = to_icon(icon_rgba(false))?;
        let running_icon = to_icon(icon_rgba(true))?;

        let (tx, rx) = mpsc::channel();
        let (menu_tx, menu_waker) = (tx.clone(), waker.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some(command) = command_for_menu(&e.id.0) {
                let _ = menu_tx.send(command);
                menu_waker.wake();
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = e
                && let Some(command) = command_for_click(button, button_state)
            {
                let _ = tx.send(command);
                waker.wake();
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(&shown.tooltip)
            .with_icon(stopped_icon.clone())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            icon,
            start,
            stop,
            stopped_icon,
            running_icon,
            rx,
            shown,
        })
    }

    /// The commands clicked since the last call.
    pub fn poll(&self) -> Vec<TrayCommand> {
        self.rx.try_iter().collect()
    }

    /// Shows `state`: the greyed items, the icon and the tooltip. Does nothing when it is
    /// what the tray already shows.
    pub fn sync(&mut self, state: TrayState) {
        if state == self.shown {
            return;
        }
        self.start.set_enabled(state.start_enabled);
        self.stop.set_enabled(state.running);
        if state.running != self.shown.running {
            let icon = if state.running {
                &self.running_icon
            } else {
                &self.stopped_icon
            };
            if let Err(e) = self.icon.set_icon(Some(icon.clone())) {
                tracing::warn!("cannot change the tray icon: {e}");
            }
        }
        if state.tooltip != self.shown.tooltip
            && let Err(e) = self.icon.set_tooltip(Some(&state.tooltip))
        {
            tracing::warn!("cannot change the tray tooltip: {e}");
        }
        self.shown = state;
    }
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
