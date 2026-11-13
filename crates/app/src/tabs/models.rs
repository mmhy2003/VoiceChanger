//! Models tab: import, the sample-rate prompt, notices and the library: reorder, icons, rename,
//! default pitch and delete.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use vc_models::{LibraryError, PITCH_RANGE};

use crate::app::{Tab, VcApp};
use crate::files;
use crate::import::{Notice, offer_convert_text};
use crate::model::ModelSlot;
use crate::tabs::voice::khz;

/// The sample rates RVC v2 models use.
const RATES: [u32; 3] = [32000, 40000, 48000];

pub fn show(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.heading("Models");

    ui.horizontal(|ui| {
        if ui
            .add_enabled(app.import_pick_rx.is_none(), egui::Button::new("Import…"))
            .clicked()
        {
            app.pick_import();
        }
        if ui.button("Open folder").clicked() {
            files::open_folder(app.library.root());
        }
    });

    if let Some(src) = app.importer.needs_rate() {
        let file = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut choice = None;
        let mut cancel = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(format!("\"{file}\" does not say its sample rate. Pick it:"));
            ui.horizontal(|ui| {
                for sr in RATES {
                    if ui.button(khz(sr)).clicked() {
                        choice = Some(sr);
                    }
                }
                cancel = ui.button("Cancel").clicked();
            });
        });
        if let Some(sr) = choice {
            app.importer.choose_rate(sr, &app.library);
        } else if cancel {
            app.importer.cancel_rate();
        }
    }

    poll_icon_pick(ui.ctx(), app);
    notices(ui, app);

    ui.separator();
    if app.library.entries().is_empty() {
        ui.label("No models yet. Import an RVC v2 .onnx file or drop one on the window.");
    }
    if let Some(action) = rows(ui, app) {
        apply(app, action);
    }
}

/// The Models tab's own UI state.
#[derive(Default)]
pub struct ModelsUi {
    /// The model being renamed and the name typed so far.
    pub renaming: Option<(String, String)>,
    /// The model whose Delete waits for confirmation.
    pub confirm_delete: Option<String>,
    /// The model whose icon is being picked, and the picker's answer.
    pub icon_pick: Option<(String, Receiver<Option<PathBuf>>)>,
}

/// What a click on a library row asks for.
enum Action {
    Select(String),
    MoveUp(String),
    MoveDown(String),
    Pitch(String, i32),
    PickIcon(String),
    StartRename(String, String),
    SaveRename,
    CancelRename,
    AskDelete(String),
    Delete(String),
    CancelDelete,
}

/// Shows one row per library model and returns the action clicked, if any.
fn rows(ui: &mut egui::Ui, app: &mut VcApp) -> Option<Action> {
    let mut action = None;
    let entries = app.library.entries();
    let picking_icon = app.models_ui.icon_pick.is_some();
    for (i, entry) in entries.iter().enumerate() {
        let id = &entry.id;
        let exists = app.library.file_exists(entry);
        let is_active = app.prefs.last_model.as_deref() == Some(id.as_str());
        ui.horizontal(|ui| {
            if ui
                .add_enabled(exists && i > 0, egui::Button::new("▲"))
                .clicked()
            {
                action = Some(Action::MoveUp(id.clone()));
            }
            if ui
                .add_enabled(exists && i + 1 < entries.len(), egui::Button::new("▼"))
                .clicked()
            {
                action = Some(Action::MoveDown(id.clone()));
            }
            icon(ui, app.library.icon_path(entry), &entry.name);

            let renaming = app
                .models_ui
                .renaming
                .as_mut()
                .filter(|(renaming, _)| renaming == id);
            if let Some((_, name)) = renaming {
                let edit = ui.add(egui::TextEdit::singleline(name).desired_width(160.0));
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Save").clicked() || enter {
                    action = Some(Action::SaveRename);
                }
                if ui.button("Cancel").clicked() {
                    action = Some(Action::CancelRename);
                }
            } else if ui
                .add_enabled(exists, egui::Button::selectable(is_active, &entry.name))
                .clicked()
            {
                action = Some(Action::Select(id.clone()));
            }

            ui.label(khz(entry.sample_rate));
            ui.add_enabled_ui(exists, |ui| {
                ui.label("Default pitch");
                let mut pitch = entry.default_pitch;
                if ui
                    .add(egui::DragValue::new(&mut pitch).range(PITCH_RANGE))
                    .changed()
                {
                    action = Some(Action::Pitch(id.clone(), pitch));
                }
            });
            if ui
                .add_enabled(exists && !picking_icon, egui::Button::new("Icon…"))
                .clicked()
            {
                action = Some(Action::PickIcon(id.clone()));
            }
            if ui
                .add_enabled(exists, egui::Button::new("Rename"))
                .clicked()
            {
                action = Some(Action::StartRename(id.clone(), entry.name.clone()));
            }
            if ui.button("Delete").clicked() {
                action = Some(Action::AskDelete(id.clone()));
            }
            if !exists {
                ui.colored_label(ui.visuals().error_fg_color, "file missing");
            }
        });
        if app.models_ui.confirm_delete.as_ref() == Some(id) {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "Delete \"{}\"? This removes the model file.",
                        entry.name
                    ));
                    if ui.button("Delete").clicked() {
                        action = Some(Action::Delete(id.clone()));
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(Action::CancelDelete);
                    }
                });
            });
        }
    }
    action
}

