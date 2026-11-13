//! Conversion of PyTorch `.pth` RVC v2 models to ONNX.

pub mod checkpoint;
mod error;
#[cfg(test)]
mod mutation_tests;
pub mod onnx;
pub mod pickle;
pub mod pth;
pub mod template;
#[cfg(test)]
mod testutil;
#[cfg(feature = "validate")]
pub mod validate;

pub use error::ConvertError;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;

use sha2::{Digest, Sha256};

use crate::checkpoint::Checkpoint;
use crate::onnx::ModelProto;
use crate::pth::PthArchive;
use crate::template::{Template, check_shapes, fill, match_template, templates, write_metadata};

/// Name and version written into converted models' metadata.
pub const CONVERTER: &str = concat!("vc-convert ", env!("CARGO_PKG_VERSION"));

/// A cancel flag shared between the UI and a conversion worker.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once [`Cancel::cancel`] has been called on any clone.
    pub fn check(&self) -> Result<(), ConvertError> {
        if self.is_cancelled() {
            Err(ConvertError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// The stages of a conversion, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Fill,
    Check,
    Save,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Read, Stage::Fill, Stage::Check, Stage::Save];

    /// Position of the stage in [`Stage::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }
}

/// Progress of a conversion: the current stage and how far it is (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub stage: Stage,
    pub fraction: f32,
}

/// What a `.pth` holds, read before converting it.
#[derive(Clone, Debug, PartialEq)]
pub struct Inspection {
    pub source: PathBuf,
    /// The `.pth` file name, inside the zip if the source is one.
    pub pth_name: String,
    pub file_size: u64,
    pub fp16: bool,
    pub version: String,
    pub sample_rate: u32,
    pub f0: bool,
    pub speakers: u32,
    pub info: Option<String>,
    /// SHA-256 of the source file, lower-case hex.
    pub sha256: String,
}

/// A converted ONNX model, not yet saved.
#[derive(Debug)]
pub struct Converted {
    pub onnx: Vec<u8>,
    pub sample_rate: u32,
}

/// Reads the model at `path` without its tensor data. Config and shape
/// problems show up here, before Convert is pressed.
pub fn inspect(path: &Path) -> Result<Inspection, ConvertError> {
    let bytes = fs::read(path)?;
    inspect_with(path, bytes, templates())
}

