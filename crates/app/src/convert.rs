//! Converting a PyTorch `.pth` into a library model: inspection, conversion and export run on
//! "vc-convert" threads, and [`Converter::poll`] moves the Convert tab between its states.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use vc_convert::{Cancel, ConvertError, Converted, Inspection, Progress, Stage};
use vc_models::{Library, LibraryError, StagedModel, discard_staged, stage_bytes};

/// Reads a `.pth` (or a `.zip` holding one) without converting it.
pub type InspectFn = Arc<dyn Fn(&Path) -> Result<Inspection, ConvertError> + Send + Sync>;
/// Converts an inspected model, reporting progress and stopping when cancelled.
pub type ConvertFn = Arc<
    dyn Fn(&Inspection, &Cancel, &Sender<Progress>) -> Result<Converted, ConvertError>
        + Send
        + Sync,
>;

/// What the user chose for the new model.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub name: String,
    pub pitch: i32,
    pub icon: Option<PathBuf>,
}

/// The Convert tab's state.
pub enum State {
    Empty,
    Inspecting {
        path: PathBuf,
        rx: Receiver<Result<Inspection, ConvertError>>,
    },
    Ready {
        inspection: Inspection,
        form: Form,
    },
    Converting {
        inspection: Inspection,
        form: Form,
        progress: Progress,
        cancel: Cancel,
        progress_rx: Receiver<Progress>,
        rx: Receiver<Result<(StagedModel, u32), ConvertError>>,
    },
    Done {
        id: String,
        name: String,
        note: Option<String>,
        export_rx: Option<Receiver<String>>,
    },
    Problem {
        file: String,
        message: String,
    },
}

const STOPPED: &str = "The conversion stopped unexpectedly. Details are in the log.";

/// Runs `job` on a "vc-convert" thread. A spawn failure yields a closed channel.
fn spawn<T: Send + 'static>(job: impl FnOnce() -> T + Send + 'static) -> Receiver<T> {
    let (tx, rx) = mpsc::channel();
    let _ = thread::Builder::new()
        .name("vc-convert".into())
        .spawn(move || {
            let _ = tx.send(job());
        });
    rx
}

/// The file name of `path`, for messages.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A library error as a conversion error, so it gets a user message.
fn library_error(e: LibraryError) -> ConvertError {
    match e {
        LibraryError::Io(e) => ConvertError::from(e),
        e => ConvertError::Io(io::Error::other(e.to_string())),
    }
}

/// Inspects and converts one file at a time, on "vc-convert" threads.
pub struct Converter {
    inspect: InspectFn,
    convert: ConvertFn,
    pub state: State,
}

impl Converter {
    pub fn new(inspect: InspectFn, convert: ConvertFn) -> Self {
        Self {
            inspect,
            convert,
            state: State::Empty,
        }
    }

    /// Inspects `path`. Ignored while converting.
    pub fn open(&mut self, path: PathBuf) {
        if self.is_converting() {
            return;
        }
        let inspect = self.inspect.clone();
        let src = path.clone();
        self.state = State::Inspecting {
            path,
            rx: spawn(move || inspect(&src)),
        };
    }

    /// Starts converting when Ready with a name that isn't blank.
    pub fn start(&mut self, library: &Library) {
        let State::Ready { inspection, form } = &self.state else {
            return;
        };
        if form.name.trim().is_empty() {
            return;
        }
        let (inspection, form) = (inspection.clone(), form.clone());
        let cancel = Cancel::new();
        let (progress_tx, progress_rx) = mpsc::channel();
        let convert = self.convert.clone();
        let root = library.root().to_path_buf();
        let taken = library.ids();
        let (i, name, flag) = (inspection.clone(), form.name.clone(), cancel.clone());
        let rx = spawn(move || {
            let r = convert(&i, &flag, &progress_tx)?;
            let _ = progress_tx.send(Progress {
                stage: Stage::Save,
                fraction: 0.0,
            });
            flag.check()?;
            let staged = stage_bytes(&root, &name, &r.onnx, &taken).map_err(library_error)?;
            if flag.is_cancelled() {
                discard_staged(&root, &staged);
                return Err(ConvertError::Cancelled);
            }
            let _ = progress_tx.send(Progress {
                stage: Stage::Save,
                fraction: 1.0,
            });
            Ok((staged, r.sample_rate))
        });
        self.state = State::Converting {
            inspection,
            form,
            progress: Progress {
                stage: Stage::Read,
                fraction: 0.0,
            },
            cancel,
            progress_rx,
            rx,
        };
    }

