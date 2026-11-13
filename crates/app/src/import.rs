//! Importing voice models: what to do with a dropped file, and a background queue that inspects
//! and copies `.onnx` files into the library one at a time.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use vc_engine::ModelMeta;
use vc_models::{Library, StagedModel, copy_model, discard_staged};

/// Validates a model and reads its metadata; the error is shown to the user.
pub type InspectFn = Arc<dyn Fn(&Path) -> Result<ModelMeta, String> + Send + Sync>;

/// What a dropped or picked file leads to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    Import(PathBuf),
    OfferConvert(PathBuf),
    Reject(String),
}

/// `.onnx` (any case) is imported; `.pth` and `.zip` are offered to the Convert tab; anything else is rejected.
pub fn classify_drop(path: &Path) -> DropAction {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext.as_deref() {
        Some("onnx") => DropAction::Import(path.to_path_buf()),
        Some("pth" | "zip") => DropAction::OfferConvert(path.to_path_buf()),
        _ => DropAction::Reject(format!(
            "Could not import \"{}\": only RVC v2 .onnx models can be imported here.",
            file_name(path)
        )),
    }
}

/// Where a dropped file goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropRoute {
    Convert(PathBuf),
    Library(PathBuf),
}

/// A `.pth` or `.zip` dropped on the Convert tab is converted; everything else goes to the library.
pub fn route_drop(on_convert_tab: bool, path: &Path) -> DropRoute {
    match classify_drop(path) {
        DropAction::OfferConvert(path) if on_convert_tab => DropRoute::Convert(path),
        _ => DropRoute::Library(path.to_path_buf()),
    }
}

/// The file name of `path`, for messages.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// "Could not import \"{file}\": {reason}"
fn failed(src: &Path, reason: impl std::fmt::Display) -> String {
    format!("Could not import \"{}\": {reason}", file_name(src))
}

/// A message on the Models tab.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    OfferConvert(PathBuf),
    Error(String),
    Info(String),
}

/// The text of the Open in Convert notice.
pub fn offer_convert_text(path: &Path) -> String {
    format!(
        "\"{}\" is a PyTorch model. Convert it to ONNX first.",
        file_name(path)
    )
}

/// Runs `job` on a "vc-import" thread. A spawn failure yields a closed channel.
fn spawn<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("vc-import".into())
        .spawn(move || {
            let _ = tx.send(job());
        });
    rx
}

const STOPPED: &str = "the import stopped unexpectedly";

enum Step {
    Idle,
    Inspecting {
        src: PathBuf,
        rx: Receiver<Result<ModelMeta, String>>,
    },
    NeedRate {
        src: PathBuf,
        f0: bool,
    },
    Copying {
        src: PathBuf,
        rx: Receiver<Result<StagedModel, String>>,
        sample_rate: u32,
        f0: bool,
    },
}

/// Imports queued files one at a time, on "vc-import" threads.
pub struct Importer {
    inspect: InspectFn,
    queue: VecDeque<PathBuf>,
    step: Step,
}

impl Importer {
    pub fn new(inspect: InspectFn) -> Self {
        Self {
            inspect,
            queue: VecDeque::new(),
            step: Step::Idle,
        }
    }

    pub fn enqueue(&mut self, src: PathBuf) {
        self.queue.push_back(src);
    }

    /// A file is in flight or waiting.
    pub fn is_busy(&self) -> bool {
        !matches!(self.step, Step::Idle) || !self.queue.is_empty()
    }

    /// The file whose sample rate the user must pick.
    pub fn needs_rate(&self) -> Option<&Path> {
        match &self.step {
            Step::NeedRate { src, .. } => Some(src),
            _ => None,
        }
    }

    /// Imports the waiting file at `sample_rate`.
    pub fn choose_rate(&mut self, sample_rate: u32, library: &Library) {
        if let Step::NeedRate { src, f0 } = std::mem::replace(&mut self.step, Step::Idle) {
            self.step = copying(src, sample_rate, f0, library);
        }
    }

    /// Skips the waiting file.
    pub fn cancel_rate(&mut self) {
        if matches!(self.step, Step::NeedRate { .. }) {
            self.step = Step::Idle;
        }
    }