/// The model's icon in 24 px, or its avatar letter.
fn icon(ui: &mut egui::Ui, path: Option<PathBuf>, name: &str) {
    let size = egui::vec2(ICON_SIZE, ICON_SIZE);
    match path {
        Some(p) => {
            ui.add(egui::Image::new(icon_uri(&p)).fit_to_exact_size(size));
        }
        None => {
            egui::Frame::new()
                .fill(ui.visuals().widgets.inactive.bg_fill)
                .corner_radius(ICON_SIZE / 2.0)
                .show(ui, |ui| {
                    ui.add_sized(
                        size,
                        egui::Label::new(egui::RichText::new(avatar_letter(name)).strong()),
                    );
                });
        }
    }
}

/// Side of a model icon, in points.
const ICON_SIZE: f32 = 24.0;

/// The URI egui's file loader reads an icon from.
fn icon_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

/// Carries out a row action; library errors become notices.
fn apply(app: &mut VcApp, action: Action) {
    match action {
        Action::Select(id) => app.select_model(&id),
        Action::MoveUp(id) => {
            let result = app.library.move_up(&id);
            report(app, "move", &id, result);
        }
        Action::MoveDown(id) => {
            let result = app.library.move_down(&id);
            report(app, "move", &id, result);
        }
        Action::Pitch(id, pitch) => {
            let result = app.library.set_default_pitch(&id, pitch);
            report(app, "set the pitch of", &id, result);
        }
        Action::PickIcon(id) => {
            let rx = files::pick_file("Choose an icon", "Images", &["png", "jpg", "jpeg"]);
            app.models_ui.icon_pick = Some((id, rx));
        }
        Action::StartRename(id, current) => app.models_ui.renaming = Some((id, current)),
        Action::SaveRename => {
            if let Some((id, new_name)) = app.models_ui.renaming.clone() {
                let result = app.library.rename(&id, &new_name);
                if report(app, "rename", &id, result) {
                    app.models_ui.renaming = None;
                }
            }
        }
        Action::CancelRename => app.models_ui.renaming = None,
        Action::AskDelete(id) => {
            let is_active = app.prefs.last_model.as_deref() == Some(id.as_str());
            match delete_blocked(is_active, app.is_running()) {
                Some(reason) => app.notices.push(Notice::Error(reason.to_string())),
                None => app.models_ui.confirm_delete = Some(id),
            }
        }
        Action::Delete(id) => {
            app.models_ui.confirm_delete = None;
            delete(app, &id);
        }
        Action::CancelDelete => app.models_ui.confirm_delete = None,
    }
}

