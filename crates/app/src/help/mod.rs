//! In-app help: the guide's topics and entries, their ranges, and whether the Help window is open.

use std::ops::RangeInclusive;

use vc_engine::StreamSettings;
use vc_engine::stream::settings::{BLOCK_MS, CONTEXT_MS, CROSSFADE_MS, GATE_DB_RANGE};

use crate::app::{GAIN_DB_RANGE, GPU_RANGE, MONITOR_DB_RANGE, PerfSettings, Tab, VoiceSettings};

mod content;
pub mod ui;

/// The longest tip, in characters.
#[cfg(test)]
pub const SHORT_MAX: usize = 220;

/// A page of the Help window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    Start,
    Voice,
    Models,
    Convert,
    Devices,
    Performance,
    General,
    Troubleshooting,
}

impl Topic {
    /// All topics, in display order.
    pub const ALL: [Topic; 8] = [
        Topic::Start,
        Topic::Voice,
        Topic::Models,
        Topic::Convert,
        Topic::Devices,
        Topic::Performance,
        Topic::General,
        Topic::Troubleshooting,
    ];

    /// Label shown in the Help window's topic row.
    pub fn title(self) -> &'static str {
        match self {
            Topic::Start => "Getting started",
            Topic::Voice => "Voice",
            Topic::Models => "Models",
            Topic::Convert => "Convert",
            Topic::Devices => "Audio devices",
            Topic::Performance => "Performance",
            Topic::General => "General",
            Topic::Troubleshooting => "Troubleshooting",
        }
    }

    /// The topic Help opens on from `tab`.
    pub fn for_tab(tab: Tab) -> Topic {
        match tab {
            Tab::Voice => Topic::Voice,
            Tab::Models => Topic::Models,
            Tab::Convert => Topic::Convert,
            Tab::Settings => Topic::Devices,
        }
    }
}

/// One explained control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    // Voice tab and status bar
    Model,
    StartStop,
    Bypass,
    Pitch,
    InputGain,
    OutputGain,
    NoiseGate,
    Meters,
    Latency,
    Inference,
    LateBlocks,
    Underruns,
    // Models tab
    Import,
    OpenFolder,
    Reorder,
    ModelName,
    DefaultPitch,
    IconRenameDelete,
    // Convert tab
    ChooseFile,
    ConvertForm,
    ConvertButton,
    AfterConvert,
    // Settings > Audio devices
    Microphone,
    VirtualMic,
    Monitor,
    MonitorVolume,
    Refresh,
    // Settings > Performance
    Provider,
    Gpu,
    Block,
    ExtraContext,
    Crossfade,
    // Settings > General
    Hotkey,
    StartInTray,
    Updates,
    OpenLogs,
    About,
}

impl Id {
    /// Every entry, in the order above.
    #[cfg(test)]
    pub const ALL: [Id; 37] = [
        Id::Model,
        Id::StartStop,
        Id::Bypass,
        Id::Pitch,
        Id::InputGain,
        Id::OutputGain,
        Id::NoiseGate,
        Id::Meters,
        Id::Latency,
        Id::Inference,
        Id::LateBlocks,
        Id::Underruns,
        Id::Import,
        Id::OpenFolder,
        Id::Reorder,
        Id::ModelName,
        Id::DefaultPitch,
        Id::IconRenameDelete,
        Id::ChooseFile,
        Id::ConvertForm,
        Id::ConvertButton,
        Id::AfterConvert,
        Id::Microphone,
        Id::VirtualMic,
        Id::Monitor,
        Id::MonitorVolume,
        Id::Refresh,
        Id::Provider,
        Id::Gpu,
        Id::Block,
        Id::ExtraContext,
        Id::Crossfade,
        Id::Hotkey,
        Id::StartInTray,
        Id::Updates,
        Id::OpenLogs,
        Id::About,
    ];
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
        Topic::Start => content::START,
        Topic::Voice => content::VOICE,
        Topic::Models => content::MODELS,
        Topic::Convert => content::CONVERT,
        Topic::Devices => content::DEVICES,
        Topic::Performance => content::PERFORMANCE,
        Topic::General => content::GENERAL,
        Topic::Troubleshooting => content::TROUBLESHOOTING,
    }
}

/// The entry of `id`. Every `Id` has exactly one (tested).
pub fn item(id: Id) -> &'static Item {
    Topic::ALL
        .iter()
        .flat_map(|t| items(*t))
        .find(|i| i.id == Some(id))
        .expect("every Id has an entry")
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
        if plus && v > 0.0 {
            format!("+{v}")
        } else {
            format!("{v}")
        }
    };
    let unit = if unit.is_empty() {
        String::new()
    } else {
        format!(" {unit}")
    };
    let text = format!("{} to {}{unit}", num(*range.start()), num(*range.end()));
    match default {
        Some(d) => format!("{text} · default {}", num(d)),
        None => text,
    }
}

