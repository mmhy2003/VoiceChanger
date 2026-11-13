//! Where VoiceChanger keeps its settings, models and logs.

use std::path::{Path, PathBuf};

/// Folder name under `%APPDATA%` and `%LOCALAPPDATA%`; matches the window title.
pub const APP_NAME: &str = "VoiceChanger";

/// The app's folders. Nothing is created here; writers create what they need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppDirs {
    pub config: PathBuf,
    pub models: PathBuf,
    pub logs: PathBuf,
    /// Downloaded installers.
    pub updates: PathBuf,
}

impl AppDirs {
    /// `roaming/VoiceChanger/{config,models}` and `local/VoiceChanger/{logs,updates}`.
    pub fn under(roaming: &Path, local: &Path) -> Self {
        let app = roaming.join(APP_NAME);
        let local = local.join(APP_NAME);
        Self {
            config: app.join("config"),
            models: app.join("models"),
            logs: local.join("logs"),
            updates: local.join("updates"),
        }
    }

    /// Under `%APPDATA%` and `%LOCALAPPDATA%`, or the temp folder when Windows does not say.
    pub fn system() -> Self {
        let roaming = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
        let local = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
        Self::under(&roaming, &local)
    }

    /// `config/settings.toml`.
    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirs_layout() {
        let dirs = AppDirs::under(Path::new("C:/r"), Path::new("C:/l"));
        assert_eq!(
            dirs.config,
            Path::new("C:/r").join("VoiceChanger").join("config")
        );
        assert_eq!(
            dirs.models,
            Path::new("C:/r").join("VoiceChanger").join("models")
        );
        assert_eq!(
            dirs.logs,
            Path::new("C:/l").join("VoiceChanger").join("logs")
        );
        assert_eq!(
            dirs.updates,
            Path::new("C:/l").join("VoiceChanger").join("updates")
        );
        assert_eq!(dirs.settings_file(), dirs.config.join("settings.toml"));
    }
}
