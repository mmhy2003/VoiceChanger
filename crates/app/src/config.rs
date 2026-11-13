//! The settings file: devices, controls and preferences, saved as TOML a moment after they change.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use vc_audio::{DEFAULT_MONITOR_DB, DeviceRef};
use vc_engine::StreamSettings;

use crate::app::{PerfSettings, ProviderChoice, VoiceSettings};

/// Version written to new files; a file with a higher version is set aside.
pub const CONFIG_VERSION: u32 = 1;
/// How long the settings must stay unchanged before they are written.
pub const SAVE_DELAY: Duration = Duration::from_secs(1);

/// Everything saved in `settings.toml`. Missing fields take their defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub devices: DevicesConfig,
    pub perf: PerfConfig,
    pub voice: VoiceSettings,
    /// Library ID of the last chosen model.
    pub last_model: Option<String>,
    /// Pitch per library ID; a `BTreeMap` so the file is written in a stable order.
    pub pitch_by_model: BTreeMap<String, i32>,
    pub hotkey: Option<String>,
    pub start_minimised: bool,
    pub updates: UpdatesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            devices: DevicesConfig::default(),
            perf: PerfConfig::default(),
            voice: VoiceSettings::default(),
            last_model: None,
            pitch_by_model: BTreeMap::new(),
            hotkey: None,
            start_minimised: false,
            updates: UpdatesConfig::default(),
        }
    }
}

/// The `[updates]` table: automatic checks and downloads, and what the last check left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatesConfig {
    pub auto_check: bool,
    pub auto_download: bool,
    /// Unix seconds of the last check; 0 = never.
    pub last_check: u64,
    /// A version the user chose to skip.
    pub skipped_version: Option<String>,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            auto_check: true,
            auto_download: false,
            last_check: 0,
            skipped_version: None,
        }
    }
}

/// The chosen devices and the monitor switch and volume.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevicesConfig {
    pub input: Option<SavedDevice>,
    pub cable: Option<SavedDevice>,
    pub monitor: Option<SavedDevice>,
    pub monitor_enabled: bool,
    /// Monitor volume in dB.
    pub monitor_db: f32,
}

impl Default for DevicesConfig {
    fn default() -> Self {
        Self {
            input: None,
            cable: None,
            monitor: None,
            monitor_enabled: false,
            monitor_db: DEFAULT_MONITOR_DB,
        }
    }
}

/// A device as saved: endpoint ID, with the name as the fallback match.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedDevice {
    pub id: String,
    pub name: String,
}

impl From<&DeviceRef> for SavedDevice {
    fn from(r: &DeviceRef) -> Self {
        Self {
            id: r.id.clone(),
            name: r.name.clone(),
        }
    }
}

impl From<SavedDevice> for DeviceRef {
    fn from(d: SavedDevice) -> Self {
        Self {
            id: d.id,
            name: d.name,
        }
    }
}

/// Settings → Performance as saved, flattened.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerfConfig {
    pub provider: ProviderChoice,
    pub gpu: i32,
    pub block_ms: u32,
    pub context_ms: u32,
    pub crossfade_ms: u32,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self::from_settings(&PerfSettings::default())
    }
}

impl PerfConfig {
    pub fn from_settings(s: &PerfSettings) -> Self {
        Self {
            provider: s.provider,
            gpu: s.gpu,
            block_ms: s.stream.block_ms,
            context_ms: s.stream.context_ms,
            crossfade_ms: s.stream.crossfade_ms,
        }
    }

    pub fn to_settings(self) -> PerfSettings {
        PerfSettings {
            provider: self.provider,
            gpu: self.gpu,
            stream: StreamSettings {
                block_ms: self.block_ms,
                context_ms: self.context_ms,
                crossfade_ms: self.crossfade_ms,
            },
        }
    }
}

/// The parts of the config that live outside `AppState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefs {
    pub last_model: Option<String>,
    pub pitch_by_model: BTreeMap<String, i32>,
    pub hotkey: Option<String>,
    pub start_minimised: bool,
    pub updates: UpdatesConfig,
}

impl Prefs {
    pub fn from_config(c: &Config) -> Self {
        Self {
            last_model: c.last_model.clone(),
            pitch_by_model: c.pitch_by_model.clone(),
            hotkey: c.hotkey.clone(),
            start_minimised: c.start_minimised,
            updates: c.updates.clone(),
        }
    }
}

/// Reads the settings. A missing file gives the defaults with no note. A file that is unreadable, corrupt or
/// from a newer version gives the defaults and a note; corrupt and newer files are renamed to `settings.toml.bad`.
pub fn load(path: &Path) -> (Config, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Config::default(), None),
        Err(e) => {
            tracing::warn!("cannot read {}: {e}", path.display());
            return (
                Config::default(),
                Some(format!("Settings could not be read ({e}); using defaults.")),
            );
        }
    };
    let note = match toml::from_str::<Config>(&text) {
        Ok(config) if config.version <= CONFIG_VERSION => return (config, None),
        Ok(config) => {
            tracing::warn!("{} has version {}", path.display(), config.version);
            "The settings were saved by a newer VoiceChanger (saved as settings.toml.bad); using defaults."
        }
        Err(e) => {
            tracing::warn!("cannot parse {}: {e}", path.display());
            "The settings file was unreadable (saved as settings.toml.bad); using defaults."
        }
    };
    if let Err(e) = vc_models::back_up_bad(path) {
        tracing::warn!("cannot back up {}: {e}", path.display());
    }
    (Config::default(), Some(note.to_string()))
}

