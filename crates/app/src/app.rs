//! Top-level app state and the tab bar.

use std::any::Any;
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use eframe::egui;
use vc_audio::{
    AudioSession, DEFAULT_MONITOR_DB, DeviceList, Endpoint, Processor, Selection, Shared,
    StreamStatus, db_to_gain, list_devices,
};
use vc_engine::{Controls, Pipeline, Provider, StreamSettings, StreamStats, Streamer};

use crate::engine::{EngineProcessor, ErrorSlot};
use crate::model::{self, ModelSlot, provider_label};
use crate::status_bar::{self, StatusSnapshot};
use crate::tabs;
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
        Endpoint::Input => ("input", sel.input.as_str()),
        Endpoint::Cable => ("cable output", sel.cable.as_str()),
        Endpoint::Monitor => ("monitor", sel.monitor.as_deref().unwrap_or_default()),
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// Default selection for `devices`, default monitor volume, no notes.
    pub fn new(devices: DeviceList) -> Self {
        Self {
            selection: Selection::defaults(&devices),
            devices,
            monitor_db: DEFAULT_MONITOR_DB,
            notes: Vec::new(),
            perf: PerfSettings::default(),
            voice: VoiceSettings::default(),
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
    pub(crate) session: Option<AudioSession>,
    /// Created once and reused by every session.
    shared: Arc<Shared>,
    /// Why audio stopped or could not start, shown on the Voice tab.
    pub(crate) banner: Option<String>,
    pub(crate) input_meter: MeterState,
    pub(crate) output_meter: MeterState,
    pub(crate) model: ModelSlot<Pipeline>,
    /// The last model file chosen, reloaded when the provider changes.
    pub(crate) model_path: Option<PathBuf>,
    /// The open file picker's answer.
    pub(crate) pick_rx: Option<Receiver<Option<PathBuf>>>,
    /// Pitch and gate, read by the streamer once per block.
    controls: Arc<Controls>,
    /// Inference timings written by the streamer.
    pub(crate) stats: Arc<StreamStats>,
    /// Why the voice processor failed, for the banner.
    processor_error: ErrorSlot,
    /// Set by a block, context or crossfade change; the stream restarts after this frame.
    pub(crate) restart_requested: bool,
}

impl VcApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            tab: Tab::default(),
            state: AppState::new(list_devices()),
            session: None,
            shared: Arc::new(Shared::new()),
            banner: None,
            input_meter: MeterState::default(),
            output_meter: MeterState::default(),
            model: ModelSlot::default(),
            model_path: None,
            pick_rx: None,
            controls: Arc::new(Controls::new()),
            stats: Arc::new(StreamStats::new()),
            processor_error: ErrorSlot::default(),
            restart_requested: false,
        }
    }

    /// Opens the model file picker; its answer is polled in `update`.
    pub(crate) fn pick_model(&mut self) {
        self.pick_rx = Some(model::pick_model());
    }

    /// Starts loading the model at `path` with the current Performance settings.
    pub(crate) fn load_model(&mut self, path: PathBuf) {
        let name = model::model_name(&path);
        let perf = self.state.perf;
        self.model = match model::spawn_load(path.clone(), perf.provider(), perf.stream) {
            Ok(rx) => ModelSlot::loading(name, rx),
            Err(e) => ModelSlot::Failed(e.to_string()),
        };
        self.model_path = Some(path);
    }

    /// Picks up the file picker's answer and the loader's result.
    fn poll_model(&mut self) {
        if let Some(rx) = &self.pick_rx {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.pick_rx = None;
                    self.load_model(path);
                }
                Ok(None) | Err(TryRecvError::Disconnected) => self.pick_rx = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(Err(msg)) = self.model.poll() {
            self.banner = Some(format!("Could not load the model: {msg}"));
        }
    }

    /// Re-enumerates the audio devices.
    pub(crate) fn refresh_devices(&mut self) {
        self.state.refresh_with(list_devices());
    }

    pub(crate) fn is_running(&self) -> bool {
        self.session.is_some()
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
        match AudioSession::start(
            &self.state.selection,
            self.shared.clone(),
            Box::new(processor),
        ) {
            Ok(session) => self.session = Some(session),
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
        if let Some(s) = self.session.take() {
            let back = s.stop();
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

/// Banner text when a stopped session did not hand the model back.
const MODEL_LOST: &str = "The model was unloaded; choose it again with Model…";

impl eframe::App for VcApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_status();
        self.poll_model();
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
            Tab::Models => tabs::models::show(ui),
            Tab::Convert => tabs::convert::show(ui),
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
            && let Some(path) = &self.model_path
        {
            self.load_model(path.clone());
        }

        if self.is_running() {
            ctx.request_repaint_after(Duration::from_millis(33));
        } else if self.model.is_loading() || self.pick_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut state = AppState::new(list.clone());
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
        assert_eq!(state.selection.input, "Microphone (Realtek)");
        assert_eq!(state.notes.len(), 1);
    }

    fn selection() -> Selection {
        Selection {
            input: "USB Mic".into(),
            cable: "CABLE Input (VB-Audio Virtual Cable)".into(),
            monitor: Some("Headphones".into()),
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
        let state = AppState::new(DeviceList::from_names(vec![], vec![], None, None));
        assert_eq!(state.voice, voice);
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
