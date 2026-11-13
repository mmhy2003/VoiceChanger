//! Top-level app state and the tab bar.

use std::any::Any;
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use eframe::egui;
use serde::{Deserialize, Serialize};
use vc_audio::{
    AudioControl, DeviceList, DeviceRef, Endpoint, Processor, Selection, SessionInfo, Shared,
    StreamStatus, db_to_gain,
};
use vc_engine::{Controls, Pipeline, Provider, StreamSettings, StreamStats, Streamer};

use crate::config::{Config, DevicesConfig, PerfConfig, Prefs, SaveTracker, SavedDevice};
use vc_models::{Library, ModelEntry};

use crate::engine::{EngineProcessor, ErrorSlot};
use crate::files;
use crate::import::{DropAction, Importer, Notice, classify_drop};
use crate::model::{self, ModelSlot, provider_label};
use crate::paths::AppDirs;
use crate::status_bar::{self, StatusSnapshot};
use crate::switch::{PendingSwitch, SwitchPlan, pitch_for, plan_switch};
use crate::tabs;
use crate::tabs::models::ModelsUi;
use crate::widgets::MeterState;

/// Window title, including the app version.
pub fn window_title() -> String {
    format!("VoiceChanger {}", env!("CARGO_PKG_VERSION"))
}

/// First line of an error message, trimmed and cut to 120 characters; "unknown error" if empty.
pub fn short_reason(e: Option<&str>) -> String {
    const MAX_CHARS: usize = 120;
    let line = e.and_then(|e| e.lines().next()).unwrap_or_default().trim();
    if line.is_empty() {
        return "unknown error".into();
    }
    match line.char_indices().nth(MAX_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.into(),
    }
}

/// Banner text for a session that stopped with `status`; `None` while it is running.
/// `processor_error` is the voice processor's error message, if any.
pub fn banner_text(
    status: StreamStatus,
    sel: &Selection,
    processor_error: Option<&str>,
) -> Option<String> {
    let device = |which| match which {
        Endpoint::Input => ("input", sel.input.name.as_str()),
        Endpoint::Cable => ("cable output", sel.cable.name.as_str()),
        Endpoint::Monitor => (
            "monitor",
            sel.monitor.as_ref().map_or("", |m| m.name.as_str()),
        ),
    };
    match status {
        StreamStatus::Running => None,
        StreamStatus::DeviceLost { which } => {
            let (role, name) = device(which);
            Some(format!(
                "Audio stopped: the {role} device \"{name}\" was disconnected. \
                 Pick another device in Settings and press Start."
            ))
        }
        StreamStatus::Failed { which } => {
            let (role, name) = device(which);
            Some(format!(
                "Audio stopped: the {role} device \"{name}\" reported an error. \
                 Press Start to try again."
            ))
        }
        StreamStatus::ProcessorFailed => Some(format!(
            "Voice conversion stopped: {}. Details are in the log. Press Start to try again.",
            short_reason(processor_error)
        )),
    }
}

/// The tabs of the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Voice,
    Models,
    Convert,
    Settings,
}

impl Tab {
    /// All tabs, in display order.
    pub const ALL: [Tab; 4] = [Tab::Voice, Tab::Models, Tab::Convert, Tab::Settings];

    /// Label shown in the tab bar.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Voice => "Voice",
            Tab::Models => "Models",
            Tab::Convert => "Convert",
            Tab::Settings => "Settings",
        }
    }
}

/// Execution provider picked in Settings → Performance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderChoice {
    #[default]
    DirectMl,
    Cuda,
    Cpu,
}

/// Settings → Performance: where the model runs and how the stream is cut into blocks.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PerfSettings {
    pub provider: ProviderChoice,
    /// GPU index for DirectML and CUDA.
    pub gpu: i32,
    pub stream: StreamSettings,
}