/// Deletes a model and its file. The active model can only go while stopped; the slot is emptied
/// first so the file is not in use.
fn delete(app: &mut VcApp, id: &str) {
    let is_active = app.prefs.last_model.as_deref() == Some(id);
    if let Some(reason) = delete_blocked(is_active, app.is_running()) {
        app.notices.push(Notice::Error(reason.to_string()));
        return;
    }
    if is_active {
        app.model = ModelSlot::Empty;
        app.pending = None;
    }
    // A switch to the deleted model would swap in a voice the library no longer has.
    if app.pending.as_ref().is_some_and(|p| p.id == id) {
        app.pending = None;
    }
    let result = app.library.remove(id);
    if report(app, "delete", id, result) {
        if is_active {
            app.prefs.last_model = None;
        }
        app.prefs.pitch_by_model.remove(id);
    }
}

/// Turns a library error into a notice; true on success.
fn report(app: &mut VcApp, action: &str, id: &str, result: Result<(), LibraryError>) -> bool {
    match result {
        Ok(()) => true,
        Err(e) => {
            let name = app.library.get(id).map_or(id, |e| e.name.as_str());
            app.notices
                .push(Notice::Error(format!("Could not {action} \"{name}\": {e}")));
            false
        }
    }
}

/// Picks up the icon picker's answer and makes the image the model's icon.
fn poll_icon_pick(ctx: &egui::Context, app: &mut VcApp) {
    let Some((id, rx)) = &app.models_ui.icon_pick else {
        return;
    };
    let path = match rx.try_recv() {
        Err(TryRecvError::Empty) => return,
        Ok(path) => path,
        Err(TryRecvError::Disconnected) => None,
    };
    let id = id.clone();
    app.models_ui.icon_pick = None;
    let Some(path) = path else {
        return;
    };
    let result = app.library.set_icon(&id, &path);
    if report(app, "set the icon of", &id, result)
        && let Some(icon) = app.library.get(&id).and_then(|e| app.library.icon_path(e))
    {
        // The new image may have the old file name, so egui must read it again.
        ctx.forget_image(&icon_uri(&icon));
    }
}

/// Why the model cannot be deleted now.
pub fn delete_blocked(is_active: bool, running: bool) -> Option<&'static str> {
    (is_active && running).then_some("Stop the voice before deleting the active model.")
}

/// The first letter or digit, upper-cased; "?" when there is none.
pub fn avatar_letter(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map_or_else(|| "?".to_string(), |c| c.to_uppercase().collect())
}

/// Shows the notices, each with its buttons, and applies the one clicked.
fn notices(ui: &mut egui::Ui, app: &mut VcApp) {
    let mut dismiss = None;
    let mut convert = None;
    for (i, notice) in app.notices.iter().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| match notice {
                Notice::Error(msg) => {
                    ui.colored_label(ui.visuals().error_fg_color, msg);
                    if ui.button("Dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
                Notice::Info(msg) => {
                    ui.label(msg);
                    if ui.button("Dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
                Notice::OfferConvert(path) => {
                    ui.label(offer_convert_text(path));
                    if ui.button("Open in Convert").clicked() {
                        convert = Some((i, path.clone()));
                    }
                    if ui.button("Dismiss").clicked() {
                        dismiss = Some(i);
                    }
                }
            });
        });
    }
    if let Some((i, path)) = convert {
        app.notices.remove(i);
        app.convert_file = Some(path);
        app.tab = Tab::Convert;
    } else if let Some(i) = dismiss {
        app.notices.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_blocked_cases() {
        assert_eq!(
            delete_blocked(true, true),
            Some("Stop the voice before deleting the active model.")
        );
        assert_eq!(delete_blocked(true, false), None);
        assert_eq!(delete_blocked(false, true), None);
        assert_eq!(delete_blocked(false, false), None);
    }

    #[test]
    fn avatar_letter_cases() {
        assert_eq!(avatar_letter("ana"), "A");
        assert_eq!(avatar_letter("  9lives"), "9");
        assert_eq!(avatar_letter("!!"), "?");
        assert_eq!(avatar_letter(""), "?");
    }
}
