//! Convert tab: choose a PyTorch model, check what it holds, convert it into the library, then use
//! or export it.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use vc_convert::Inspection;
use vc_models::PITCH_RANGE;

use crate::app::{Tab, VcApp};
use crate::convert::{Form, State, file_name, overall, stage_label};
use crate::files;
use crate::tabs::voice::khz;

/// The Convert tab's open pickers.
#[derive(Default)]
pub struct ConvertUi {
    /// The Choose file… picker's answer.
    pub pick_rx: Option<Receiver<Option<PathBuf>>>,
    /// The icon picker's answer.
    pub icon_pick_rx: Option<Receiver<Option<PathBuf>>>,
    /// The Export .onnx… dialog's answer.
    pub export_pick_rx: Option<Receiver<Option<PathBuf>>>,
}

impl ConvertUi {
    /// A picker is open.
    pub fn is_waiting(&self) -> bool {
        self.pick_rx.is_some() || self.icon_pick_rx.is_some() || self.export_pick_rx.is_some()
    }
}

/// What a click on the Convert tab asks for.
enum Action {
    Choose,
    PickIcon,
    ClearIcon,
    Convert,
    Cancel,
    UseNow(String),
    ShowModels,
    Export(String),
    Another,
    OpenLogs,
}

pub fn show(ui: &mut egui::Ui, app: &mut VcApp) {
    ui.heading("Convert");
    poll_picks(app);

    let picking = app.convert_ui.pick_rx.is_some();
    let picking_icon = app.convert_ui.icon_pick_rx.is_some();
    let picking_export = app.convert_ui.export_pick_rx.is_some();
    let mut action = None;
    match &mut app.converter.state {
        State::Empty => choose(ui, picking, &mut action),
        State::Inspecting { path, .. } => {
            choose(ui, picking, &mut action);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Reading {}…", file_name(path)));
            });
        }
        State::Ready { inspection, form } => {
            summary(ui, inspection);
            form_ui(ui, form, picking_icon, &mut action);
            if ui
                .add_enabled(!form.name.trim().is_empty(), egui::Button::new("Convert"))
                .clicked()
            {
                action = Some(Action::Convert);
            }
        }
        State::Converting {
            inspection,
            form,
            progress,
            ..
        } => {
            summary(ui, inspection);
            ui.add_enabled_ui(false, |ui| form_ui(ui, form, true, &mut action));
            ui.label(stage_label(progress.stage));
            ui.add(egui::ProgressBar::new(overall(*progress)).show_percentage());
            if ui.button("Cancel").clicked() {
                action = Some(Action::Cancel);
            }
        }
        State::Done {
            id,
            name,
            note,
            export_rx,
        } => {
            ui.label(format!("Added \"{name}\" to your models."));
            if let Some(note) = note {
                ui.label(note.as_str());
            }
            ui.horizontal(|ui| {
                if ui.button("Use now").clicked() {
                    action = Some(Action::UseNow(id.clone()));
                }
                if ui.button("Show in Models").clicked() {
                    action = Some(Action::ShowModels);
                }
                let exporting = export_rx.is_some() || picking_export;
                if ui
                    .add_enabled(!exporting, egui::Button::new("Export .onnx…"))
                    .clicked()
                {
                    action = Some(Action::Export(id.clone()));
                }
                if ui
                    .add_enabled(!picking, egui::Button::new("Convert another…"))
                    .clicked()
                {
                    action = Some(Action::Another);
                }
                if export_rx.is_some() {
                    ui.spinner();
                }
            });
        }
        State::Problem { file, message } => {
            ui.label(file.as_str());
            ui.colored_label(ui.visuals().error_fg_color, message.as_str());
            ui.horizontal(|ui| {
                if ui.button("Open logs folder").clicked() {
                    action = Some(Action::OpenLogs);
                }
                if ui
                    .add_enabled(!picking, egui::Button::new("Choose another file…"))
                    .clicked()
                {
                    action = Some(Action::Choose);
                }
            });
        }
    }
    if let Some(action) = action {
        apply(app, action);
    }
}

/// Choose file… and the drop hint.
fn choose(ui: &mut egui::Ui, picking: bool, action: &mut Option<Action>) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!picking, egui::Button::new("Choose file…"))
            .clicked()
        {
            *action = Some(Action::Choose);
        }
        ui.label("or drop a .pth or .zip file on the window");
    });
}

/// What the file holds, one row per line of [`summary_lines`].
fn summary(ui: &mut egui::Ui, inspection: &Inspection) {
    egui::Grid::new("convert-summary")
        .num_columns(2)
        .show(ui, |ui| {
            for (label, value) in summary_lines(inspection) {
                ui.label(label);
                ui.label(value);
                ui.end_row();
            }
        });
    ui.separator();
}

/// Name, default pitch and icon of the new model.
fn form_ui(ui: &mut egui::Ui, form: &mut Form, picking_icon: bool, action: &mut Option<Action>) {
    egui::Grid::new("convert-form")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Name");
            ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(200.0));
            ui.end_row();

            ui.label("Default pitch");
            ui.add(egui::DragValue::new(&mut form.pitch).range(PITCH_RANGE));
            ui.end_row();

            ui.label("Icon");
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!picking_icon, egui::Button::new("Choose…"))
                    .clicked()
                {
                    *action = Some(Action::PickIcon);
                }
                if let Some(icon) = &form.icon {
                    ui.label(file_name(icon));
                    if ui.button("Clear").clicked() {
                        *action = Some(Action::ClearIcon);
                    }
                }
            });
            ui.end_row();
        });
}