    /// Asks the conversion to stop; the state changes when the worker answers.
    pub fn cancel(&mut self) {
        if let State::Converting { cancel, .. } = &self.state {
            cancel.cancel();
        }
    }

    /// Back to no file (Convert another…).
    pub fn reset(&mut self) {
        self.state = State::Empty;
    }

    pub fn is_converting(&self) -> bool {
        matches!(self.state, State::Converting { .. })
    }

    /// A worker is running: inspecting, converting or exporting.
    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            State::Inspecting { .. }
                | State::Converting { .. }
                | State::Done {
                    export_rx: Some(_),
                    ..
                }
        )
    }

    /// Copies the converted model `src` to `dest`. Only when Done and not already exporting.
    pub fn export(&mut self, src: PathBuf, dest: PathBuf) {
        if let State::Done {
            export_rx: export_rx @ None,
            ..
        } = &mut self.state
        {
            *export_rx = Some(spawn(move || export_note(&dest, fs::copy(&src, &dest))));
        }
    }

    /// Takes the workers' answers; returns the id of a model added to `library`.
    pub fn poll(&mut self, library: &mut Library) -> Option<String> {
        let mut added = None;
        self.state = match std::mem::replace(&mut self.state, State::Empty) {
            State::Inspecting { path, rx } => match rx.try_recv() {
                Err(TryRecvError::Empty) => State::Inspecting { path, rx },
                Ok(Ok(inspection)) => State::Ready {
                    form: Form {
                        name: default_name(&path),
                        pitch: 0,
                        icon: None,
                    },
                    inspection,
                },
                Ok(Err(e)) => {
                    tracing::warn!("inspecting {}: {e}", path.display());
                    problem(&path, e.user_message())
                }
                Err(TryRecvError::Disconnected) => {
                    tracing::warn!("inspecting {}: the worker stopped", path.display());
                    problem(&path, STOPPED)
                }
            },
            State::Converting {
                inspection,
                form,
                mut progress,
                cancel,
                progress_rx,
                rx,
            } => {
                if let Some(last) = progress_rx.try_iter().last() {
                    progress = last;
                }
                let path = inspection.source.clone();
                match rx.try_recv() {
                    Err(TryRecvError::Empty) => State::Converting {
                        inspection,
                        form,
                        progress,
                        cancel,
                        progress_rx,
                        rx,
                    },
                    Ok(Ok((staged, sample_rate))) => match add(library, staged, sample_rate, &form)
                    {
                        Ok((id, name, note)) => {
                            added = Some(id.clone());
                            State::Done {
                                id,
                                name,
                                note,
                                export_rx: None,
                            }
                        }
                        Err(e) => {
                            tracing::warn!("adding {} to the library: {e}", path.display());
                            problem(&path, e.user_message())
                        }
                    },
                    Ok(Err(ConvertError::Cancelled)) => State::Ready { inspection, form },
                    Ok(Err(e)) => {
                        tracing::warn!("converting {}: {e}", path.display());
                        problem(&path, e.user_message())
                    }
                    Err(TryRecvError::Disconnected) => {
                        tracing::warn!("converting {}: the worker stopped", path.display());
                        problem(&path, STOPPED)
                    }
                }
            }
            State::Done {
                id,
                name,
                note,
                export_rx: Some(rx),
            } => match rx.try_recv() {
                Err(TryRecvError::Empty) => State::Done {
                    id,
                    name,
                    note,
                    export_rx: Some(rx),
                },
                Ok(exported) => State::Done {
                    id,
                    name,
                    note: Some(exported),
                    export_rx: None,
                },
                Err(TryRecvError::Disconnected) => State::Done {
                    id,
                    name,
                    note: Some("Could not export: the export stopped unexpectedly".into()),
                    export_rx: None,
                },
            },
            state => state,
        };
        added
    }
}

