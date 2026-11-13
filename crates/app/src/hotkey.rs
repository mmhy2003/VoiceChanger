//! The global Start/Stop hotkey: parsing, recording and registering a key combination.
#![expect(dead_code, reason = "wired into Settings in the next step")]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui::{self, Key};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

/// A key with at least one of Ctrl, Alt or Shift.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: egui::Key,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ComboError {
    #[error("Press a key combination.")]
    Empty,
    #[error("Add Ctrl, Alt or Shift to the key.")]
    NeedsModifier,
    #[error("Unknown key \"{0}\".")]
    UnknownKey(String),
}

/// The keys a hotkey can use, with their key code and display name.
/// Escape is not in the list: it cancels recording.
const KEYS: &[(Key, Code, &str)] = &[
    (Key::A, Code::KeyA, "A"),
    (Key::B, Code::KeyB, "B"),
    (Key::C, Code::KeyC, "C"),
    (Key::D, Code::KeyD, "D"),
    (Key::E, Code::KeyE, "E"),
    (Key::F, Code::KeyF, "F"),
    (Key::G, Code::KeyG, "G"),
    (Key::H, Code::KeyH, "H"),
    (Key::I, Code::KeyI, "I"),
    (Key::J, Code::KeyJ, "J"),
    (Key::K, Code::KeyK, "K"),
    (Key::L, Code::KeyL, "L"),
    (Key::M, Code::KeyM, "M"),
    (Key::N, Code::KeyN, "N"),
    (Key::O, Code::KeyO, "O"),
    (Key::P, Code::KeyP, "P"),
    (Key::Q, Code::KeyQ, "Q"),
    (Key::R, Code::KeyR, "R"),
    (Key::S, Code::KeyS, "S"),
    (Key::T, Code::KeyT, "T"),
    (Key::U, Code::KeyU, "U"),
    (Key::V, Code::KeyV, "V"),
    (Key::W, Code::KeyW, "W"),
    (Key::X, Code::KeyX, "X"),
    (Key::Y, Code::KeyY, "Y"),
    (Key::Z, Code::KeyZ, "Z"),
    (Key::Num0, Code::Digit0, "0"),
    (Key::Num1, Code::Digit1, "1"),
    (Key::Num2, Code::Digit2, "2"),
    (Key::Num3, Code::Digit3, "3"),
    (Key::Num4, Code::Digit4, "4"),
    (Key::Num5, Code::Digit5, "5"),
    (Key::Num6, Code::Digit6, "6"),
    (Key::Num7, Code::Digit7, "7"),
    (Key::Num8, Code::Digit8, "8"),
    (Key::Num9, Code::Digit9, "9"),
    (Key::F1, Code::F1, "F1"),
    (Key::F2, Code::F2, "F2"),
    (Key::F3, Code::F3, "F3"),
    (Key::F4, Code::F4, "F4"),
    (Key::F5, Code::F5, "F5"),
    (Key::F6, Code::F6, "F6"),
    (Key::F7, Code::F7, "F7"),
    (Key::F8, Code::F8, "F8"),
    (Key::F9, Code::F9, "F9"),
    (Key::F10, Code::F10, "F10"),
    (Key::F11, Code::F11, "F11"),
    (Key::F12, Code::F12, "F12"),
    (Key::Space, Code::Space, "Space"),
    (Key::Insert, Code::Insert, "Insert"),
    (Key::Delete, Code::Delete, "Delete"),
    (Key::Home, Code::Home, "Home"),
    (Key::End, Code::End, "End"),
    (Key::PageUp, Code::PageUp, "PageUp"),
    (Key::PageDown, Code::PageDown, "PageDown"),
    (Key::ArrowUp, Code::ArrowUp, "Up"),
    (Key::ArrowDown, Code::ArrowDown, "Down"),
    (Key::ArrowLeft, Code::ArrowLeft, "Left"),
    (Key::ArrowRight, Code::ArrowRight, "Right"),
];