impl Config {
    /// Writes the settings atomically, creating the folder.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        vc_models::write_atomic(path, text.as_bytes())
    }
}

/// Decides when to write the settings: once they differ from the saved copy and have settled for `SAVE_DELAY`.
pub struct SaveTracker {
    saved: Config,
    pending: Option<(Config, Instant)>,
}

impl SaveTracker {
    pub fn new(saved: Config) -> Self {
        Self {
            saved,
            pending: None,
        }
    }

    /// True when `current` differs from the saved copy and has not changed for `SAVE_DELAY`.
    pub fn poll(&mut self, current: &Config, now: Instant) -> bool {
        if *current == self.saved {
            self.pending = None;
            return false;
        }
        let since = match &self.pending {
            Some((pending, since)) if pending == current => *since,
            _ => {
                self.pending = Some((current.clone(), now));
                now
            }
        };
        now.saturating_duration_since(since) >= SAVE_DELAY
    }

    /// Records `saved` as what is on disk (also after a failed write, so it is not retried every frame).
    pub fn mark_saved(&mut self, saved: Config) {
        self.saved = saved;
        self.pending = None;
    }

    /// True while a change is waiting for its delay.
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// True when `current` differs from the saved copy.
    pub fn needs_save(&self, current: &Config) -> bool {
        *current != self.saved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn config_defaults() {
        let c = Config::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.devices.input, None);
        assert_eq!(c.devices.cable, None);
        assert_eq!(c.devices.monitor, None);
        assert!(!c.devices.monitor_enabled);
        assert_eq!(c.devices.monitor_db, DEFAULT_MONITOR_DB);
        assert_eq!(c.perf.to_settings(), PerfSettings::default());
        assert_eq!(c.voice.gate_db, -50.0);
        assert_eq!(c.voice, VoiceSettings::default());
        assert_eq!(c.last_model, None);
        assert!(c.pitch_by_model.is_empty());
        assert_eq!(c.hotkey, None);
        assert!(!c.start_minimised);
        assert_eq!(c.updates, UpdatesConfig::default());
        assert!(c.updates.auto_check);
        assert!(!c.updates.auto_download);
        assert_eq!(c.updates.last_check, 0);
        assert_eq!(c.updates.skipped_version, None);
    }

    fn sample() -> Config {
        Config {
            devices: DevicesConfig {
                input: Some(SavedDevice {
                    id: "wasapi:{in}".into(),
                    name: "USB Mic".into(),
                }),
                cable: Some(SavedDevice {
                    id: "wasapi:{cable}".into(),
                    name: "CABLE Input".into(),
                }),
                monitor: Some(SavedDevice {
                    id: String::new(),
                    name: "Headphones".into(),
                }),
                monitor_enabled: true,
                monitor_db: -6.5,
            },
            perf: PerfConfig {
                provider: ProviderChoice::Cuda,
                gpu: 1,
                block_ms: 300,
                context_ms: 400,
                crossfade_ms: 60,
            },
            voice: VoiceSettings {
                pitch: 5,
                input_db: 2.0,
                output_db: -3.0,
                gate_db: -40.0,
            },
            last_model: Some("ana-v2".into()),
            pitch_by_model: [("ana-v2".to_string(), 5), ("bob".to_string(), -12)].into(),
            hotkey: Some("Ctrl+Alt+V".into()),
            start_minimised: true,
            updates: UpdatesConfig {
                auto_check: false,
                auto_download: true,
                last_check: 1_763_100_000,
                skipped_version: Some("1.1.0".into()),
            },
            ..Config::default()
        }
    }

