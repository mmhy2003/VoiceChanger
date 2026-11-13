//! The voice model: picking an `.onnx` file and loading it on a background thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use vc_engine::{Pipeline, PipelineConfig, Provider, StreamSettings};

use crate::tabs::voice::khz;

/// What the Voice tab shows about a loaded model.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub name: String,
    pub sample_rate: u32,
    pub requested: Provider,
    pub active: Provider,
}

/// What the loader thread sends back: the model and its info, or why it failed.
pub type LoadResult<P> = Result<(P, ModelInfo), String>;

/// The app's one model, from loading to a running session and back.
#[derive(Default)]
pub enum ModelSlot<P> {
    #[default]
    Empty,
    Loading {
        name: String,
        rx: Receiver<LoadResult<P>>,
    },
    Ready(P, ModelInfo),
    /// A running session holds the model.
    InSession(ModelInfo),
    Failed(String),
}

impl<P> ModelSlot<P> {
    pub fn loading(name: String, rx: Receiver<LoadResult<P>>) -> Self {
        Self::Loading { name, rx }
    }

    /// While loading, picks up the loader's result: `Some(Ok(()))` when the model is ready,
    /// `Some(Err(msg))` when it failed. `None` while still loading or when not loading.
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        let Self::Loading { rx, .. } = self else {
            return None;
        };
        let (next, result) = match rx.try_recv() {
            Err(TryRecvError::Empty) => return None,
            Ok(Ok((p, info))) => (Self::Ready(p, info), Ok(())),
            Ok(Err(msg)) => (Self::Failed(msg.clone()), Err(msg)),
            Err(TryRecvError::Disconnected) => {
                let msg = "the model loader stopped unexpectedly".to_string();
                (Self::Failed(msg.clone()), Err(msg))
            }
        };
        *self = next;
        Some(result)
    }

    /// Hands the ready model to a session; the slot keeps its info.
    pub fn take(&mut self) -> Option<P> {
        match std::mem::take(self) {
            Self::Ready(p, info) => {
                *self = Self::InSession(info);
                Some(p)
            }
            other => {
                *self = other;
                None
            }
        }
    }

    /// Returns the model from a stopped session; dropped unless the slot is in a session.
    pub fn give_back(&mut self, p: P) {
        if let Self::InSession(info) = self {
            *self = Self::Ready(p, info.clone());
        }
    }

    /// The session did not return the model.
    pub fn lost(&mut self) {
        if matches!(self, Self::InSession(_)) {
            *self = Self::Empty;
        }
    }

    #[allow(dead_code, reason = "the status bar reads it from step 12 on")]
    pub fn info(&self) -> Option<&ModelInfo> {
        match self {
            Self::Ready(_, info) | Self::InSession(info) => Some(info),
            _ => None,
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(..))
    }

    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    /// One line for the Voice tab, e.g. "voice · 40 kHz · DirectML (GPU 0)".
    pub fn label(&self) -> String {
        match self {
            Self::Empty => "No model".into(),
            Self::Loading { name, .. } => format!("Loading {name}…"),
            Self::Ready(_, info) | Self::InSession(info) => format!(
                "{} · {} · {}",
                info.name,
                khz(info.sample_rate),
                provider_label(info.requested, info.active)
            ),
            Self::Failed(msg) => format!("Failed to load: {msg}"),
        }
    }
}

/// "CPU (fallback)" when the requested GPU provider fell back to the CPU, else the active provider.
pub fn provider_label(requested: Provider, active: Provider) -> String {
    if active == Provider::Cpu && requested != Provider::Cpu {
        "CPU (fallback)".into()
    } else {
        active.to_string()
    }
}

