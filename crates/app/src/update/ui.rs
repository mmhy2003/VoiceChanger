//! The update banner, the "Update available" window and the Settings → General rows.

use eframe::egui;

use super::state::Phase;
use super::version::Version;
use crate::app::VcApp;

/// One line above the tabs while an update is offered, with View… and ✕.
pub fn banner(ui: &mut egui::Ui, app: &mut VcApp) {
    let Some(text) = app.updates.banner_text() else {
        return;
    };
    ui.horizontal(|ui| {
        ui.label(text);
        if ui.link("View…").clicked() {
            app.updates.window_open = true;
        }
        if ui.small_button("✕").clicked() {
            app.updates.banner_hidden = true;
        }
    });
}

/// The "Update available" window: the release, its notes, the three choices and the download.
pub fn window(ctx: &egui::Context, app: &mut VcApp) {
    let mut open = app.updates.window_open;
    let mut action = None;
    egui::Window::new("Update available")
        .collapsible(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(update) = app.updates.update() {
                ui.heading(&update.title);
                let current = Version::current();
                ui.label(if update.date.is_empty() {
                    format!("You have {current}.")
                } else {
                    format!("You have {current}. Released {}.", update.date)
                });
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| ui.label(&update.notes));
            }
            if app.is_running() {
                ui.colored_label(ui.visuals().warn_fg_color, "Updating stops the voice.");
            }
            let installing = matches!(app.updates.phase, Phase::Installing);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!installing, egui::Button::new("Update now"))
                    .clicked()
                {
                    action = app.updates.update_now();
                }
                if ui.button("Later").clicked() {
                    app.updates.later();
                }
                if ui.button("Skip this version").clicked()
                    && app.updates.skip(&mut app.prefs.updates)
                {
                    app.updater.cancel();
                }
            });
            match &app.updates.phase {
                Phase::Downloading { done, total, .. } => {
                    let fraction = if *total == 0 {
                        0.0
                    } else {
                        *done as f32 / *total as f32
                    };
                    let text = progress_text(*done, *total);
                    ui.horizontal(|ui| {
                        ui.add(egui::ProgressBar::new(fraction).text(text));
                        if ui.button("Cancel").clicked() && app.updates.cancel_download() {
                            app.updater.cancel();
                        }
                    });
                }
                Phase::Installing => {
                    ui.label("Starting the installer…");
                }
                _ => {}
            }
            if let Some(error) = &app.updates.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
        });
    // The title bar's close button works like Later.
    if !open {
        app.updates.later();
    }
    app.run_update_action(ctx, action);
}

/// Settings → General: automatic checks, background downloads and Check for updates.
pub fn settings_rows(ui: &mut egui::Ui, app: &mut VcApp) {
    let prefs = &mut app.prefs.updates;
    ui.checkbox(
        &mut prefs.auto_check,
        "Check for updates automatically (every 6 hours)",
    );
    ui.add_enabled(
        prefs.auto_check,
        egui::Checkbox::new(
            &mut prefs.auto_download,
            "Download updates in the background",
        ),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                app.updates.can_check(),
                egui::Button::new("Check for updates"),
            )
            .clicked()
        {
            app.check_for_updates(true);
        }
        if matches!(app.updates.phase, Phase::Checking { .. }) {
            ui.label("Checking…");
        } else if let Some(status) = &app.updates.status {
            ui.label(status);
        }
    });
}

/// Download progress in megabytes, e.g. "12.3 MB of 45.6 MB".
pub fn progress_text(done: u64, total: u64) -> String {
    let mb = |bytes: u64| bytes as f64 / 1e6;
    format!("{:.1} MB of {:.1} MB", mb(done), mb(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_text_mb() {
        assert_eq!(progress_text(12_345_678, 45_600_000), "12.3 MB of 45.6 MB");
        assert_eq!(progress_text(0, 0), "0.0 MB of 0.0 MB");
    }
}