    #[test]
    fn config_round_trip() {
        let dir = TempDir::new("config-round-trip");
        let path = dir.path().join("config").join("settings.toml");
        let config = sample();
        config.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("provider = \"cuda\""), "{text}");
        assert!(text.contains("[updates]"), "{text}");
        assert!(text.contains("last_check = 1763100000"), "{text}");
        assert_eq!(load(&path), (config, None));
    }

    #[test]
    fn missing_file_gives_defaults_without_note() {
        let dir = TempDir::new("config-missing");
        assert_eq!(
            load(&dir.path().join("settings.toml")),
            (Config::default(), None)
        );
    }

    #[test]
    fn missing_fields_take_defaults() {
        let dir = TempDir::new("config-missing-fields");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "[voice]\npitch = 3\n").unwrap();
        let (config, note) = load(&path);
        assert_eq!(note, None);
        let mut expected = Config::default();
        expected.voice.pitch = 3;
        assert_eq!(config, expected);
    }

    #[test]
    fn missing_updates_table_takes_defaults() {
        let dir = TempDir::new("config-missing-updates");
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "[voice]
pitch = 3
",
        )
        .unwrap();
        let (config, note) = load(&path);
        assert_eq!(note, None);
        assert_eq!(config.updates, UpdatesConfig::default());
    }

    #[test]
    fn partial_updates_table() {
        let dir = TempDir::new("config-partial-updates");
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "[updates]
auto_check = false
",
        )
        .unwrap();
        let (config, note) = load(&path);
        assert_eq!(note, None);
        assert_eq!(
            config.updates,
            UpdatesConfig {
                auto_check: false,
                ..UpdatesConfig::default()
            }
        );
    }

    #[test]
    fn corrupt_settings_backed_up() {
        let dir = TempDir::new("config-corrupt");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "this is = = not toml").unwrap();
        let (config, note) = load(&path);
        assert_eq!(config, Config::default());
        assert_eq!(
            note.as_deref(),
            Some("The settings file was unreadable (saved as settings.toml.bad); using defaults.")
        );
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("settings.toml.bad")).unwrap(),
            "this is = = not toml"
        );
    }

    #[test]
    fn newer_settings_backed_up() {
        let dir = TempDir::new("config-newer");
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "version = 99\n[voice]\npitch = 3\n").unwrap();
        let (config, note) = load(&path);
        assert_eq!(config, Config::default());
        assert_eq!(
            note.as_deref(),
            Some(
                "The settings were saved by a newer VoiceChanger (saved as settings.toml.bad); using defaults."
            )
        );
        assert!(!path.exists());
        assert!(dir.path().join("settings.toml.bad").exists());
    }

    #[test]
    fn unreadable_settings_note() {
        let dir = TempDir::new("config-unreadable");
        // A folder where the file should be cannot be read as a string.
        let path = dir.path().join("settings.toml");
        std::fs::create_dir(&path).unwrap();
        let (config, note) = load(&path);
        assert_eq!(config, Config::default());
        let note = note.unwrap();
        assert!(note.starts_with("Settings could not be read ("), "{note}");
        assert!(note.ends_with("); using defaults."), "{note}");
        assert!(path.is_dir());
    }

    #[test]
    fn perf_config_round_trip() {
        let settings = PerfSettings {
            provider: ProviderChoice::Cpu,
            gpu: 2,
            stream: StreamSettings {
                block_ms: 120,
                context_ms: 800,
                crossfade_ms: 30,
            },
        };
        let perf = PerfConfig::from_settings(&settings);
        assert_eq!(
            perf,
            PerfConfig {
                provider: ProviderChoice::Cpu,
                gpu: 2,
                block_ms: 120,
                context_ms: 800,
                crossfade_ms: 30,
            }
        );
        assert_eq!(perf.to_settings(), settings);
    }

    #[test]
    fn saved_device_converts_both_ways() {
        let r = DeviceRef {
            id: "wasapi:{x}".into(),
            name: "Mic".into(),
        };
        let saved = SavedDevice::from(&r);
        assert_eq!(
            saved,
            SavedDevice {
                id: "wasapi:{x}".into(),
                name: "Mic".into(),
            }
        );
        assert_eq!(DeviceRef::from(saved), r);
    }

    #[test]
    fn prefs_from_config() {
        let config = sample();
        assert_eq!(
            Prefs::from_config(&config),
            Prefs {
                last_model: Some("ana-v2".into()),
                pitch_by_model: config.pitch_by_model.clone(),
                hotkey: Some("Ctrl+Alt+V".into()),
                start_minimised: true,
                updates: config.updates.clone(),
            }
        );
    }

    #[test]
    fn save_tracker_debounces() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let base = Config::default();
        let mut a = base.clone();
        a.voice.pitch = 1;
        let mut b = base.clone();
        b.voice.pitch = 2;

        let mut tracker = SaveTracker::new(base.clone());
        assert!(!tracker.poll(&base, at(0)));
        assert!(!tracker.is_pending());
        assert!(!tracker.needs_save(&base));

        assert!(!tracker.poll(&a, at(0)));
        assert!(tracker.is_pending());
        assert!(tracker.needs_save(&a));
        assert!(!tracker.poll(&a, at(500)));
        assert!(!tracker.poll(&b, at(800)));
        assert!(!tracker.poll(&b, at(1500)));
        assert!(tracker.poll(&b, at(1800)));

        tracker.mark_saved(b.clone());
        assert!(!tracker.poll(&b, at(3000)));
        assert!(!tracker.is_pending());
        assert!(!tracker.needs_save(&b));
    }

    #[test]
    fn save_tracker_change_back_cancels() {
        let t0 = Instant::now();
        let base = Config::default();
        let mut a = base.clone();
        a.start_minimised = true;
        let mut tracker = SaveTracker::new(base.clone());
        assert!(!tracker.poll(&a, t0));
        assert!(!tracker.poll(&base, t0 + Duration::from_millis(500)));
        assert!(!tracker.is_pending());
        assert!(!tracker.poll(&base, t0 + Duration::from_secs(5)));
    }
}