/// Whether the Help window is open and on which topic. Not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpState {
    pub open: bool,
    pub topic: Topic,
}

impl HelpState {
    /// Open on Getting started on a first run, closed otherwise.
    pub fn at_startup(first_run: bool) -> Self {
        Self {
            open: first_run,
            topic: Topic::Start,
        }
    }

    /// Closes the window if it is open; otherwise opens it on `tab`'s topic.
    pub fn toggle(&mut self, tab: Tab) {
        if self.open {
            self.open = false;
        } else {
            self.open_on(Topic::for_tab(tab));
        }
    }

    /// Opens the window on `topic`.
    pub fn open_on(&mut self, topic: Topic) {
        self.open = true;
        self.topic = topic;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_id_has_exactly_one_item() {
        for id in Id::ALL {
            let count = Topic::ALL
                .iter()
                .flat_map(|t| items(*t))
                .filter(|i| i.id == Some(id))
                .count();
            assert_eq!(count, 1, "{id:?}");
        }
    }

    #[test]
    fn item_finds_each_entry() {
        for id in Id::ALL {
            assert_eq!(item(id).id, Some(id));
        }
    }

    #[test]
    fn every_topic_has_items() {
        for t in Topic::ALL {
            assert!(!items(t).is_empty(), "{t:?}");
            let general = matches!(t, Topic::Start | Topic::Troubleshooting);
            for i in items(t) {
                assert_eq!(i.id.is_none(), general, "{t:?}: {}", i.name);
            }
        }
    }

    #[test]
    fn texts_are_filled_and_short() {
        for i in Topic::ALL.iter().flat_map(|t| items(*t)) {
            assert!(!i.name.is_empty() && i.name == i.name.trim(), "{}", i.name);
            assert!(
                !i.short.is_empty() && i.short == i.short.trim(),
                "{}",
                i.name
            );
            assert!(i.short.chars().count() <= SHORT_MAX, "{}", i.name);
        }
    }

    #[test]
    fn range_texts() {
        let expected = [
            (Id::Pitch, "-24 to +24 st · default 0"),
            (Id::DefaultPitch, "-24 to +24 st"),
            (Id::InputGain, "-20 to +20 dB · default 0"),
            (Id::OutputGain, "-20 to +20 dB · default 0"),
            (Id::NoiseGate, "-90 to -20 dB · default -50"),
            (Id::MonitorVolume, "-40 to 0 dB · default -6"),
            (Id::Gpu, "0 to 7 · default 0"),
            (Id::Block, "50 to 1000 ms · default 250"),
            (Id::ExtraContext, "0 to 2000 ms · default 500"),
            (Id::Crossfade, "10 to 200 ms · default 50"),
        ];
        for id in Id::ALL {
            let want = expected
                .iter()
                .find(|(e, _)| *e == id)
                .map(|(_, s)| s.to_string());
            assert_eq!(range_text(id), want, "{id:?}");
        }
    }

    #[test]
    fn topic_for_tab() {
        assert_eq!(Topic::for_tab(Tab::Voice), Topic::Voice);
        assert_eq!(Topic::for_tab(Tab::Models), Topic::Models);
        assert_eq!(Topic::for_tab(Tab::Convert), Topic::Convert);
        assert_eq!(Topic::for_tab(Tab::Settings), Topic::Devices);
    }

    #[test]
    fn help_state_at_startup() {
        assert_eq!(
            HelpState::at_startup(true),
            HelpState {
                open: true,
                topic: Topic::Start
            }
        );
        assert!(!HelpState::at_startup(false).open);
    }

    #[test]
    fn toggle_opens_on_tab_topic_and_closes() {
        let mut h = HelpState::at_startup(false);
        h.toggle(Tab::Settings);
        assert_eq!(
            h,
            HelpState {
                open: true,
                topic: Topic::Devices
            }
        );
        h.toggle(Tab::Settings);
        assert_eq!(
            h,
            HelpState {
                open: false,
                topic: Topic::Devices
            }
        );
    }

    #[test]
    fn open_on_switches_topic() {
        let mut h = HelpState {
            open: true,
            topic: Topic::Voice,
        };
        h.open_on(Topic::General);
        assert_eq!(
            h,
            HelpState {
                open: true,
                topic: Topic::General
            }
        );
    }
}