/// The Problem state for `path`.
fn problem(path: &Path, message: &str) -> State {
    State::Problem {
        file: file_name(path),
        message: message.to_string(),
    }
}

/// Indexes `staged` with the form's pitch and icon; returns its id, name and a note when the icon
/// couldn't be set. If indexing fails, the file is deleted.
fn add(
    library: &mut Library,
    staged: StagedModel,
    sample_rate: u32,
    form: &Form,
) -> Result<(String, String, Option<String>), ConvertError> {
    let entry = library
        .add(staged.clone(), sample_rate, true)
        .map_err(|e| {
            discard_staged(library.root(), &staged);
            library_error(e)
        })?;
    if let Err(e) = library.set_default_pitch(&entry.id, form.pitch) {
        tracing::warn!("setting the default pitch of {}: {e}", entry.id);
    }
    let note = form.icon.as_ref().and_then(|icon| {
        library
            .set_icon(&entry.id, icon)
            .err()
            .map(|e| format!("The icon couldn't be set: {e}"))
    });
    Ok((entry.id, entry.name, note))
}

/// The file stem of the chosen file: the default model name.
pub fn default_name(source: &Path) -> String {
    source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// How far the whole conversion is, from 0 to 1; each stage is a quarter.
pub fn overall(p: Progress) -> f32 {
    (p.stage.index() as f32 + p.fraction.clamp(0.0, 1.0)) / Stage::ALL.len() as f32
}

pub fn stage_label(s: Stage) -> &'static str {
    match s {
        Stage::Read => "Reading file…",
        Stage::Fill => "Filling weights…",
        Stage::Check => "Checking model…",
        Stage::Save => "Saving to library…",
    }
}

/// The note shown after exporting to `dest`.
pub fn export_note(dest: &Path, r: io::Result<u64>) -> String {
    match r {
        Ok(_) => format!("Exported to \"{}\".", dest.display()),
        Err(e) if e.kind() == io::ErrorKind::StorageFull => {
            ConvertError::DiskFull.user_message().to_string()
        }
        Err(e) => format!("Could not export: {e}"),
    }
}
#[cfg(test)]
mod tests {
    use std::io;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::testutil::TempDir;

    const ANA: &str = "C:/x/Ana v2.pth";

    /// A filled inspection of `path`.
    fn inspection(path: &Path) -> Inspection {
        Inspection {
            source: path.to_path_buf(),
            pth_name: "Ana v2.pth".into(),
            file_size: 55 << 20,
            fp16: true,
            version: "v2".into(),
            sample_rate: 40000,
            f0: true,
            speakers: 1,
            info: Some("300epoch".into()),
            sha256: "ab".repeat(32),
        }
    }

    fn inspect_ok() -> InspectFn {
        Arc::new(|p: &Path| Ok(inspection(p)))
    }

    /// A converter whose conversion sends {Fill, 0.5} and gives `result`.
    fn converter(result: fn() -> Result<Converted, ConvertError>) -> Converter {
        Converter::new(
            inspect_ok(),
            Arc::new(move |_, _, progress| {
                let _ = progress.send(Progress {
                    stage: Stage::Fill,
                    fraction: 0.5,
                });
                result()
            }),
        )
    }

    fn converted() -> Result<Converted, ConvertError> {
        Ok(Converted {
            onnx: b"onnx".to_vec(),
            sample_rate: 48000,
        })
    }

    /// An empty library in `dir/models`.
    fn library(dir: &TempDir) -> Library {
        let (library, note) = Library::open(dir.path().join("models"));
        assert_eq!(note, None);
        library
    }