/// The global key code for `key`, if it can be used in a hotkey.
pub fn key_code(key: egui::Key) -> Option<Code> {
    entry(key).map(|&(_, code, _)| code)
}

fn entry(key: egui::Key) -> Option<&'static (Key, Code, &'static str)> {
    KEYS.iter().find(|(k, _, _)| *k == key)
}

impl Combo {
    /// Needs Ctrl, Alt or Shift.
    pub fn new(mods: egui::Modifiers, key: egui::Key) -> Result<Self, ComboError> {
        if !(mods.ctrl || mods.alt || mods.shift) {
            return Err(ComboError::NeedsModifier);
        }
        Ok(Self {
            ctrl: mods.ctrl,
            alt: mods.alt,
            shift: mods.shift,
            key,
        })
    }

    /// Parses "Ctrl+Alt+V": modifier and key names are case-insensitive and the key comes last.
    pub fn parse(s: &str) -> Result<Self, ComboError> {
        if s.trim().is_empty() {
            return Err(ComboError::Empty);
        }
        let mut tokens: Vec<&str> = s.split('+').map(str::trim).collect();
        let key_name = tokens.pop().unwrap_or_default();
        let mut mods = egui::Modifiers::NONE;
        for token in tokens {
            match token.to_ascii_lowercase().as_str() {
                "ctrl" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                _ => return Err(ComboError::UnknownKey(token.to_owned())),
            }
        }
        let &(key, _, _) = KEYS
            .iter()
            .find(|(_, _, name)| name.eq_ignore_ascii_case(key_name))
            .ok_or_else(|| ComboError::UnknownKey(key_name.to_owned()))?;
        Self::new(mods, key)
    }

    pub fn hotkey(&self) -> HotKey {
        let mut mods = Modifiers::empty();
        mods.set(Modifiers::CONTROL, self.ctrl);
        mods.set(Modifiers::ALT, self.alt);
        mods.set(Modifiers::SHIFT, self.shift);
        let code = key_code(self.key).expect("a combo only holds keys from KEYS");
        HotKey::new(Some(mods), code)
    }
}

impl fmt::Display for Combo {
    /// "Ctrl+Alt+Shift+V", in that order.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        let name = entry(self.key).map_or_else(|| self.key.name(), |&(_, _, name)| name);
        f.write_str(name)
    }
}

/// What a key press does while a hotkey is being recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    Cancel,
    Combo(Combo),
    Invalid(String),
}

/// Escape cancels; any other key needs a modifier.
pub fn record_key(key: egui::Key, mods: egui::Modifiers) -> RecordOutcome {
    if key == Key::Escape {
        return RecordOutcome::Cancel;
    }
    match Combo::new(mods, key) {
        Ok(combo) => RecordOutcome::Combo(combo),
        Err(e) => RecordOutcome::Invalid(e.to_string()),
    }
}

/// The registered global hotkey, and whether it was pressed since the last check.
pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    current: Option<HotKey>,
    pressed: Arc<AtomicBool>,
}