impl PerfSettings {
    /// The engine provider for this choice and GPU.
    pub fn provider(&self) -> Provider {
        match self.provider {
            ProviderChoice::DirectMl => Provider::DirectMl { device: self.gpu },
            ProviderChoice::Cuda => Provider::Cuda { device: self.gpu },
            ProviderChoice::Cpu => Provider::Cpu,
        }
    }
}

/// Voice tab controls, applied live to the running stream.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceSettings {
    /// Pitch shift in semitones.
    pub pitch: i32,
    pub input_db: f32,
    pub output_db: f32,
    /// Noise gate threshold in dB.
    pub gate_db: f32,
}

impl Default for VoiceSettings {
    fn default() -> Self {
        Self {
            pitch: 0,
            input_db: 0.0,
            output_db: 0.0,
            gate_db: -50.0,
        }
    }
}

/// Device list and the user's choices, kept apart from the UI so it can be tested without hardware.
#[derive(Clone, Debug, PartialEq)]
pub struct AppState {
    pub devices: DeviceList,
    pub selection: Selection,
    /// Monitor volume in dB.
    pub monitor_db: f32,
    /// Messages from the last Refresh, e.g. a selected device that disappeared.
    pub notes: Vec<String>,
    pub perf: PerfSettings,
    pub voice: VoiceSettings,
}

impl AppState {
    /// The saved choices matched against `devices`. A saved device that is gone falls back to its default
    /// with a note; a device that was never saved takes its default silently.
    pub fn from_config(devices: DeviceList, config: &Config) -> Self {
        let saved = &config.devices;
        let device = |d: &Option<SavedDevice>| d.clone().map(DeviceRef::from).unwrap_or_default();
        let mut selection = Selection {
            input: device(&saved.input),
            cable: device(&saved.cable),
            monitor: saved.monitor.clone().map(DeviceRef::from),
            monitor_enabled: saved.monitor_enabled,
        };
        if selection.monitor.is_none() {
            selection.monitor = Selection::defaults(&devices).monitor;
        }
        let notes = selection.reconcile(&devices);
        Self {
            devices,
            selection,
            monitor_db: saved.monitor_db,
            notes,
            perf: config.perf.to_settings(),
            voice: config.voice,
        }
    }

    /// The saved form of this state, with the preferences kept outside it.
    pub fn to_config(&self, prefs: &Prefs) -> Config {
        let device = |r: &DeviceRef| (!r.is_empty()).then(|| SavedDevice::from(r));
        let sel = &self.selection;
        Config {
            devices: DevicesConfig {
                input: device(&sel.input),
                cable: device(&sel.cable),
                monitor: sel.monitor.as_ref().and_then(device),
                monitor_enabled: sel.monitor_enabled,
                monitor_db: self.monitor_db,
            },
            perf: PerfConfig::from_settings(&self.perf),
            voice: self.voice,
            last_model: prefs.last_model.clone(),
            pitch_by_model: prefs.pitch_by_model.clone(),
            hotkey: prefs.hotkey.clone(),
            start_minimised: prefs.start_minimised,
            ..Config::default()
        }
    }

    /// Applies a fresh device list and keeps the reconcile notes.
    pub fn refresh_with(&mut self, devices: DeviceList) {
        self.devices = devices;
        self.notes = self.selection.reconcile(&self.devices);
    }
}

