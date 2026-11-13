//! Native file pickers and Explorer.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// Opens the native file picker on a "vc-file-dialog" thread; sends `None` when cancelled.
/// rfd initialises COM, so it stays on its own thread: the UI thread is STA for OLE drag-and-drop.
/// A spawn failure yields a closed channel.
pub fn pick_file(
    title: &'static str,
    filter: &'static str,
    exts: &'static [&'static str],
) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("vc-file-dialog".into())
        .spawn(move || {
            let path = rfd::FileDialog::new()
                .set_title(title)
                .add_filter(filter, exts)
                .pick_file();
            let _ = tx.send(path);
        });
    rx
}

/// Opens `dir` in Explorer, creating it first; errors are logged.
pub fn open_folder(dir: &Path) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("cannot create {}: {e}", dir.display());
    }
    if let Err(e) = std::process::Command::new("explorer.exe").arg(dir).spawn() {
        tracing::warn!("cannot open {} in Explorer: {e}", dir.display());
    }
}
