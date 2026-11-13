#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! VoiceChanger desktop app.

mod app;
mod config;
mod engine;
mod files;
mod import;
mod model;
mod paths;
mod status_bar;
mod tabs;
#[cfg(test)]
mod testutil;
mod widgets;

use std::path::Path;

use eframe::egui;
use tracing::Level;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

fn main() -> eframe::Result {
    let _guard = init_logging(&paths::AppDirs::system().logs);
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "VoiceChanger starting");
    let dirs = paths::AppDirs::system();
    let (config, note) = config::load(&dirs.settings_file());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app::window_title())
            .with_inner_size([720.0, 520.0])
            .with_min_inner_size([520.0, 400.0]),
        ..Default::default()
    };
    let startup = app::Startup {
        dirs,
        config,
        notes: note.into_iter().collect(),
    };
    eframe::run_native(
        &app::window_title(),
        options,
        Box::new(|cc| Ok(Box::new(app::VcApp::new(cc, startup)))),
    )
}

/// Logs to daily files in `dir` (seven kept), and to stderr in debug builds.
/// Falls back to stderr only when the folder or the file can't be set up.
/// Keep the guard alive until exit so buffered lines are flushed.
fn init_logging(dir: &Path) -> Option<WorkerGuard> {
    let appender = std::fs::create_dir_all(dir)
        .map_err(|e| e.to_string())
        .and_then(|()| {
            RollingFileAppender::builder()
                .rotation(Rotation::DAILY)
                .filename_prefix("voicechanger.log")
                .max_log_files(7)
                .build(dir)
                .map_err(|e| e.to_string())
        });
    let appender = match appender {
        Ok(appender) => appender,
        Err(e) => {
            tracing_subscriber::fmt()
                .with_max_level(Level::INFO)
                .with_writer(std::io::stderr)
                .init();
            tracing::warn!(dir = %dir.display(), "log files disabled: {e}");
            return None;
        }
    };
    let (file, guard) = tracing_appender::non_blocking(appender);
    #[cfg(debug_assertions)]
    let writer = {
        use tracing_subscriber::fmt::writer::MakeWriterExt;
        file.and(std::io::stderr)
    };
    #[cfg(not(debug_assertions))]
    let writer = file;
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_ansi(false)
        .with_writer(writer)
        .init();
    Some(guard)
}