    /// Advances the queue; returns `Ok(id)` per imported model and `Err(message)` per failure.
    pub fn poll(&mut self, library: &mut Library) -> Vec<Result<String, String>> {
        let mut results = Vec::new();
        loop {
            self.step = match std::mem::replace(&mut self.step, Step::Idle) {
                Step::Idle => {
                    let Some(src) = self.queue.pop_front() else {
                        return results;
                    };
                    let inspect = self.inspect.clone();
                    let path = src.clone();
                    Step::Inspecting {
                        src,
                        rx: spawn(move || inspect(&path)),
                    }
                }
                Step::Inspecting { src, rx } => match rx.try_recv() {
                    Err(TryRecvError::Empty) => {
                        self.step = Step::Inspecting { src, rx };
                        return results;
                    }
                    Ok(Ok(meta)) => match meta.sample_rate {
                        Some(sr) => copying(src, sr, meta.f0, library),
                        None => Step::NeedRate { src, f0: meta.f0 },
                    },
                    Ok(Err(e)) => {
                        results.push(Err(failed(&src, e)));
                        Step::Idle
                    }
                    Err(TryRecvError::Disconnected) => {
                        results.push(Err(failed(&src, STOPPED)));
                        Step::Idle
                    }
                },
                step @ Step::NeedRate { .. } => {
                    self.step = step;
                    return results;
                }
                Step::Copying {
                    src,
                    rx,
                    sample_rate,
                    f0,
                } => match rx.try_recv() {
                    Err(TryRecvError::Empty) => {
                        self.step = Step::Copying {
                            src,
                            rx,
                            sample_rate,
                            f0,
                        };
                        return results;
                    }
                    Ok(Ok(staged)) => {
                        let result = library.add(staged.clone(), sample_rate, f0);
                        results.push(match result {
                            Ok(entry) => Ok(entry.id),
                            Err(e) => {
                                discard_staged(library.root(), &staged);
                                Err(failed(&src, e))
                            }
                        });
                        Step::Idle
                    }
                    Ok(Err(e)) => {
                        results.push(Err(failed(&src, e)));
                        Step::Idle
                    }
                    Err(TryRecvError::Disconnected) => {
                        results.push(Err(failed(&src, STOPPED)));
                        Step::Idle
                    }
                },
            };
        }
    }
}