/// Converts the inspected model, reporting progress on `progress` and
/// stopping with `Cancelled` once `cancel` is set.
pub fn convert(
    i: &Inspection,
    cancel: &Cancel,
    progress: &Sender<Progress>,
) -> Result<Converted, ConvertError> {
    let bytes = fs::read(&i.source)?;
    convert_with(i, bytes, templates(), cancel, progress)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The decoded template for `ckpt`, its shapes checked against the weights.
fn prepare(templates: &[Template], ckpt: &Checkpoint) -> Result<ModelProto, ConvertError> {
    let template = match_template(templates, &ckpt.config)?;
    let model = onnx::decode(template.bytes)
        .map_err(|e| ConvertError::NoTemplate(format!("{}: {e}", template.info.file)))?;
    check_shapes(&model, &ckpt.weights)?;
    Ok(model)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn inspect_with(
    source: &Path,
    bytes: Vec<u8>,
    templates: &[Template],
) -> Result<Inspection, ConvertError> {
    let sha256 = sha256_hex(&bytes);
    let file_size = bytes.len() as u64;
    let archive = PthArchive::open(bytes, &file_name(source))?;
    let ckpt = Checkpoint::from_value(&archive.root, &archive.name)?;
    prepare(templates, &ckpt)?;
    Ok(Inspection {
        source: source.to_path_buf(),
        pth_name: archive.name,
        file_size,
        fp16: ckpt.fp16,
        version: ckpt.version,
        sample_rate: ckpt.sample_rate,
        f0: ckpt.f0,
        speakers: ckpt.speakers,
        info: ckpt.info,
        sha256,
    })
}

fn send(progress: &Sender<Progress>, stage: Stage, fraction: f32) {
    let _ = progress.send(Progress { stage, fraction });
}

fn convert_with(
    i: &Inspection,
    bytes: Vec<u8>,
    templates: &[Template],
    cancel: &Cancel,
    progress: &Sender<Progress>,
) -> Result<Converted, ConvertError> {
    let converted = build_with(i, bytes, templates, cancel, progress)?;
    #[cfg(feature = "validate")]
    {
        send(progress, Stage::Check, 0.0);
        validate::check(&converted.onnx)?;
        send(progress, Stage::Check, 1.0);
    }
    Ok(converted)
}

/// The read and fill stages of [`convert_with`]: the filled model, not yet checked.
fn build_with(
    i: &Inspection,
    bytes: Vec<u8>,
    templates: &[Template],
    cancel: &Cancel,
    progress: &Sender<Progress>,
) -> Result<Converted, ConvertError> {
    let send = |stage, fraction| send(progress, stage, fraction);

    send(Stage::Read, 0.0);
    if sha256_hex(&bytes) != i.sha256 {
        return Err(ConvertError::Io(io::Error::other(
            "the file changed since it was inspected",
        )));
    }
    let mut archive = PthArchive::open(bytes, &file_name(&i.source))?;
    let ckpt = Checkpoint::from_value(&archive.root, &archive.name)?;
    match_template(templates, &ckpt.config)?;
    send(Stage::Read, 1.0);
    cancel.check()?;

    let mut model = prepare(templates, &ckpt)?;
    fill(
        &mut model,
        &ckpt.weights,
        &mut |t| archive.tensor_f32(t),
        cancel,
        &mut |f| send(Stage::Fill, f),
    )?;
    write_metadata(&mut model, &ckpt, &i.sha256);
    Ok(Converted {
        onnx: onnx::encode(&model),
        sample_rate: ckpt.sample_rate,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;
    use crate::pickle::DType;
    use crate::testutil::{Fixture, TempDir, config_48k, rvc_pth, toy_template};
    #[cfg(feature = "validate")]
    use crate::testutil::{assets_or_skip, synthetic_pth};
    #[cfg(feature = "validate")]
    use vc_engine::assets::ORT_DLL;

    /// The toy template for the 40k fixture's config.
    fn toy_40k() -> Template {
        let archive = PthArchive::open(rvc_pth(&Fixture::v2_40k()), "voice.pth").expect("open");
        let ckpt = Checkpoint::from_value(&archive.root, &archive.name).expect("classify");
        toy_template(ckpt.config)
    }

    /// `f` written as `voice.pth` in `dir`.
    fn write(dir: &TempDir, f: &Fixture) -> PathBuf {
        let path = dir.path().join("voice.pth");
        std::fs::write(&path, rvc_pth(f)).expect("write fixture");
        path
    }

    fn inspect_toy(path: &Path) -> Result<Inspection, ConvertError> {
        let bytes = std::fs::read(path).expect("read fixture");
        inspect_with(path, bytes, &[toy_40k()])
    }

    /// Reads and fills like [`convert`], without validation: the toy
    /// template has no graph to run.
    fn convert_toy(
        i: &Inspection,
        cancel: &Cancel,
        progress: &Sender<Progress>,
    ) -> Result<Converted, ConvertError> {
        let bytes = std::fs::read(&i.source).expect("read fixture");
        build_with(i, bytes, &[toy_40k()], cancel, progress)
    }

    fn inspected(dir: &TempDir) -> Inspection {
        inspect_toy(&write(dir, &Fixture::v2_40k())).expect("inspect")
    }

    #[test]
    fn inspect_reports_fixture() {
        let dir = TempDir::new("inspect-reports");
        let path = write(&dir, &Fixture::v2_40k());
        let bytes = std::fs::read(&path).expect("read");
        let i = inspect_toy(&path).expect("inspect");
        assert_eq!(
            i,
            Inspection {
                source: path,
                pth_name: "voice.pth".into(),
                file_size: bytes.len() as u64,
                fp16: true,
                version: "v2".into(),
                sample_rate: 40000,
                f0: true,
                speakers: 3,
                info: Some("300epoch".into()),
                sha256: format!("{:x}", Sha256::digest(&bytes)),
            }
        );
    }

    #[test]
    fn inspect_shows_problems_early() {
        let dir = TempDir::new("inspect-problems");

        let mut v1 = Fixture::v2_40k();
        v1.version = Some("v1");
        let r = inspect_toy(&write(&dir, &v1));
        assert!(matches!(r, Err(ConvertError::RvcV1)), "got {r:?}");

        let mut no_f0 = Fixture::v2_40k();
        no_f0.f0 = Some(0);
        let r = inspect_toy(&write(&dir, &no_f0));
        assert!(matches!(r, Err(ConvertError::NoF0)), "got {r:?}");

        let mut k48 = Fixture::v2_40k();
        k48.config = config_48k();
        let r = inspect_toy(&write(&dir, &k48));
        assert!(matches!(r, Err(ConvertError::NoTemplate(_))), "got {r:?}");

        let mut shape = Fixture::v2_40k();
        shape.weights[2].1 = vec![512, 192, 5];
        let r = inspect_toy(&write(&dir, &shape));
        assert!(matches!(r, Err(ConvertError::NoTemplate(_))), "got {r:?}");
    }

    #[test]
    fn inspect_missing_file_is_io() {
        let dir = TempDir::new("inspect-missing");
        let r = inspect(&dir.path().join("nothing.pth"));
        assert!(matches!(r, Err(ConvertError::Io(_))), "got {r:?}");
    }

    #[test]
    fn convert_writes_filled_model_with_metadata() {
        let dir = TempDir::new("convert-writes");
        let i = inspected(&dir);
        let (tx, _rx) = mpsc::channel();
        let out = convert_toy(&i, &Cancel::new(), &tx).expect("convert");
        assert_eq!(out.sample_rate, 40000);

        let m = onnx::decode(&out.onnx).expect("decode output");
        let inits = &m.graph.as_ref().expect("graph").initializer;
        assert!(inits.iter().all(|t| !template::is_stripped(t)));
        let emb_g = inits
            .iter()
            .find(|t| t.name.as_deref() == Some("emb_g.weight"))
            .expect("emb_g.weight");
        assert_eq!(emb_g.dims, [3, 256]);
        assert_eq!(emb_g.raw_data.as_ref().map(Vec::len), Some(3 * 256 * 4));
        assert_eq!(onnx::metadata(&m, "sr"), Some("40000"));
        assert_eq!(onnx::metadata(&m, "source_sha256"), Some(i.sha256.as_str()));
    }

    #[test]
    fn convert_progress_stages_in_order() {
        let dir = TempDir::new("convert-progress");
        let i = inspected(&dir);
        let (tx, rx) = mpsc::channel();
        convert_toy(&i, &Cancel::new(), &tx).expect("convert");
        drop(tx);
        let all: Vec<Progress> = rx.iter().collect();

        let stages: Vec<Stage> = all.iter().map(|p| p.stage).collect();
        let first_fill = stages.iter().position(|s| *s == Stage::Fill).expect("fill");
        assert!(first_fill > 0);
        assert!(stages[..first_fill].iter().all(|s| *s == Stage::Read));
        assert!(stages[first_fill..].iter().all(|s| *s == Stage::Fill));
        assert_eq!(all[first_fill - 1].fraction, 1.0);
        assert_eq!(all.last().map(|p| p.fraction), Some(1.0));
    }

    #[test]
    fn convert_cancelled_before_fill() {
        let dir = TempDir::new("convert-cancelled");
        let i = inspected(&dir);
        let cancel = Cancel::new();
        cancel.cancel();
        let (tx, rx) = mpsc::channel();
        let r = convert_toy(&i, &cancel, &tx);
        assert!(matches!(r, Err(ConvertError::Cancelled)), "got {r:?}");
        drop(tx);
        assert!(rx.iter().all(|p| p.stage == Stage::Read));
    }

    #[test]
    fn convert_detects_changed_file() {
        let dir = TempDir::new("convert-changed");
        let i = inspected(&dir);
        let mut f = Fixture::v2_40k();
        f.weights[3].2 = DType::F32;
        std::fs::write(&i.source, rvc_pth(&f)).expect("rewrite");
        let (tx, _rx) = mpsc::channel();
        let r = convert_toy(&i, &Cancel::new(), &tx);
        assert!(matches!(r, Err(ConvertError::Io(_))), "got {r:?}");
    }

    #[cfg(feature = "validate")]
    #[test]
    fn real_templates_fill_and_load() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        for t in templates() {
            let file = &t.info.file;
            let dir = TempDir::new(&format!("real-{file}"));
            let path = dir.path().join("voice.pth");
            std::fs::write(&path, synthetic_pth(t, 1)).expect("write pth");
            let i = inspect(&path).expect("inspect");
            let bytes = std::fs::read(&path).expect("read pth");
            let (tx, _rx) = mpsc::channel();
            let out = build_with(&i, bytes, templates(), &Cancel::new(), &tx).expect("fill");
            let (audio, sr) = validate::synthetic_output(&out.onnx).expect("run");

            assert_eq!(i.sample_rate, t.info.config.sr, "{file}");
            assert_eq!(i.speakers, 1, "{file}");
            assert_eq!(sr, t.info.config.sr, "{file}");
            assert_eq!(
                audio.len(),
                validate::FRAMES * (sr as usize / 100),
                "{file}"
            );
            assert!(audio.iter().all(|v| v.is_finite()), "{file}");
        }
    }

    #[cfg(feature = "validate")]
    #[test]
    fn convert_checks_real_template() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let t = &templates()[0];
        let dir = TempDir::new("convert-checks");
        let path = dir.path().join("voice.pth");
        std::fs::write(&path, synthetic_pth(t, 1)).expect("write pth");
        let i = inspect(&path).expect("inspect");
        let (tx, rx) = mpsc::channel();
        let out = convert(&i, &Cancel::new(), &tx).expect("convert");
        assert_eq!(out.sample_rate, t.info.config.sr);
        drop(tx);
        let checks: Vec<f32> = rx
            .iter()
            .filter(|p| p.stage == Stage::Check)
            .map(|p| p.fraction)
            .collect();
        assert_eq!(checks, [0.0, 1.0]);
    }

    #[test]
    fn cancel_is_shared_between_clones() {
        let cancel = Cancel::new();
        let clone = cancel.clone();
        assert!(!clone.is_cancelled());
        assert!(clone.check().is_ok());
        cancel.cancel();
        assert!(clone.is_cancelled());
        assert!(matches!(clone.check(), Err(ConvertError::Cancelled)));
    }

    #[test]
    fn stage_index_order() {
        let indices: Vec<usize> = Stage::ALL.iter().map(|s| s.index()).collect();
        assert_eq!(indices, [0, 1, 2, 3]);
        assert_eq!(
            Stage::ALL,
            [Stage::Read, Stage::Fill, Stage::Check, Stage::Save]
        );
    }
}