/// The eframe application.
pub struct VcApp {
    pub(crate) tab: Tab,
    pub(crate) state: AppState,
    /// The audio-control thread, which makes every cpal call.
    pub(crate) audio: AudioControl,
    /// The running session's sample rates; `Some` while running.
    pub(crate) session_info: Option<SessionInfo>,
    /// Created once and reused by every session.
    shared: Arc<Shared>,
    /// Why audio stopped or could not start, shown on the Voice tab.
    pub(crate) banner: Option<String>,
    pub(crate) input_meter: MeterState,
    pub(crate) output_meter: MeterState,
    pub(crate) model: ModelSlot<Pipeline>,
    /// The library model whose generator is loading, to swap in when ready.
    pub(crate) pending: Option<PendingSwitch>,
    /// Pitch and gate, read by the streamer once per block.
    controls: Arc<Controls>,
    /// Inference timings written by the streamer.
    pub(crate) stats: Arc<StreamStats>,
    /// Why the voice processor failed, for the banner.
    processor_error: ErrorSlot,
    /// Set by a block, context or crossfade change; the stream restarts after this frame.
    pub(crate) restart_requested: bool,
    /// Set once the dead-audio-thread banner has been shown, so dismissing it sticks.
    audio_dead_shown: bool,
    /// Where the settings, models and logs live.
    dirs: AppDirs,
    /// The saved preferences that live outside `state`.
    pub(crate) prefs: Prefs,
    /// Writes the settings a moment after they change.
    save: SaveTracker,
    /// The imported voice models.
    pub(crate) library: Library,
    /// Inspects and copies imported files in the background.
    pub(crate) importer: Importer,
    /// Messages on the Models tab, oldest first.
    pub(crate) notices: Vec<Notice>,
    /// The PyTorch file handed to the Convert tab.
    pub(crate) convert_file: Option<PathBuf>,
    /// The open Import… picker's answer.
    pub(crate) import_pick_rx: Option<Receiver<Option<PathBuf>>>,
    /// Renaming, delete confirmation and the icon picker on the Models tab.
    pub(crate) models_ui: ModelsUi,
}

/// What `main` hands the app: its folders, the loaded settings and any notes from loading them.
pub struct Startup {
    pub dirs: AppDirs,
    pub config: Config,
    pub notes: Vec<String>,
}

impl VcApp {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        // egui 0.32's ViewportBuilder cannot open minimised; the command runs after the first frame.
        if startup.config.start_minimised {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        // Model icons are read from file:// URIs.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let shared = Arc::new(Shared::new());
        let audio = AudioControl::spawn(shared.clone());
        let state = AppState::from_config(audio.list().unwrap_or_default(), &startup.config);
        let (library, library_note) = Library::open(startup.dirs.models.clone());
        let notes: Vec<&str> = startup
            .notes
            .iter()
            .chain(&library_note)
            .chain(&state.notes)
            .map(String::as_str)
            .collect();
        let banner = (!notes.is_empty()).then(|| notes.join(" "));
        let mut app = Self {
            tab: Tab::default(),
            state,
            audio,
            session_info: None,
            shared,
            banner,
            input_meter: MeterState::default(),
            output_meter: MeterState::default(),
            model: ModelSlot::default(),
            pending: None,
            controls: Arc::new(Controls::new()),
            stats: Arc::new(StreamStats::new()),
            processor_error: ErrorSlot::default(),
            restart_requested: false,
            audio_dead_shown: false,
            dirs: startup.dirs,
            prefs: Prefs::from_config(&startup.config),
            save: SaveTracker::new(startup.config),
            library,
            importer: Importer::new(Arc::new(|p| {
                vc_engine::inspect_model(p).map_err(|e| e.to_string())
            })),
            notices: Vec::new(),
            convert_file: None,
            import_pick_rx: None,
            models_ui: ModelsUi::default(),
        };
        // Load the last model; the app never starts streaming by itself.
        if let Some(id) = app.prefs.last_model.clone()
            && app.library.get(&id).is_some()
        {
            app.select_model(&id);
        }
        app
    }

    /// Opens the Import… picker; its answer is polled in `update`.
    pub(crate) fn pick_import(&mut self) {
        self.import_pick_rx = Some(files::pick_file(
            "Import a voice model",
            "Voice models",
            &["onnx", "pth", "zip"],
        ));
    }

    /// Imports, offers to convert or rejects a dropped or picked file.
    pub(crate) fn receive_file(&mut self, path: PathBuf) {
        match classify_drop(&path) {
            DropAction::Import(path) => self.importer.enqueue(path),
            DropAction::OfferConvert(path) => self.notices.push(Notice::OfferConvert(path)),
            DropAction::Reject(msg) => self.notices.push(Notice::Error(msg)),
        }
    }