/// Starts copying `src` into the library, with the ids taken now.
fn copying(src: PathBuf, sample_rate: u32, f0: bool, library: &Library) -> Step {
    let root = library.root().to_path_buf();
    let taken = library.ids();
    let path = src.clone();
    Step::Copying {
        src,
        rx: spawn(move || copy_model(&root, &path, &taken).map_err(|e| e.to_string())),
        sample_rate,
        f0,
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::testutil::TempDir;

    fn meta(sample_rate: Option<u32>) -> ModelMeta {
        ModelMeta {
            sample_rate,
            f0: true,
        }
    }

    /// An importer whose inspection always gives `result`.
    fn importer(result: Result<ModelMeta, String>) -> Importer {
        Importer::new(Arc::new(move |_| result.clone()))
    }

    /// A source file `name` in `dir/src`, and an empty library in `dir/models`.
    fn setup(dir: &TempDir, names: &[&str]) -> (Vec<PathBuf>, Library) {
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let paths = names
            .iter()
            .map(|n| {
                let p = src.join(n);
                std::fs::write(&p, n.as_bytes()).unwrap();
                p
            })
            .collect();
        let (library, note) = Library::open(dir.path().join("models"));
        assert_eq!(note, None);
        (paths, library)
    }

    /// Polls every 10 ms, for up to 5 s, until the importer is idle or waits for a sample rate.
    fn drive(imp: &mut Importer, library: &mut Library) -> Vec<Result<String, String>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        loop {
            results.extend(imp.poll(library));
            if !imp.is_busy() || imp.needs_rate().is_some() {
                return results;
            }
            assert!(Instant::now() < deadline, "import timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn model_files(library: &Library) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(library.root()) else {
            return Vec::new();
        };
        let mut names: Vec<String> = dir
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".onnx"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn classify_drop_cases() {
        for name in ["voice.onnx", "Voice.ONNX", r"C:\m\a b.Onnx"] {
            let p = PathBuf::from(name);
            assert_eq!(classify_drop(&p), DropAction::Import(p.clone()), "{name}");
        }
        for name in ["voice.pth", "voice.PTH", "voice.zip"] {
            let p = PathBuf::from(name);
            assert_eq!(
                classify_drop(&p),
                DropAction::OfferConvert(p.clone()),
                "{name}"
            );
        }
        assert_eq!(
            classify_drop(Path::new(r"C:\m\notes.txt")),
            DropAction::Reject(
                "Could not import \"notes.txt\": only RVC v2 .onnx models can be imported here."
                    .into()
            )
        );
        assert_eq!(
            classify_drop(Path::new("README")),
            DropAction::Reject(
                "Could not import \"README\": only RVC v2 .onnx models can be imported here."
                    .into()
            )
        );
    }

    #[test]
    fn import_with_metadata_rate() {
        let dir = TempDir::new("import-metadata-rate");
        let (paths, mut library) = setup(&dir, &["Ana v2.onnx"]);
        let mut imp = importer(Ok(meta(Some(48000))));
        assert!(!imp.is_busy());
        imp.enqueue(paths[0].clone());
        assert!(imp.is_busy());
        assert_eq!(drive(&mut imp, &mut library), [Ok("ana-v2".to_string())]);
        assert!(!imp.is_busy());
        let entry = library.get("ana-v2").unwrap();
        assert_eq!(entry.name, "Ana v2");
        assert_eq!(entry.sample_rate, 48000);
        assert!(entry.f0);
        assert!(library.file_exists(entry));
    }

    #[test]
    fn import_without_rate_waits_for_choice() {
        let dir = TempDir::new("import-waits-for-rate");
        let (paths, mut library) = setup(&dir, &["bo.onnx"]);
        let mut imp = importer(Ok(meta(None)));
        imp.enqueue(paths[0].clone());
        assert_eq!(drive(&mut imp, &mut library), []);
        assert_eq!(imp.needs_rate(), Some(paths[0].as_path()));
        assert!(imp.is_busy());
        assert!(imp.poll(&mut library).is_empty());
        assert!(library.entries().is_empty());
        assert!(model_files(&library).is_empty());

        imp.choose_rate(40000, &library);
        assert_eq!(imp.needs_rate(), None);
        assert_eq!(drive(&mut imp, &mut library), [Ok("bo".to_string())]);
        assert_eq!(library.get("bo").unwrap().sample_rate, 40000);
        assert_eq!(model_files(&library), ["bo.onnx"]);
    }

    #[test]
    fn cancel_rate_skips_file() {
        let dir = TempDir::new("import-cancel-rate");
        let (paths, mut library) = setup(&dir, &["no-rate.onnx", "cy.onnx"]);
        let mut imp = Importer::new(Arc::new(|p: &Path| {
            Ok(meta(p.ends_with("cy.onnx").then_some(32000)))
        }));
        imp.enqueue(paths[0].clone());
        imp.enqueue(paths[1].clone());
        assert_eq!(drive(&mut imp, &mut library), []);
        assert_eq!(imp.needs_rate(), Some(paths[0].as_path()));
        imp.cancel_rate();
        assert_eq!(imp.needs_rate(), None);
        assert!(imp.is_busy(), "the next file is still queued");
        assert_eq!(drive(&mut imp, &mut library), [Ok("cy".to_string())]);
        assert_eq!(library.ids(), ["cy"]);
        assert_eq!(model_files(&library), ["cy.onnx"]);
    }

    #[test]
    fn unsupported_model_message() {
        let dir = TempDir::new("import-unsupported");
        let (paths, mut library) = setup(&dir, &["x.onnx"]);
        let mut imp = importer(Err("unsupported model: fp16 export".into()));
        imp.enqueue(paths[0].clone());
        assert_eq!(
            drive(&mut imp, &mut library),
            [Err(
                "Could not import \"x.onnx\": unsupported model: fp16 export".to_string()
            )]
        );
        assert!(!imp.is_busy());
        assert!(library.entries().is_empty());
        assert!(model_files(&library).is_empty());
    }

    #[test]
    fn queue_processes_in_order() {
        let dir = TempDir::new("import-in-order");
        let (paths, mut library) = setup(&dir, &["c.onnx", "a.onnx", "b.onnx", "a.onnx"]);
        let mut imp = importer(Ok(meta(Some(40000))));
        for p in &paths {
            imp.enqueue(p.clone());
        }
        let ids = ["c", "a", "b", "a-2"].map(|id| Ok(id.to_string()));
        assert_eq!(drive(&mut imp, &mut library), ids);
        assert_eq!(library.ids(), ["c", "a", "b", "a-2"]);
    }

    #[test]
    fn route_drop_cases() {
        for name in [r"C:\m\voice.pth", r"C:\m\voice.zip"] {
            let p = PathBuf::from(name);
            assert_eq!(
                route_drop(true, &p),
                DropRoute::Convert(p.clone()),
                "{name}"
            );
        }
        let pth = PathBuf::from(r"C:\m\voice.pth");
        assert_eq!(route_drop(false, &pth), DropRoute::Library(pth.clone()));
        let onnx = PathBuf::from(r"C:\m\voice.onnx");
        assert_eq!(route_drop(true, &onnx), DropRoute::Library(onnx.clone()));
    }

    #[test]
    fn offer_convert_text() {
        assert_eq!(
            super::offer_convert_text(Path::new(r"C:\models\Ana v2.pth")),
            "\"Ana v2.pth\" is a PyTorch model. Convert it to ONNX first."
        );
    }
}