impl Hotkeys {
    /// Creates the manager on the UI thread. A press sets `pressed` and repaints, so it is seen while minimised.
    pub fn new(ctx: &egui::Context) -> Self {
        let pressed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&pressed);
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
            if e.state == HotKeyState::Pressed {
                flag.store(true, Ordering::Relaxed);
                ctx.request_repaint();
            }
        }));
        let manager = GlobalHotKeyManager::new()
            .inspect_err(|e| tracing::error!("global hotkeys unavailable: {e}"))
            .ok();
        Self {
            manager,
            current: None,
            pressed,
        }
    }

    /// Unregisters the current hotkey and registers `combo`. On failure nothing is registered.
    pub fn set(&mut self, combo: Option<Combo>) -> Result<(), String> {
        let Some(manager) = &self.manager else {
            return Err("Global hotkeys are unavailable.".into());
        };
        if let Some(old) = self.current.take()
            && let Err(e) = manager.unregister(old)
        {
            tracing::warn!("could not unregister the hotkey: {e}");
        }
        let Some(combo) = combo else {
            return Ok(());
        };
        let hotkey = combo.hotkey();
        match manager.register(hotkey) {
            Ok(()) => {
                self.current = Some(hotkey);
                Ok(())
            }
            Err(e) => {
                tracing::warn!(%combo, "could not register the hotkey: {e}");
                Err(format!("{combo} is already used by another app."))
            }
        }
    }

    /// Whether the hotkey was pressed since the last call.
    pub fn take_pressed(&self) -> bool {
        self.pressed.swap(false, Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use egui::{Key, Modifiers};
    use global_hotkey::hotkey::Modifiers as HkMods;

    use super::*;

    #[test]
    fn parse_format_round_trip() {
        for s in ["Ctrl+Alt+V", "Shift+F9"] {
            assert_eq!(Combo::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(
            Combo::parse("ctrl+shift+1").unwrap().to_string(),
            "Ctrl+Shift+1"
        );
        assert_eq!(
            Combo::parse(" Shift + alt + ctrl + up ")
                .unwrap()
                .to_string(),
            "Ctrl+Alt+Shift+Up"
        );
        assert_eq!(
            Combo::parse("Ctrl+Alt+V").unwrap(),
            Combo {
                ctrl: true,
                alt: true,
                shift: false,
                key: Key::V
            }
        );
    }

    #[test]
    fn modifier_required() {
        assert_eq!(Combo::parse("V"), Err(ComboError::NeedsModifier));
        assert_eq!(
            Combo::new(Modifiers::NONE, Key::V),
            Err(ComboError::NeedsModifier)
        );
        assert_eq!(Combo::parse(""), Err(ComboError::Empty));
        assert_eq!(Combo::parse("  "), Err(ComboError::Empty));
        assert_eq!(
            ComboError::NeedsModifier.to_string(),
            "Add Ctrl, Alt or Shift to the key."
        );
    }

    #[test]
    fn unknown_key_rejected() {
        assert_eq!(
            Combo::parse("Ctrl+Foo"),
            Err(ComboError::UnknownKey("Foo".into()))
        );
        assert_eq!(
            ComboError::UnknownKey("Foo".into()).to_string(),
            "Unknown key \"Foo\"."
        );
        assert_eq!(
            Combo::parse("Ctrl+Escape"),
            Err(ComboError::UnknownKey("Escape".into()))
        );
    }

    #[test]
    fn egui_keys_map() {
        assert_eq!(key_code(Key::V), Some(Code::KeyV));
        assert_eq!(key_code(Key::Num1), Some(Code::Digit1));
        assert_eq!(key_code(Key::F9), Some(Code::F9));
        assert_eq!(key_code(Key::Space), Some(Code::Space));
        assert_eq!(key_code(Key::ArrowLeft), Some(Code::ArrowLeft));
        assert_eq!(key_code(Key::Escape), None);
    }

    #[test]
    fn combo_to_hotkey() {
        let combo = Combo::parse("Ctrl+Shift+F9").unwrap();
        assert_eq!(
            combo.hotkey(),
            HotKey::new(Some(HkMods::CONTROL | HkMods::SHIFT), Code::F9)
        );
        let combo = Combo::parse("Alt+Space").unwrap();
        assert_eq!(combo.hotkey(), HotKey::new(Some(HkMods::ALT), Code::Space));
    }

    #[test]
    fn record_key_cases() {
        assert_eq!(
            record_key(Key::Escape, Modifiers::CTRL),
            RecordOutcome::Cancel
        );
        assert_eq!(
            record_key(Key::V, Modifiers::CTRL | Modifiers::ALT),
            RecordOutcome::Combo(Combo::parse("Ctrl+Alt+V").unwrap())
        );
        assert_eq!(
            record_key(Key::V, Modifiers::NONE),
            RecordOutcome::Invalid("Add Ctrl, Alt or Shift to the key.".into())
        );
    }
}