/// Loads and warms up the model at `path` on a "vc-model-loader" thread.
pub fn spawn_load(
    path: PathBuf,
    provider: Provider,
    settings: StreamSettings,
) -> std::io::Result<Receiver<LoadResult<Pipeline>>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("vc-model-loader".into())
        .spawn(move || {
            let name = model_name(&path);
            let cfg = PipelineConfig {
                model: path,
                sr_override: None,
                provider,
                // The symmetric warm-up only needs the window's shape, not the stream's context.
                warmup_len: settings.window_16k(),
                warmup_context: 0,
            };
            let result = Pipeline::new(&cfg)
                .map(|p| {
                    let info = ModelInfo {
                        name,
                        sample_rate: p.sample_rate(),
                        requested: provider,
                        active: p.active_provider(),
                    };
                    (p, info)
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(result);
        })?;
    Ok(rx)
}

/// File stem of a model path, used as its display name.
pub fn model_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Opens the native file picker on a "vc-file-dialog" thread; sends `None` when cancelled.
/// rfd initialises COM, which must stay off the UI thread (see `main.rs`).
/// A spawn failure yields a closed channel.
pub fn pick_model() -> Receiver<Option<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("vc-file-dialog".into())
        .spawn(move || {
            let path = rfd::FileDialog::new()
                .set_title("Choose an RVC v2 ONNX model")
                .add_filter("RVC v2 ONNX model", &["onnx"])
                .pick_file();
            let _ = tx.send(path);
        });
    rx
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn info() -> ModelInfo {
        ModelInfo {
            name: "voice".into(),
            sample_rate: 40000,
            requested: Provider::DirectMl { device: 0 },
            active: Provider::DirectMl { device: 0 },
        }
    }

    fn loading() -> (ModelSlot<u32>, mpsc::Sender<LoadResult<u32>>) {
        let (tx, rx) = mpsc::channel();
        (ModelSlot::loading("voice".into(), rx), tx)
    }

    #[test]
    fn slot_loading_to_ready() {
        let (mut slot, tx) = loading();
        assert!(slot.is_loading());
        assert_eq!(slot.poll(), None);
        assert!(slot.is_loading());
        tx.send(Ok((7, info()))).unwrap();
        assert_eq!(slot.poll(), Some(Ok(())));
        assert!(slot.is_ready());
        assert!(!slot.is_loading());
        assert_eq!(slot.info(), Some(&info()));
        assert_eq!(slot.poll(), None);
    }

    #[test]
    fn slot_loading_to_failed() {
        let (mut slot, tx) = loading();
        tx.send(Err("bad".into())).unwrap();
        assert_eq!(slot.poll(), Some(Err("bad".into())));
        assert!(matches!(&slot, ModelSlot::Failed(m) if m == "bad"));
        assert!(!slot.is_ready());
        assert_eq!(slot.info(), None);
    }

    #[test]
    fn slot_loader_disconnected_fails() {
        let (mut slot, tx) = loading();
        drop(tx);
        let msg = "the model loader stopped unexpectedly".to_string();
        assert_eq!(slot.poll(), Some(Err(msg.clone())));
        assert!(matches!(&slot, ModelSlot::Failed(m) if *m == msg));
    }

    #[test]
    fn empty_slot_polls_none() {
        let mut slot = ModelSlot::<u32>::default();
        assert!(matches!(slot, ModelSlot::Empty));
        assert_eq!(slot.poll(), None);
    }

    #[test]
    fn take_and_give_back() {
        let mut slot = ModelSlot::Ready(7u32, info());
        assert_eq!(slot.take(), Some(7));
        assert!(matches!(&slot, ModelSlot::InSession(i) if *i == info()));
        assert!(!slot.is_ready());
        assert_eq!(slot.info(), Some(&info()));
        assert_eq!(slot.take(), None);
        slot.give_back(7);
        assert!(matches!(slot, ModelSlot::Ready(7, _)));
        assert!(slot.is_ready());
    }

    #[test]
    fn give_back_outside_session_is_dropped() {
        let mut slot = ModelSlot::<u32>::Empty;
        slot.give_back(7);
        assert!(matches!(slot, ModelSlot::Empty));
    }

    #[test]
    fn lost_goes_empty() {
        let mut slot = ModelSlot::Ready(7u32, info());
        slot.take();
        slot.lost();
        assert!(matches!(slot, ModelSlot::Empty));
        assert_eq!(slot.label(), "No model");
    }

    #[test]
    fn labels() {
        assert_eq!(ModelSlot::<u32>::Empty.label(), "No model");
        assert_eq!(loading().0.label(), "Loading voice…");
        assert_eq!(
            ModelSlot::Ready(7u32, info()).label(),
            "voice · 40 kHz · DirectML (GPU 0)"
        );
        assert_eq!(
            ModelSlot::<u32>::InSession(info()).label(),
            "voice · 40 kHz · DirectML (GPU 0)"
        );
        assert_eq!(
            ModelSlot::<u32>::Failed("bad".into()).label(),
            "Failed to load: bad"
        );
    }

    #[test]
    fn model_name_is_file_stem() {
        assert_eq!(
            model_name(std::path::Path::new(r"C:\models\Ana v2.onnx")),
            "Ana v2"
        );
    }

    #[test]
    fn provider_label_fallback() {
        let dml = Provider::DirectMl { device: 1 };
        assert_eq!(provider_label(dml, Provider::Cpu), "CPU (fallback)");
        assert_eq!(provider_label(dml, dml), "DirectML (GPU 1)");
        assert_eq!(provider_label(Provider::Cpu, Provider::Cpu), "CPU");
        assert_eq!(
            provider_label(Provider::Cuda { device: 0 }, Provider::Cuda { device: 0 }),
            "CUDA (GPU 0)"
        );
    }
}