    /// Takes files dropped on the window; any drop opens the Models tab.
    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if dropped.is_empty() {
            return;
        }
        self.tab = Tab::Models;
        for path in dropped.into_iter().filter_map(|f| f.path) {
            self.receive_file(path);
        }
    }

    /// Picks up the Import… picker's answer and the importer's results.
    fn poll_import(&mut self) {
        if let Some(rx) = &self.import_pick_rx {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.import_pick_rx = None;
                    self.receive_file(path);
                }
                Ok(None) | Err(TryRecvError::Disconnected) => self.import_pick_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        for result in self.importer.poll(&mut self.library) {
            self.notices.push(match result {
                Ok(id) => {
                    let name = self.library.get(&id).map_or(id.as_str(), |e| &e.name);
                    Notice::Info(format!("Imported \"{name}\"."))
                }
                Err(msg) => Notice::Error(msg),
            });
        }
    }

    /// Writes the settings; a failure is logged and not retried until they change again.
    fn write_config(&mut self, cfg: Config) {
        let path = self.dirs.settings_file();
        if let Err(e) = cfg.save(&path) {
            tracing::warn!("cannot save {}: {e}", path.display());
        }
        self.save.mark_saved(cfg);
    }

    /// Makes the library model `id` the active one: loads it, or loads its generator to swap in
    /// (also while running). The newest request wins.
    pub(crate) fn select_model(&mut self, id: &str) {
        let Some(entry) = self.library.get(id).cloned() else {
            return;
        };
        if !self.library.file_exists(&entry) {
            self.banner = Some(format!("The model file for \"{}\" is missing.", entry.name));
            return;
        }
        if self.prefs.last_model.as_deref() == Some(id) && self.model.info().is_some() {
            // Already active.
            self.pending = None;
            return;
        }
        let (path, perf) = (self.library.model_path(&entry), self.state.perf);
        match plan_switch(self.model.info().is_some(), self.is_running()) {
            SwitchPlan::LoadAll => {
                self.pending = None;
                self.model = match model::spawn_load(
                    entry.name.clone(),
                    path,
                    Some(entry.sample_rate),
                    perf.provider(),
                    perf.stream,
                ) {
                    Ok(rx) => ModelSlot::loading(entry.name.clone(), rx),
                    Err(e) => ModelSlot::Failed(e.to_string()),
                };
                self.activate(&entry);
            }
            SwitchPlan::Swap | SwitchPlan::SwapWithRestart => {
                match model::spawn_generator_load(
                    entry.name.clone(),
                    path,
                    entry.sample_rate,
                    perf.provider(),
                    perf.stream,
                ) {
                    // Replacing the pending switch drops the older loader's receiver.
                    Ok(rx) => {
                        self.pending = Some(PendingSwitch {
                            id: entry.id.clone(),
                            name: entry.name.clone(),
                            rx,
                        });
                    }
                    Err(e) => {
                        self.banner = Some(format!("Could not switch to \"{}\": {e}", entry.name));
                    }
                }
            }
        }
    }

    /// Remembers `entry` as the last model and sets its pitch.
    fn activate(&mut self, entry: &ModelEntry) {
        self.prefs.last_model = Some(entry.id.clone());
        self.state.voice.pitch =
            pitch_for(&entry.id, &self.prefs.pitch_by_model, entry.default_pitch);
    }

    /// Swaps in a pending generator once it is loaded, with a quick stop and start while running.
    /// A failed load keeps the old voice.
    fn poll_switch(&mut self) {
        let Some(pending) = &self.pending else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("the model loader stopped unexpectedly".to_string())
            }
            Ok(result) => result,
        };
        let Some(pending) = self.pending.take() else {
            return;
        };
        let (generator, info) = match result {
            Ok(loaded) => loaded,
            Err(msg) => {
                self.banner = Some(format!("Could not switch to \"{}\": {msg}", pending.name));
                return;
            }
        };
        let running = self.is_running();
        if running {
            self.stop();
        }
        if let Some((p, slot_info)) = self.model.ready_mut() {
            drop(p.swap_generator(generator));
            *slot_info = info;
            if let Some(entry) = self.library.get(&pending.id).cloned() {
                self.activate(&entry);
            }
        } else {
            self.banner = Some("Could not switch: the model was unloaded.".to_string());
        }
        if running {
            self.start();
        }
    }

    /// Saves the pitch slider's value for the active model.
    pub(crate) fn pitch_changed(&mut self) {
        if let Some(id) = &self.prefs.last_model {
            self.prefs
                .pitch_by_model
                .insert(id.clone(), self.state.voice.pitch);
        }
    }

    /// Picks up the loader's result.
    fn poll_model(&mut self) {
        if let Some(Err(msg)) = self.model.poll() {
            self.banner = Some(format!("Could not load the model: {msg}"));
        }
    }

    /// Re-enumerates the audio devices.
    pub(crate) fn refresh_devices(&mut self) {
        self.state
            .refresh_with(self.audio.list().unwrap_or_default());
    }

    pub(crate) fn is_running(&self) -> bool {
        self.session_info.is_some()
    }

    /// Streams the loaded model with the current selection; on failure, the banner says why.
    pub(crate) fn start(&mut self) {
        self.banner = None;
        let Some(pipeline) = self.model.take() else {
            return;
        };
        *self
            .processor_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let streamer = Streamer::new(
            pipeline,
            self.state.perf.stream,
            self.controls.clone(),
            self.stats.clone(),
        );
        let processor = EngineProcessor::new(streamer, self.processor_error.clone());
        match self.audio.start(&self.state.selection, Box::new(processor)) {
            Ok(info) => self.session_info = Some(info),
            Err((e, back)) => {
                self.recover(back);
                let text = format!("Could not start audio: {e}");
                self.banner = Some(match self.banner.take() {
                    Some(lost) => format!("{text}. {lost}"),
                    None => text,
                });
            }
        }
    }

    /// Stops audio, takes the model back and clears the meters.
    pub(crate) fn stop(&mut self) {
        if self.session_info.take().is_some() {
            let back = self.audio.stop();
            self.recover(back);
        }
        self.input_meter.reset();
        self.output_meter.reset();
    }

    /// Returns the pipeline inside a stopped session's processor to the model slot.
    fn recover(&mut self, p: Option<Box<dyn Processor>>) {
        let ep = p.and_then(|p| {
            (p as Box<dyn Any>)
                .downcast::<EngineProcessor<Pipeline>>()
                .ok()
        });
        match ep {
            Some(ep) => self.model.give_back(ep.into_inner()),
            None => {
                self.model.lost();
                self.banner = Some(MODEL_LOST.to_string());
            }
        }
    }

    /// Stops a session whose device was lost or failed, and says why in the banner.
    fn poll_status(&mut self) {
        if !self.is_running() {
            return;
        }
        let error = self
            .processor_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(text) = banner_text(
            self.shared.status(),
            &self.state.selection,
            error.as_deref(),
        ) {
            self.stop();
            self.banner = Some(match self.banner.take() {
                Some(lost) => format!("{text} {lost}"),
                None => text,
            });
            self.refresh_devices();
        }
    }

    /// Applies the Voice tab controls to the stream; they take effect on the next block.
    fn push_voice(&self) {
        let voice = self.state.voice;
        self.controls.set_pitch(voice.pitch as f32);
        self.controls.set_gate_db(voice.gate_db);
        self.shared.set_input_gain(db_to_gain(voice.input_db));
        self.shared.set_output_gain(db_to_gain(voice.output_db));
    }

    /// What the status bar shows; `None` unless running.
    fn status_snapshot(&self) -> Option<StatusSnapshot> {
        if !self.is_running() {
            return None;
        }
        Some(StatusSnapshot {
            latency: Duration::from_micros(self.shared.latency_us.load(Relaxed)),
            inference: self.stats.inference(),
            block: self.stats.block(),
            ratio: self.stats.ratio(),
            provider: self
                .model
                .info()
                .map(|info| provider_label(info.requested, info.active))
                .unwrap_or_default(),
            late_blocks: self.stats.late_blocks(),
            underruns: self.shared.underruns.load(Relaxed),
        })
    }
}