/// The form of the file being read or converted, if any.
fn ready_form(state: &mut State) -> Option<&mut Form> {
    match state {
        State::Ready { form, .. } => Some(form),
        _ => None,
    }
}

/// Carries out a click.
fn apply(app: &mut VcApp, action: Action) {
    match action {
        Action::Choose => pick_model(app),
        Action::PickIcon => {
            app.convert_ui.icon_pick_rx = Some(files::pick_file(
                "Choose an icon",
                "Images",
                &["png", "jpg", "jpeg"],
            ));
        }
        Action::ClearIcon => {
            if let Some(form) = ready_form(&mut app.converter.state) {
                form.icon = None;
            }
        }
        Action::Convert => app.converter.start(&app.library),
        Action::Cancel => app.converter.cancel(),
        Action::UseNow(id) => app.select_model(&id),
        Action::ShowModels => app.tab = Tab::Models,
        Action::Export(id) => {
            app.convert_ui.export_pick_rx = Some(files::save_file(
                "Export the ONNX model",
                format!("{id}.onnx"),
                "ONNX models",
                &["onnx"],
            ));
        }
        Action::Another => {
            app.converter.reset();
            pick_model(app);
        }
        Action::OpenLogs => files::open_folder(&app.dirs.logs),
    }
}

/// Opens the Choose file… picker; its answer becomes `convert_file`.
fn pick_model(app: &mut VcApp) {
    app.convert_ui.pick_rx = Some(files::pick_file(
        "Choose a PyTorch model",
        "PyTorch models",
        &["pth", "zip"],
    ));
}

/// The picker's path once it answers; `None` while it is open, when cancelled or when it is gone.
fn take_pick(rx: &mut Option<Receiver<Option<PathBuf>>>) -> Option<PathBuf> {
    let answer = match rx.as_ref()?.try_recv() {
        Err(TryRecvError::Empty) => return None,
        Ok(path) => path,
        Err(TryRecvError::Disconnected) => None,
    };
    *rx = None;
    answer
}

/// Picks up the answers of the file, icon and export dialogs.
fn poll_picks(app: &mut VcApp) {
    if let Some(path) = take_pick(&mut app.convert_ui.pick_rx) {
        app.convert_file = Some(path);
    }
    if let Some(icon) = take_pick(&mut app.convert_ui.icon_pick_rx)
        && let Some(form) = ready_form(&mut app.converter.state)
    {
        form.icon = Some(icon);
    }
    if let Some(dest) = take_pick(&mut app.convert_ui.export_pick_rx)
        && let State::Done { id, .. } = &app.converter.state
        && let Some(entry) = app.library.get(id)
    {
        let src = app.library.model_path(entry);
        app.converter.export(src, dest);
    }
}

/// The summary of an inspected file, as (label, value) rows.
pub fn summary_lines(i: &Inspection) -> Vec<(&'static str, String)> {
    let fp16 = if i.fp16 { " (fp16)" } else { "" };
    let mut lines = vec![
        ("Version", format!("RVC {}", i.version)),
        ("Sample rate", khz(i.sample_rate)),
        ("Pitch (F0)", if i.f0 { "Yes" } else { "No" }.to_string()),
        ("Speakers", i.speakers.to_string()),
        (
            "File",
            format!("{} · {:.1} MB{fp16}", i.pth_name, i.file_size as f64 / 1e6),
        ),
    ];
    if let Some(info) = &i.info {
        lines.push(("Training info", info.clone()));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inspection() -> Inspection {
        Inspection {
            source: PathBuf::from(r"C:\x\Ana v2.pth"),
            pth_name: "Ana v2.pth".into(),
            file_size: 52_400_000,
            fp16: true,
            version: "v2".into(),
            sample_rate: 40000,
            f0: true,
            speakers: 1,
            info: Some("300epoch".into()),
            sha256: "ab".repeat(32),
        }
    }

    fn lines(rows: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        rows.iter().map(|&(k, v)| (k, v.to_string())).collect()
    }

    #[test]
    fn summary_lines_cases() {
        assert_eq!(
            summary_lines(&inspection()),
            lines(&[
                ("Version", "RVC v2"),
                ("Sample rate", "40 kHz"),
                ("Pitch (F0)", "Yes"),
                ("Speakers", "1"),
                ("File", "Ana v2.pth · 52.4 MB (fp16)"),
                ("Training info", "300epoch"),
            ])
        );
        let plain = Inspection {
            pth_name: "bo.pth".into(),
            file_size: 55_049_999,
            fp16: false,
            sample_rate: 48000,
            speakers: 109,
            info: None,
            ..inspection()
        };
        assert_eq!(
            summary_lines(&plain),
            lines(&[
                ("Version", "RVC v2"),
                ("Sample rate", "48 kHz"),
                ("Pitch (F0)", "Yes"),
                ("Speakers", "109"),
                ("File", "bo.pth · 55.0 MB"),
            ])
        );
    }
}
