//! Starting the downloaded installer: silent, with a progress window, and it relaunches the app.

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// The Inno Setup command line: silent with a progress window, no message boxes, no reboot, and
/// `/RELAUNCH` so setup starts VoiceChanger again when it is done.
pub fn installer_args(log: &Path) -> Vec<OsString> {
    let mut log_arg = OsString::from("/LOG=");
    log_arg.push(log);
    let mut args: Vec<OsString> = ["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH"]
        .iter()
        .map(OsString::from)
        .collect();
    args.push(log_arg);
    args
}

/// Creates the log's folder and starts the installer. It never waits: the app closes next, so
/// setup can replace its files.
pub fn spawn_installer(exe: &Path, log: &Path) -> io::Result<()> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    Command::new(exe)
        .args(installer_args(log))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn installer_args_silent_relaunch() {
        let args = installer_args(Path::new("C:/l/update-install.log"));
        let expected: Vec<OsString> = [
            "/SILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/RELAUNCH",
            "/LOG=C:/l/update-install.log",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(args, expected);
    }

    #[test]
    fn spawn_missing_exe_errors() {
        let dir = TempDir::new("install-missing");
        let log = dir.path().join("logs").join("x.log");
        assert!(spawn_installer(&dir.path().join("nope.exe"), &log).is_err());
    }

    #[test]
    fn spawn_real_exe_ok() {
        let dir = TempDir::new("install-real");
        let log = dir.path().join("logs").join("x.log");
        spawn_installer(Path::new(r"C:\Windows\System32\where.exe"), &log).unwrap();
        assert!(dir.path().join("logs").is_dir());
    }
}
