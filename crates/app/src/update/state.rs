//! What the UI knows about updates: the check, the offer, the download and the install. It is pure:
//! the UI feeds it worker messages and user choices, and carries out the actions it returns.

use std::path::PathBuf;

use super::release::Update;
use super::schedule;
use super::version::Version;
use super::worker::{CheckOutcome, Command, Message};
use crate::config::UpdatesConfig;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Checking {
        manual: bool,
    },
    Available(Update),
    Downloading {
        update: Update,
        done: u64,
        total: u64,
        /// Install as soon as the download is ready (the user chose Update now).
        install: bool,
    },
    Ready {
        update: Update,
        path: PathBuf,
    },
    Installing,
}

/// What the UI must do next.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Send(Command),
    Install(PathBuf),
}

#[derive(Debug, Default)]
pub struct UpdateState {
    pub phase: Phase,
    pub window_open: bool,
    pub banner_hidden: bool,
    /// The last check result, shown in Settings → General.
    pub status: Option<String>,
    /// Shown in the update window.
    pub error: Option<String>,
}

impl UpdateState {
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking { .. } | Phase::Downloading { .. } | Phase::Installing
        )
    }

    /// Checks also run while an installer is ready, so a newer release is still seen.
    pub fn can_check(&self) -> bool {
        matches!(
            self.phase,
            Phase::Idle | Phase::Available(_) | Phase::Ready { .. }
        )
    }

    /// Ready stays Ready during the check, so the installer can still be run.
    pub fn checking(&mut self, manual: bool) {
        if !self.can_check() {
            return;
        }
        if !matches!(self.phase, Phase::Ready { .. }) {
            self.phase = Phase::Checking { manual };
        }
        if manual {
            self.status = None;
        }
    }

    pub fn apply(
        &mut self,
        msg: Message,
        prefs: &UpdatesConfig,
        current: Version,
    ) -> Option<Action> {
        match msg {
            Message::Checked { manual, outcome } => self.checked(manual, outcome, prefs, current),
            Message::Progress { done, total } => {
                if let Phase::Downloading {
                    done: d, total: t, ..
                } = &mut self.phase
                {
                    *d = done;
                    *t = total;
                }
                None
            }
            Message::Ready(path) => {
                let Phase::Downloading {
                    update, install, ..
                } = std::mem::take(&mut self.phase)
                else {
                    return None;
                };
                self.phase = Phase::Ready {
                    update,
                    path: path.clone(),
                };
                install.then_some(Action::Install(path))
            }
            Message::DownloadFailed(e) => {
                if let Some(install) = self.stop_download() {
                    self.error = Some(e);
                    self.window_open |= install;
                }
                None
            }
            Message::Cancelled => {
                self.stop_download();
                None
            }
        }
    }

    fn checked(
        &mut self,
        manual: bool,
        outcome: CheckOutcome,
        prefs: &UpdatesConfig,
        current: Version,
    ) -> Option<Action> {
        if matches!(self.phase, Phase::Downloading { .. } | Phase::Installing) {
            return None;
        }
        // A ready installer gives way only to a newer release that would be offered.
        if let Phase::Ready { update, .. } = &self.phase {
            let replaces = matches!(&outcome, CheckOutcome::Available(u)
            if u.version > update.version
                && schedule::should_notify(
                    u.version,
                    current,
                    prefs.skipped_version.as_deref(),
                    manual,
                ));
            if !replaces {
                if manual {
                    self.status = Some(format!(
                        "VoiceChanger {} is ready to install.",
                        update.version
                    ));
                }
                return None;
            }
        }
        self.phase = Phase::Idle;
        match outcome {
            CheckOutcome::Available(u) => {
                let version = u.version;
                if !schedule::should_notify(
                    version,
                    current,
                    prefs.skipped_version.as_deref(),
                    manual,
                ) {
                    self.status = Some(format!("VoiceChanger {version} is available (skipped)."));
                    return None;
                }
                self.banner_hidden = false;
                self.window_open = manual;
                self.error = None;
                self.status = Some(format!("VoiceChanger {version} is available."));
                if prefs.auto_download {
                    self.phase = Phase::Downloading {
                        total: u.installer.size,
                        update: u.clone(),
                        done: 0,
                        install: false,
                    };
                    Some(Action::Send(Command::Download(u)))
                } else {
                    self.phase = Phase::Available(u);
                    None
                }
            }
            CheckOutcome::UpToDate => {
                self.status = Some(format!("You have the latest version ({current})."));
                None
            }
            CheckOutcome::NotInstallable(r) => {
                self.status = Some(format!("No installable update: {r}."));
                None
            }
            CheckOutcome::Failed(r) => {
                if manual {
                    self.status = Some(format!("Could not check for updates: {r}"));
                }
                None
            }
        }
    }

    /// Downloading → Available; returns the download's `install` flag, or None if not downloading.
    fn stop_download(&mut self) -> Option<bool> {
        if !matches!(self.phase, Phase::Downloading { .. }) {
            return None;
        }
        let Phase::Downloading {
            update, install, ..
        } = std::mem::take(&mut self.phase)
        else {
            unreachable!("checked above");
        };
        self.phase = Phase::Available(update);
        Some(install)
    }

    pub fn update_now(&mut self) -> Option<Action> {
        self.error = None;
        self.window_open = true;
        match &mut self.phase {
            Phase::Available(u) => {
                let u = u.clone();
                self.phase = Phase::Downloading {
                    total: u.installer.size,
                    update: u.clone(),
                    done: 0,
                    install: true,
                };
                Some(Action::Send(Command::Download(u)))
            }
            Phase::Downloading { install, .. } => {
                *install = true;
                None
            }
            Phase::Ready { path, .. } => Some(Action::Install(path.clone())),
            _ => None,
        }
    }

    pub fn later(&mut self) {
        self.window_open = false;
    }

    /// Returns true if a download was under way, which the UI must then cancel.
    pub fn skip(&mut self, prefs: &mut UpdatesConfig) -> bool {
        if let Some(version) = self.update().map(|u| u.version) {
            prefs.skipped_version = Some(version.to_string());
            self.status = Some(format!("VoiceChanger {version} will be skipped."));
        }
        let downloading = matches!(self.phase, Phase::Downloading { .. });
        self.phase = Phase::Idle;
        self.window_open = false;
        downloading
    }

    pub fn cancel_download(&mut self) -> bool {
        self.stop_download().is_some()
    }

    pub fn update(&self) -> Option<&Update> {
        match &self.phase {
            Phase::Available(update)
            | Phase::Downloading { update, .. }
            | Phase::Ready { update, .. } => Some(update),
            _ => None,
        }
    }

    pub fn banner_text(&self) -> Option<String> {
        if self.banner_hidden {
            return None;
        }
        match &self.phase {
            Phase::Available(u) | Phase::Downloading { update: u, .. } => {
                Some(format!("VoiceChanger {} is available.", u.version))
            }
            Phase::Ready { update, .. } => Some(format!(
                "VoiceChanger {} is ready to install.",
                update.version
            )),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::release::{Asset, installer_name};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn cur() -> Version {
        v("1.0.0")
    }

    fn upd(s: &str) -> Update {
        let version = v(s);
        Update {
            version,
            title: format!("VoiceChanger {version}"),
            notes: "notes".into(),
            date: "2026-11-14".into(),
            installer: Asset {
                name: installer_name(version),
                browser_download_url: "https://example.invalid/setup.exe".into(),
                size: 100,
            },
            checksums: Asset {
                name: "SHA256SUMS.txt".into(),
                browser_download_url: "https://example.invalid/sums".into(),
                size: 90,
            },
        }
    }

    fn checked(manual: bool, outcome: CheckOutcome) -> Message {
        Message::Checked { manual, outcome }
    }

    fn available(manual: bool, s: &str) -> Message {
        checked(manual, CheckOutcome::Available(upd(s)))
    }

    /// A state that has offered `s` after an automatic check.
    fn offered(s: &str) -> UpdateState {
        let mut st = UpdateState::default();
        st.checking(false);
        st.apply(available(false, s), &UpdatesConfig::default(), cur());
        st
    }

    fn path() -> PathBuf {
        PathBuf::from("C:/updates/Setup-VoiceChanger-1.1.0.exe")
    }

    #[test]
    fn auto_available_shows_banner_not_window() {
        let mut st = UpdateState::default();
        st.checking(false);
        let action = st.apply(available(false, "1.1.0"), &UpdatesConfig::default(), cur());
        assert_eq!(action, None);
        assert_eq!(st.phase, Phase::Available(upd("1.1.0")));
        assert!(!st.window_open);
        assert_eq!(
            st.banner_text().as_deref(),
            Some("VoiceChanger 1.1.0 is available.")
        );
        assert_eq!(
            st.status.as_deref(),
            Some("VoiceChanger 1.1.0 is available.")
        );
    }

    #[test]
    fn manual_available_opens_window() {
        let mut st = UpdateState::default();
        st.checking(true);
        let action = st.apply(available(true, "1.1.0"), &UpdatesConfig::default(), cur());
        assert_eq!(action, None);
        assert_eq!(st.phase, Phase::Available(upd("1.1.0")));
        assert!(st.window_open);
    }

    #[test]
    fn skipped_version_quiet_unless_manual() {
        let prefs = UpdatesConfig {
            skipped_version: Some("1.1.0".into()),
            ..UpdatesConfig::default()
        };
        let mut st = UpdateState::default();
        st.checking(false);
        assert_eq!(st.apply(available(false, "1.1.0"), &prefs, cur()), None);
        assert_eq!(st.phase, Phase::Idle);
        assert_eq!(st.banner_text(), None);
        assert_eq!(
            st.status.as_deref(),
            Some("VoiceChanger 1.1.0 is available (skipped).")
        );

        st.checking(true);
        assert_eq!(st.apply(available(true, "1.1.0"), &prefs, cur()), None);
        assert_eq!(st.phase, Phase::Available(upd("1.1.0")));
        assert!(st.window_open);
    }

    #[test]
    fn auto_download_starts_download() {
        let prefs = UpdatesConfig {
            auto_download: true,
            ..UpdatesConfig::default()
        };
        let mut st = UpdateState::default();
        st.checking(false);
        let action = st.apply(available(false, "1.1.0"), &prefs, cur());
        assert_eq!(action, Some(Action::Send(Command::Download(upd("1.1.0")))));
        assert_eq!(
            st.phase,
            Phase::Downloading {
                update: upd("1.1.0"),
                done: 0,
                total: 100,
                install: false
            }
        );
        assert_eq!(st.apply(Message::Ready(path()), &prefs, cur()), None);
        assert_eq!(
            st.phase,
            Phase::Ready {
                update: upd("1.1.0"),
                path: path()
            }
        );
        assert_eq!(
            st.banner_text().as_deref(),
            Some("VoiceChanger 1.1.0 is ready to install.")
        );
    }

    #[test]
    fn update_now_downloads_then_installs() {
        let prefs = UpdatesConfig::default();
        let mut st = offered("1.1.0");
        st.error = Some("old".into());
        assert_eq!(
            st.update_now(),
            Some(Action::Send(Command::Download(upd("1.1.0"))))
        );
        assert!(st.window_open);
        assert_eq!(st.error, None);
        let progress = Message::Progress {
            done: 50,
            total: 100,
        };
        assert_eq!(st.apply(progress, &prefs, cur()), None);
        assert_eq!(
            st.phase,
            Phase::Downloading {
                update: upd("1.1.0"),
                done: 50,
                total: 100,
                install: true
            }
        );
        assert_eq!(
            st.apply(Message::Ready(path()), &prefs, cur()),
            Some(Action::Install(path()))
        );
    }

    #[test]
    fn update_now_while_auto_downloading_joins() {
        let prefs = UpdatesConfig {
            auto_download: true,
            ..UpdatesConfig::default()
        };
        let mut st = UpdateState::default();
        st.checking(false);
        st.apply(available(false, "1.1.0"), &prefs, cur());
        assert_eq!(st.update_now(), None);
        assert!(matches!(st.phase, Phase::Downloading { install: true, .. }));
        assert!(st.window_open);
        assert_eq!(
            st.apply(Message::Ready(path()), &prefs, cur()),
            Some(Action::Install(path()))
        );
    }

    #[test]
    fn update_now_when_ready_installs() {
        let mut st = UpdateState {
            phase: Phase::Ready {
                update: upd("1.1.0"),
                path: path(),
            },
            ..UpdateState::default()
        };
        assert_eq!(st.update_now(), Some(Action::Install(path())));
        assert!(st.window_open);
    }

    #[test]
    fn download_failed_back_to_available_with_error() {
        let prefs = UpdatesConfig::default();
        for install in [true, false] {
            let mut st = UpdateState {
                phase: Phase::Downloading {
                    update: upd("1.1.0"),
                    done: 10,
                    total: 100,
                    install,
                },
                ..UpdateState::default()
            };
            let msg = Message::DownloadFailed("bad hash".into());
            assert_eq!(st.apply(msg, &prefs, cur()), None);
            assert_eq!(st.phase, Phase::Available(upd("1.1.0")));
            assert_eq!(st.window_open, install);
            assert_eq!(st.error.as_deref(), Some("bad hash"));
        }
    }

    #[test]
    fn cancel() {
        let prefs = UpdatesConfig::default();
        let downloading = || UpdateState {
            phase: Phase::Downloading {
                update: upd("1.1.0"),
                done: 10,
                total: 100,
                install: true,
            },
            ..UpdateState::default()
        };
        let mut st = downloading();
        assert!(st.cancel_download());
        assert_eq!(st.phase, Phase::Available(upd("1.1.0")));

        let mut st = downloading();
        assert_eq!(st.apply(Message::Cancelled, &prefs, cur()), None);
        assert_eq!(st.phase, Phase::Available(upd("1.1.0")));

        let mut st = UpdateState::default();
        assert!(!st.cancel_download());
        assert_eq!(st.phase, Phase::Idle);
    }

    #[test]
    fn up_to_date_and_failures() {
        let prefs = UpdatesConfig::default();
        let mut st = UpdateState::default();

        st.checking(false);
        st.apply(checked(false, CheckOutcome::UpToDate), &prefs, cur());
        assert_eq!(st.phase, Phase::Idle);
        assert_eq!(
            st.status.as_deref(),
            Some("You have the latest version (1.0.0).")
        );

        st.checking(false);
        st.apply(
            checked(false, CheckOutcome::Failed("offline".into())),
            &prefs,
            cur(),
        );
        assert_eq!(st.phase, Phase::Idle);
        assert_eq!(
            st.status.as_deref(),
            Some("You have the latest version (1.0.0).")
        );

        st.checking(true);
        assert_eq!(st.status, None);
        st.apply(
            checked(true, CheckOutcome::Failed("offline".into())),
            &prefs,
            cur(),
        );
        assert_eq!(st.phase, Phase::Idle);
        assert_eq!(
            st.status.as_deref(),
            Some("Could not check for updates: offline")
        );

        st.checking(false);
        st.apply(
            checked(false, CheckOutcome::NotInstallable("r".into())),
            &prefs,
            cur(),
        );
        assert_eq!(st.phase, Phase::Idle);
        assert_eq!(st.status.as_deref(), Some("No installable update: r."));
    }

    #[test]
    fn checks_ignored_while_busy() {
        let prefs = UpdatesConfig::default();
        let downloading = Phase::Downloading {
            update: upd("1.1.0"),
            done: 10,
            total: 100,
            install: false,
        };
        let mut st = UpdateState {
            phase: downloading.clone(),
            ..UpdateState::default()
        };
        assert!(!st.can_check());
        st.checking(true);
        assert_eq!(st.apply(available(true, "1.2.0"), &prefs, cur()), None);
        assert_eq!(st.phase, downloading);
        let busy = |phase| UpdateState {
            phase,
            ..UpdateState::default()
        };
        assert!(busy(downloading.clone()).busy());
        assert!(
            !busy(Phase::Ready {
                update: upd("1.1.0"),
                path: path()
            })
            .busy()
        );
        assert!(busy(Phase::Installing).busy());
        assert!(busy(Phase::Checking { manual: false }).busy());
        assert!(!UpdateState::default().busy());
        assert!(UpdateState::default().can_check());
        assert!(busy(Phase::Available(upd("1.1.0"))).can_check());
    }

    fn ready() -> UpdateState {
        UpdateState {
            phase: Phase::Ready {
                update: upd("1.1.0"),
                path: path(),
            },
            ..UpdateState::default()
        }
    }

    #[test]
    fn checks_while_ready_keep_the_installer() {
        let prefs = UpdatesConfig::default();
        let mut st = ready();
        assert!(st.can_check());
        st.checking(false);
        assert_eq!(st.phase, ready().phase);
        for outcome in [
            CheckOutcome::Available(upd("1.1.0")),
            CheckOutcome::UpToDate,
            CheckOutcome::Failed("offline".into()),
        ] {
            assert_eq!(st.apply(checked(false, outcome), &prefs, cur()), None);
            assert_eq!(st.phase, ready().phase);
        }
        // A newer version that was skipped does not replace the ready installer either.
        let skipped = UpdatesConfig {
            skipped_version: Some("1.2.0".into()),
            ..UpdatesConfig::default()
        };
        assert_eq!(st.apply(available(false, "1.2.0"), &skipped, cur()), None);
        assert_eq!(st.phase, ready().phase);
    }

    #[test]
    fn newer_release_while_ready_replaces_the_offer() {
        let mut st = ready();
        st.checking(false);
        assert_eq!(
            st.apply(available(false, "1.2.0"), &UpdatesConfig::default(), cur()),
            None
        );
        assert_eq!(st.phase, Phase::Available(upd("1.2.0")));

        let prefs = UpdatesConfig {
            auto_download: true,
            ..UpdatesConfig::default()
        };
        let mut st = ready();
        st.checking(false);
        assert_eq!(
            st.apply(available(false, "1.2.0"), &prefs, cur()),
            Some(Action::Send(Command::Download(upd("1.2.0"))))
        );
        assert!(matches!(st.phase, Phase::Downloading { .. }));
    }

    #[test]
    fn skip_while_downloading_stops_the_download() {
        let mut prefs = UpdatesConfig::default();
        let mut st = offered("1.1.0");
        st.update_now();
        assert!(st.skip(&mut prefs));
        assert_eq!(st.phase, Phase::Idle);
        assert_eq!(prefs.skipped_version.as_deref(), Some("1.1.0"));
        // The worker's late answers change nothing.
        assert_eq!(st.apply(Message::Cancelled, &prefs, cur()), None);
        assert_eq!(st.apply(Message::Ready(path()), &prefs, cur()), None);
        assert_eq!(st.phase, Phase::Idle);

        let mut st = offered("1.1.0");
        assert!(!st.skip(&mut prefs));
    }

    #[test]
    fn skip_saves_version() {
        let mut prefs = UpdatesConfig::default();
        let mut st = offered("1.1.0");
        st.window_open = true;
        st.skip(&mut prefs);
        assert_eq!(prefs.skipped_version.as_deref(), Some("1.1.0"));
        assert_eq!(st.phase, Phase::Idle);
        assert!(!st.window_open);
        assert_eq!(
            st.status.as_deref(),
            Some("VoiceChanger 1.1.0 will be skipped.")
        );
        assert_eq!(st.update(), None);
    }

    #[test]
    fn later_closes_window() {
        let mut st = offered("1.1.0");
        st.window_open = true;
        st.later();
        assert!(!st.window_open);
        assert_eq!(st.update(), Some(&upd("1.1.0")));
    }

    #[test]
    fn banner_hidden_hides_text() {
        let mut st = offered("1.1.0");
        assert!(st.banner_text().is_some());
        st.banner_hidden = true;
        assert_eq!(st.banner_text(), None);
        // A new offer shows the banner again.
        st.checking(false);
        st.apply(available(false, "1.1.0"), &UpdatesConfig::default(), cur());
        assert!(st.banner_text().is_some());
    }
}