    /// Polls every 10 ms, for up to 5 s, until the converter is not busy; returns the added ids.
    fn drive(c: &mut Converter, library: &mut Library) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ids = Vec::new();
        loop {
            ids.extend(c.poll(library));
            if !c.is_busy() {
                return ids;
            }
            assert!(Instant::now() < deadline, "conversion timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The `.onnx` files in the library folder.
    fn model_files(library: &Library) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(library.root()) else {
            return Vec::new();
        };
        dir.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".onnx"))
            .collect()
    }

    /// `c` opened on [`ANA`] and driven to Ready.
    fn ready(c: &mut Converter, library: &mut Library) {
        c.open(PathBuf::from(ANA));
        assert!(c.is_busy());
        assert_eq!(drive(c, library), Vec::<String>::new());
        assert!(matches!(c.state, State::Ready { .. }));
    }

    fn form_mut(c: &mut Converter) -> &mut Form {
        match &mut c.state {
            State::Ready { form, .. } => form,
            _ => panic!("not Ready"),
        }
    }

    fn problem(c: &Converter) -> (&str, &str) {
        match &c.state {
            State::Problem { file, message } => (file, message),
            _ => panic!("not a Problem"),
        }
    }

    fn done_note(c: &Converter) -> Option<&str> {
        match &c.state {
            State::Done { note, .. } => note.as_deref(),
            _ => panic!("not Done"),
        }
    }

    #[test]
    fn open_inspects_then_ready() {
        let dir = TempDir::new("convert-open");
        let mut library = library(&dir);
        let mut c = converter(converted);
        assert!(matches!(c.state, State::Empty));
        assert!(!c.is_busy());
        ready(&mut c, &mut library);
        match &c.state {
            State::Ready {
                inspection: i,
                form,
            } => {
                assert_eq!(*i, inspection(Path::new(ANA)));
                assert_eq!(
                    *form,
                    Form {
                        name: "Ana v2".into(),
                        pitch: 0,
                        icon: None
                    }
                );
            }
            _ => panic!("not Ready"),
        }
    }

    #[test]
    fn inspect_error_is_problem() {
        let dir = TempDir::new("convert-inspect-error");
        let mut library = library(&dir);
        let mut c = Converter::new(
            Arc::new(|_| Err(ConvertError::RvcV1)),
            Arc::new(|_, _, _| converted()),
        );
        c.open(PathBuf::from(ANA));
        drive(&mut c, &mut library);
        assert_eq!(
            problem(&c),
            ("Ana v2.pth", ConvertError::RvcV1.user_message())
        );
    }

    #[test]
    fn convert_success_adds_model() {
        let dir = TempDir::new("convert-success");
        let mut library = library(&dir);
        let mut c = converter(converted);
        ready(&mut c, &mut library);
        let form = form_mut(&mut c);
        form.pitch = 5;
        form.name = "My voice".into();
        c.start(&library);
        assert!(c.is_converting());
        assert_eq!(drive(&mut c, &mut library), ["my-voice"]);
        assert!(!c.is_converting());

        let entry = library.get("my-voice").unwrap();
        assert_eq!(entry.sample_rate, 48000);
        assert!(entry.f0);
        assert_eq!(entry.default_pitch, 5);
        assert_eq!(entry.icon, None);
        assert_eq!(std::fs::read(library.model_path(entry)).unwrap(), b"onnx");
        match &c.state {
            State::Done {
                id,
                name,
                note,
                export_rx,
            } => {
                assert_eq!(id, "my-voice");
                assert_eq!(name, "My voice");
                assert_eq!(*note, None);
                assert!(export_rx.is_none());
            }
            _ => panic!("not Done"),
        }
    }

    #[test]
    fn progress_keeps_last_value() {
        let dir = TempDir::new("convert-progress");
        let mut library = library(&dir);
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let go_rx = std::sync::Mutex::new(go_rx);
        let mut c = Converter::new(
            inspect_ok(),
            Arc::new(move |_, _, progress| {
                for fraction in [0.25, 0.5] {
                    let _ = progress.send(Progress {
                        stage: Stage::Fill,
                        fraction,
                    });
                }
                let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(5));
                converted()
            }),
        );
        ready(&mut c, &mut library);
        c.start(&library);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            assert_eq!(c.poll(&mut library), None);
            let State::Converting { progress, .. } = &c.state else {
                panic!("not Converting");
            };
            if progress.fraction == 0.5 {
                assert_eq!(progress.stage, Stage::Fill);
                break;
            }
            assert!(Instant::now() < deadline, "no progress");
            std::thread::sleep(Duration::from_millis(10));
        }
        go_tx.send(()).unwrap();
        assert_eq!(drive(&mut c, &mut library), ["ana-v2"]);
    }

    #[test]
    fn convert_with_icon_sets_icon() {
        let dir = TempDir::new("convert-icon");
        let mut library = library(&dir);
        let icon = dir.path().join("face.png");
        std::fs::write(&icon, b"png").unwrap();
        let mut c = converter(converted);
        ready(&mut c, &mut library);
        form_mut(&mut c).icon = Some(icon);
        c.start(&library);
        assert_eq!(drive(&mut c, &mut library), ["ana-v2"]);
        assert!(library.get("ana-v2").unwrap().icon.is_some());
        assert_eq!(done_note(&c), None);
    }

    #[test]
    fn bad_icon_is_note_not_failure() {
        let dir = TempDir::new("convert-bad-icon");
        let mut library = library(&dir);
        let icon = dir.path().join("face.txt");
        std::fs::write(&icon, b"text").unwrap();
        let mut c = converter(converted);
        ready(&mut c, &mut library);
        form_mut(&mut c).icon = Some(icon);
        c.start(&library);
        assert_eq!(drive(&mut c, &mut library), ["ana-v2"]);
        let entry = library.get("ana-v2").unwrap();
        assert_eq!(entry.icon, None);
        assert!(library.file_exists(entry));
        let note = done_note(&c).expect("a note");
        assert!(note.starts_with("The icon couldn't be set: "), "{note}");
    }

    #[test]
    fn convert_error_is_problem_and_writes_nothing() {
        let dir = TempDir::new("convert-error");
        let mut library = library(&dir);
        let mut c = converter(|| Err(ConvertError::ValidationFailed("NaN".into())));
        ready(&mut c, &mut library);
        c.start(&library);
        assert_eq!(drive(&mut c, &mut library), Vec::<String>::new());
        assert_eq!(
            problem(&c),
            (
                "Ana v2.pth",
                "The converted model didn't pass the checks and wasn't added."
            )
        );
        assert!(library.entries().is_empty());
        assert!(model_files(&library).is_empty());
    }

    #[test]
    fn disk_full_message() {
        let dir = TempDir::new("convert-disk-full");
        let mut library = library(&dir);
        let mut c = converter(|| Err(ConvertError::DiskFull));
        ready(&mut c, &mut library);
        c.start(&library);
        drive(&mut c, &mut library);
        assert_eq!(
            problem(&c).1,
            "Not enough disk space to save the converted model."
        );
    }

    #[test]
    fn stopped_worker_is_problem() {
        let dir = TempDir::new("convert-stopped");
        let mut library = library(&dir);
        let mut c = Converter::new(inspect_ok(), Arc::new(|_, _, _| panic!("worker crashed")));
        ready(&mut c, &mut library);
        c.start(&library);
        drive(&mut c, &mut library);
        assert_eq!(problem(&c), ("Ana v2.pth", STOPPED));
    }

    #[test]
    fn cancel_returns_to_ready() {
        let dir = TempDir::new("convert-cancel");
        let mut library = library(&dir);
        let mut c = Converter::new(
            inspect_ok(),
            Arc::new(|_, cancel: &Cancel, _| {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancel.is_cancelled() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(ConvertError::Cancelled)
            }),
        );
        ready(&mut c, &mut library);
        let before = match &c.state {
            State::Ready { form, .. } => form.clone(),
            _ => unreachable!(),
        };
        c.start(&library);
        assert!(c.is_converting());
        c.cancel();
        assert_eq!(drive(&mut c, &mut library), Vec::<String>::new());
        match &c.state {
            State::Ready {
                form,
                inspection: i,
            } => {
                assert_eq!(*form, before);
                assert_eq!(*i, inspection(Path::new(ANA)));
            }
            _ => panic!("not Ready"),
        }
        assert!(library.entries().is_empty());
        assert!(model_files(&library).is_empty());
    }

    #[test]
    fn open_ignored_while_converting() {
        let dir = TempDir::new("convert-open-ignored");
        let mut library = library(&dir);
        let mut c = Converter::new(
            inspect_ok(),
            Arc::new(|_, cancel: &Cancel, _| {
                let deadline = Instant::now() + Duration::from_secs(5);
                while !cancel.is_cancelled() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(ConvertError::Cancelled)
            }),
        );
        ready(&mut c, &mut library);
        c.start(&library);
        c.open(PathBuf::from("C:/x/Other.pth"));
        assert!(c.is_converting());
        c.cancel();
        drive(&mut c, &mut library);
        match &c.state {
            State::Ready { inspection: i, .. } => assert_eq!(i.source, Path::new(ANA)),
            _ => panic!("not Ready"),
        }
    }

    #[test]
    fn start_needs_name() {
        let dir = TempDir::new("convert-needs-name");
        let mut library = library(&dir);
        let mut c = converter(converted);
        ready(&mut c, &mut library);
        form_mut(&mut c).name = "  ".into();
        c.start(&library);
        assert!(!c.is_converting());
        assert!(matches!(c.state, State::Ready { .. }));
    }

    #[test]
    fn reset_goes_empty() {
        let dir = TempDir::new("convert-reset");
        let mut library = library(&dir);
        let mut c = converter(converted);
        ready(&mut c, &mut library);
        c.reset();
        assert!(matches!(c.state, State::Empty));
    }

    #[test]
    fn default_name_is_file_stem() {
        assert_eq!(default_name(Path::new(ANA)), "Ana v2");
        assert_eq!(default_name(Path::new(r"C:\m\voice.zip")), "voice");
    }

    #[test]
    fn overall_and_stage_labels() {
        let p = |stage, fraction| Progress { stage, fraction };
        assert_eq!(overall(p(Stage::Read, 0.0)), 0.0);
        assert_eq!(overall(p(Stage::Fill, 0.5)), 0.375);
        assert_eq!(overall(p(Stage::Save, 1.0)), 1.0);
        assert_eq!(overall(p(Stage::Check, 7.0)), 0.75);
        let labels = Stage::ALL.map(stage_label);
        assert_eq!(
            labels,
            [
                "Reading file…",
                "Filling weights…",
                "Checking model…",
                "Saving to library…"
            ]
        );
    }

    #[test]
    fn export_note_cases() {
        let dest = Path::new(r"C:\out\my-voice.onnx");
        assert_eq!(
            export_note(dest, Ok(4)),
            r#"Exported to "C:\out\my-voice.onnx"."#
        );
        assert_eq!(
            export_note(dest, Err(io::Error::from(io::ErrorKind::StorageFull))),
            "Not enough disk space to save the converted model."
        );
        assert_eq!(
            export_note(dest, Err(io::Error::other("denied"))),
            "Could not export: denied"
        );
    }

    #[test]
    fn export_copies_file() {
        let dir = TempDir::new("convert-export");
        let mut library = library(&dir);
        let mut c = converter(converted);
        ready(&mut c, &mut library);
        c.start(&library);
        assert_eq!(drive(&mut c, &mut library), ["ana-v2"]);
        let src = library.model_path(library.get("ana-v2").unwrap());
        let dest = dir.path().join("exported.onnx");
        c.export(src, dest.clone());
        assert!(c.is_busy());
        assert_eq!(drive(&mut c, &mut library), Vec::<String>::new());
        assert_eq!(std::fs::read(&dest).unwrap(), b"onnx");
        assert_eq!(
            done_note(&c),
            Some(format!("Exported to \"{}\".", dest.display()).as_str())
        );
    }
}