/// Banner and Start hint when the audio-control thread has died.
pub(crate) const AUDIO_DEAD: &str = "The audio thread stopped unexpectedly. Restart VoiceChanger.";

/// Banner text when a stopped session did not hand the model back.
const MODEL_LOST: &str = "The model was unloaded; choose it again from the Model list.";

impl eframe::App for VcApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.audio_dead_shown && !self.audio.is_alive() {
            self.audio_dead_shown = true;
            self.banner = Some(AUDIO_DEAD.to_string());
        }
        self.poll_status();
        self.poll_model();
        self.poll_switch();
        self.handle_drops(ctx);
        self.poll_import();
        self.shared
            .set_monitor_gain(db_to_gain(self.state.monitor_db));
        self.push_voice();
        if self.is_running() {
            let dt = ctx.input(|i| i.stable_dt);
            self.input_meter
                .update(self.shared.input_meter.load().peak, dt);
            self.output_meter
                .update(self.shared.output_meter.load().peak, dt);
        }

        // A volume change is not a selection change, so it doesn't restart the session.
        let before = self.state.selection.clone();
        let provider_before = self.state.perf.provider();
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                for t in Tab::ALL {
                    ui.selectable_value(&mut self.tab, t, t.label());
                }
            });
        });
        if let Some(snapshot) = self.status_snapshot() {
            egui::TopBottomPanel::bottom("status").show(ctx, |ui| status_bar::show(ui, &snapshot));
        }
        egui::CentralPanel::default().show(ctx, |ui| match self.tab {
            Tab::Voice => tabs::voice::show(ui, self),
            Tab::Models => tabs::models::show(ui, self),
            Tab::Convert => tabs::convert::show(ui, self),
            Tab::Settings => tabs::settings::show(ui, self),
        });
        // Block, context and crossfade restart the stream on release; the model is kept.
        if self.is_running() && (self.state.selection != before || self.restart_requested) {
            self.stop();
            self.start();
        }
        self.restart_requested = false;
        // A new provider or GPU needs a fresh session, so the model is reloaded.
        if self.state.perf.provider() != provider_before
            && !self.is_running()
            && let Some(id) = self.prefs.last_model.clone()
        {
            self.pending = None;
            self.model = ModelSlot::Empty;
            self.select_model(&id);
        }

        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else if self.model.is_loading()
            || self.pending.is_some()
            || self.importer.is_busy()
            || self.import_pick_rx.is_some()
            || self.models_ui.icon_pick.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let cfg = self.state.to_config(&self.prefs);
        if self.save.poll(&cfg, Instant::now()) {
            self.write_config(cfg);
        }
        if self.save.is_pending() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let cfg = self.state.to_config(&self.prefs);
        if self.save.needs_save(&cfg) {
            self.write_config(cfg);
        }
        self.stop();
        self.audio.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vc_audio::{DEFAULT_MONITOR_DB, DeviceInfo};

    #[test]
    fn window_title_has_version() {
        assert_eq!(
            window_title(),
            format!("VoiceChanger {}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn tabs_in_order() {
        assert_eq!(
            Tab::ALL.map(Tab::label),
            ["Voice", "Models", "Convert", "Settings"]
        );
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn vc_app_state_defaults() {
        let list = DeviceList::from_names(
            names(&["USB Mic", "Microphone (Realtek)"]),
            names(&["Headphones", "CABLE Input (VB-Audio Virtual Cable)"]),
            Some("USB Mic".into()),
            Some("Headphones".into()),
        );
        let mut state = AppState::from_config(list.clone(), &Config::default());
        assert_eq!(state.devices, list);
        assert_eq!(state.selection, Selection::defaults(&list));
        assert_eq!(state.monitor_db, DEFAULT_MONITOR_DB);
        assert!(state.notes.is_empty());

        let unplugged = DeviceList::from_names(
            names(&["Microphone (Realtek)"]),
            names(&["Headphones", "CABLE Input (VB-Audio Virtual Cable)"]),
            Some("Microphone (Realtek)".into()),
            Some("Headphones".into()),
        );
        state.refresh_with(unplugged.clone());
        assert_eq!(state.devices, unplugged);
        assert_eq!(state.selection.input.name, "Microphone (Realtek)");
        assert_eq!(state.notes.len(), 1);
    }

    fn device(name: &str) -> DeviceRef {
        DeviceRef {
            id: format!("{{id}}.{name}"),
            name: name.into(),
        }
    }

    fn selection() -> Selection {
        Selection {
            input: device("USB Mic"),
            cable: device("CABLE Input (VB-Audio Virtual Cable)"),
            monitor: Some(device("Headphones")),
            monitor_enabled: true,
        }
    }

    #[test]
    fn banner_text_device_lost_input() {
        assert_eq!(
            banner_text(
                StreamStatus::DeviceLost {
                    which: Endpoint::Input
                },
                &selection(),
                None
            )
            .as_deref(),
            Some(
                "Audio stopped: the input device \"USB Mic\" was disconnected. \
                 Pick another device in Settings and press Start."
            )
        );
    }

    #[test]
    fn banner_text_failed_cable() {
        assert_eq!(
            banner_text(
                StreamStatus::Failed {
                    which: Endpoint::Cable
                },
                &selection(),
                None
            )
            .as_deref(),
            Some(
                "Audio stopped: the cable output device \"CABLE Input (VB-Audio Virtual Cable)\" \
                 reported an error. Press Start to try again."
            )
        );
    }

    #[test]
    fn banner_text_running_none() {
        assert_eq!(banner_text(StreamStatus::Running, &selection(), None), None);
    }

    #[test]
    fn banner_text_processor_failed() {
        assert_eq!(
            banner_text(
                StreamStatus::ProcessorFailed,
                &selection(),
                Some("inference failed: x\nmore")
            )
            .as_deref(),
            Some(
                "Voice conversion stopped: inference failed: x. Details are in the log. \
                 Press Start to try again."
            )
        );
    }

    #[test]
    fn perf_provider_mapping() {
        let mut perf = PerfSettings::default();
        assert_eq!(perf.provider, ProviderChoice::DirectMl);
        assert_eq!(perf.gpu, 0);
        assert_eq!(perf.stream, StreamSettings::default());
        assert_eq!(perf.provider(), Provider::DirectMl { device: 0 });
        perf.gpu = 2;
        assert_eq!(perf.provider(), Provider::DirectMl { device: 2 });
        perf.provider = ProviderChoice::Cuda;
        assert_eq!(perf.provider(), Provider::Cuda { device: 2 });
        perf.provider = ProviderChoice::Cpu;
        assert_eq!(perf.provider(), Provider::Cpu);
    }

    #[test]
    fn voice_defaults() {
        let voice = VoiceSettings::default();
        assert_eq!(voice.pitch, 0);
        assert_eq!(voice.input_db, 0.0);
        assert_eq!(voice.output_db, 0.0);
        assert_eq!(voice.gate_db, -50.0);
        let state = AppState::from_config(
            DeviceList::from_names(vec![], vec![], None, None),
            &Config::default(),
        );
        assert_eq!(state.voice, voice);
    }

    fn info(id: &str, name: &str) -> DeviceInfo {
        DeviceInfo {
            id: id.into(),
            name: name.into(),
        }
    }

    fn saved(id: &str, name: &str) -> Option<SavedDevice> {
        Some(SavedDevice {
            id: id.into(),
            name: name.into(),
        })
    }

    /// One mic, a cable and headphones, each with its own endpoint ID.
    fn id_list() -> DeviceList {
        DeviceList::new(
            vec![
                info("{mic}", "USB Mic"),
                info("{rtk}", "Microphone (Realtek)"),
            ],
            vec![
                info("{hp}", "Headphones"),
                info("{cable}", "CABLE Input (VB-Audio Virtual Cable)"),
            ],
            Some(DeviceRef {
                id: "{rtk}".into(),
                name: "Microphone (Realtek)".into(),
            }),
            Some(DeviceRef {
                id: "{hp}".into(),
                name: "Headphones".into(),
            }),
        )
    }

    fn saved_config() -> Config {
        Config {
            devices: DevicesConfig {
                input: saved("{mic}", "USB Mic"),
                cable: saved("{cable}", "CABLE Input (VB-Audio Virtual Cable)"),
                monitor: saved("{hp}", "Headphones"),
                monitor_enabled: true,
                monitor_db: -7.5,
            },
            perf: PerfConfig {
                provider: ProviderChoice::Cpu,
                gpu: 1,
                block_ms: 300,
                context_ms: 400,
                crossfade_ms: 60,
            },
            voice: VoiceSettings {
                pitch: -4,
                input_db: 3.0,
                output_db: -2.0,
                gate_db: -42.0,
            },
            last_model: Some("ana".into()),
            pitch_by_model: [("ana".to_string(), -4)].into(),
            hotkey: Some("Ctrl+Alt+V".into()),
            start_minimised: true,
            ..Config::default()
        }
    }

    #[test]
    fn state_from_config_restores() {
        let mut config = saved_config();
        // Same endpoint, renamed since the last launch.
        config.devices.input = saved("{mic}", "Old Mic Name");
        let state = AppState::from_config(id_list(), &config);
        assert!(state.notes.is_empty(), "{:?}", state.notes);
        assert_eq!(
            state.selection,
            Selection {
                input: DeviceRef {
                    id: "{mic}".into(),
                    name: "USB Mic".into(),
                },
                cable: DeviceRef {
                    id: "{cable}".into(),
                    name: "CABLE Input (VB-Audio Virtual Cable)".into(),
                },
                monitor: Some(DeviceRef {
                    id: "{hp}".into(),
                    name: "Headphones".into(),
                }),
                monitor_enabled: true,
            }
        );
        assert_eq!(state.monitor_db, -7.5);
        assert_eq!(state.perf, config.perf.to_settings());
        assert_eq!(state.voice, config.voice);
        assert_eq!(state.devices, id_list());
    }

    #[test]
    fn state_from_config_missing_device_notes() {
        let mut config = saved_config();
        config.devices.input = saved("{gone}", "Unplugged Mic");
        let state = AppState::from_config(id_list(), &config);
        assert_eq!(
            state.notes,
            [
                "Input \"Unplugged Mic\" is no longer available; switched to \
                 \"Microphone (Realtek)\"."
            ]
        );
        assert_eq!(state.selection.input.id, "{rtk}");
        assert_eq!(state.selection.cable.id, "{cable}");
    }

    #[test]
    fn state_from_config_without_saved_devices_takes_defaults() {
        let state = AppState::from_config(id_list(), &Config::default());
        assert!(state.notes.is_empty(), "{:?}", state.notes);
        assert_eq!(state.selection, Selection::defaults(&id_list()));
    }

    #[test]
    fn to_config_round_trip() {
        let config = saved_config();
        let state = AppState::from_config(id_list(), &config);
        assert_eq!(state.to_config(&Prefs::from_config(&config)), config);
    }

    #[test]
    fn short_reason_truncates_and_defaults() {
        assert_eq!(short_reason(Some("  boom  \nsecond line")), "boom");
        assert_eq!(short_reason(None), "unknown error");
        assert_eq!(short_reason(Some("")), "unknown error");
        assert_eq!(short_reason(Some("   \nnext")), "unknown error");
        let long = "é".repeat(130);
        assert_eq!(short_reason(Some(&long)), format!("{}…", "é".repeat(120)));
        let exact = "a".repeat(120);
        assert_eq!(short_reason(Some(&exact)), exact);
    }
}
