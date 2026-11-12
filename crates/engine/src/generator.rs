//! RVC v2 generator in the RVC WebUI ONNX export format: signature checks, sample rate and inference.

use std::path::Path;

use ort::session::Session;
use ort::value::{Outlet, Tensor, TensorElementType};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::error::{EngineError, Result};
use crate::hubert::DIM;
use crate::runtime::{self, ActiveProvider, Provider};

/// Channels of the WebUI `rnd` noise input.
const NOISE_CHANNELS: usize = 192;
/// Output sample rates of RVC v2 generators.
const RATES: [u32; 3] = [32000, 40000, 48000];

/// Element type of a model input or output, as far as validation cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElemType {
    F32,
    F16,
    I64,
    Other,
}

/// Name, element type and shape (`-1` for a dynamic dimension) of a model input or output.
#[derive(Clone, Debug)]
pub struct IoSpec {
    pub name: String,
    pub elem: ElemType,
    pub dims: Vec<i64>,
}

/// Checks that a model has the RVC WebUI v2 export signature with F0.
pub fn validate_signature(inputs: &[IoSpec], outputs: &[IoSpec]) -> Result<()> {
    if inputs
        .iter()
        .chain(outputs)
        .any(|io| io.elem == ElemType::F16)
    {
        return unsupported("fp16 export");
    }
    if find(inputs, "pitch").is_none() || find(inputs, "pitchf").is_none() {
        return unsupported("no F0 inputs (models without pitch guidance are not supported)");
    }
    let Some(phone) = find(inputs, "phone") else {
        return unsupported("not an RVC WebUI ONNX export (missing input `phone`)");
    };
    match phone.dims.last().copied() {
        Some(256) => return unsupported("RVC v1 (256-dim)"),
        Some(d) if d != DIM as i64 && d != -1 => {
            return unsupported(&format!("phone has {d} features, expected {DIM}"));
        }
        _ => {}
    }
    let required = [
        ("phone", ElemType::F32, 3),
        ("phone_lengths", ElemType::I64, 1),
        ("pitch", ElemType::I64, 2),
        ("pitchf", ElemType::F32, 2),
        ("ds", ElemType::I64, 1),
        ("rnd", ElemType::F32, 3),
    ];
    for (name, elem, rank) in required {
        let Some(io) = find(inputs, name) else {
            return unsupported(&format!("missing input `{name}`"));
        };
        check_type(io, elem, rank)?;
    }
    let Some(audio) = find(outputs, "audio") else {
        return unsupported("missing output `audio`");
    };
    check_type(audio, ElemType::F32, 3)
}

/// The generator's output rate: `override_sr` if given, else the `sr` metadata value.
pub fn resolve_sr(metadata: Option<&str>, override_sr: Option<u32>) -> Result<u32> {
    let Some(sr) = override_sr.or_else(|| metadata.and_then(|m| m.trim().parse().ok())) else {
        return unsupported("unknown sample rate: pass --sr");
    };
    if !RATES.contains(&sr) {
        return unsupported(&format!(
            "unsupported sample rate {sr} (expected 32000, 40000 or 48000)"
        ));
    }
    Ok(sr)
}

fn find<'a>(list: &'a [IoSpec], name: &str) -> Option<&'a IoSpec> {
    list.iter().find(|io| io.name == name)
}

fn check_type(io: &IoSpec, elem: ElemType, rank: usize) -> Result<()> {
    if io.elem == elem && io.dims.len() == rank {
        return Ok(());
    }
    unsupported(&format!(
        "`{}` is {:?} rank {}, expected {elem:?} rank {rank}",
        io.name,
        io.elem,
        io.dims.len()
    ))
}

fn unsupported<T>(reason: &str) -> Result<T> {
    Err(EngineError::UnsupportedModel {
        reason: reason.to_string(),
    })
}

/// One generator call: `frames` rows of 768 features with the F0 of each frame.
pub struct GenInput<'a> {
    pub phone: &'a [f32],
    pub frames: usize,
    pub pitch: &'a [i64],
    pub pitchf: &'a [f32],
    pub speaker_id: i64,
}

/// An RVC v2 generator session.
pub struct Generator {
    session: Session,
    sr: u32,
    rng: StdRng,
    active: ActiveProvider,
}

impl Generator {
    /// Opens the generator at `path` on `provider`, falling back to the CPU, and checks its signature.
    pub fn load(path: &Path, provider: Provider, sr_override: Option<u32>) -> Result<Self> {
        let (session, active) = runtime::open_session(path, provider)?;
        Self::new(session, active, sr_override)
    }

    /// Like [`Generator::load`], for a model held in memory.
    pub fn load_from_memory(
        bytes: &[u8],
        provider: Provider,
        sr_override: Option<u32>,
    ) -> Result<Self> {
        let (session, active) = runtime::open_session_from_memory(bytes, provider)?;
        Self::new(session, active, sr_override)
    }

    fn new(session: Session, active: ActiveProvider, sr_override: Option<u32>) -> Result<Self> {
        let inputs: Vec<IoSpec> = session.inputs().iter().map(io_spec).collect();
        let outputs: Vec<IoSpec> = session.outputs().iter().map(io_spec).collect();
        validate_signature(&inputs, &outputs)?;
        let metadata_sr = session.metadata().map_err(inference)?.custom("sr");
        let sr = resolve_sr(metadata_sr.as_deref(), sr_override)?;
        Ok(Self {
            session,
            sr,
            rng: StdRng::seed_from_u64(0),
            active,
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.sr
    }

    pub fn active_provider(&self) -> ActiveProvider {
        self.active
    }

    /// Audio at [`Generator::sample_rate`] for `input.frames` frames (the flattened `[1,1,N]` output).
    pub fn run(&mut self, input: &GenInput) -> Result<Vec<f32>> {
        let t = input.frames;
        let rnd = standard_normal(&mut self.rng, NOISE_CHANNELS * t);
        let inputs = ort::inputs! {
            "phone" => Tensor::from_array(([1usize, t, DIM], input.phone.to_vec())).map_err(inference)?,
            "phone_lengths" => Tensor::from_array(([1usize], vec![t as i64])).map_err(inference)?,
            "pitch" => Tensor::from_array(([1usize, t], input.pitch.to_vec())).map_err(inference)?,
            "pitchf" => Tensor::from_array(([1usize, t], input.pitchf.to_vec())).map_err(inference)?,
            "ds" => Tensor::from_array(([1usize], vec![input.speaker_id])).map_err(inference)?,
            "rnd" => Tensor::from_array(([1usize, NOISE_CHANNELS, t], rnd)).map_err(inference)?,
        };
        let outputs = self.session.run(inputs).map_err(inference)?;
        let (_, audio) = outputs["audio"]
            .try_extract_tensor::<f32>()
            .map_err(inference)?;
        if !audio.iter().all(|v| v.is_finite()) {
            return Err(EngineError::Inference(
                "model produced non-finite audio".to_string(),
            ));
        }
        Ok(audio.to_vec())
    }
}

fn io_spec(outlet: &Outlet) -> IoSpec {
    let ty = outlet.dtype();
    IoSpec {
        name: outlet.name().to_string(),
        elem: match ty.tensor_type() {
            Some(TensorElementType::Float32) => ElemType::F32,
            Some(TensorElementType::Float16) => ElemType::F16,
            Some(TensorElementType::Int64) => ElemType::I64,
            _ => ElemType::Other,
        },
        dims: ty.tensor_shape().map_or_else(Vec::new, |s| s.to_vec()),
    }
}

/// `n` standard normal samples by the Box–Muller transform.
fn standard_normal(rng: &mut StdRng, n: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(n + 1);
    while out.len() < n {
        let u1 = rng.random::<f32>().max(1e-7);
        let u2 = rng.random::<f32>();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = std::f32::consts::TAU * u2;
        out.push(r * theta.cos());
        out.push(r * theta.sin());
    }
    out.truncate(n);
    out
}

fn inference(e: ort::Error) -> EngineError {
    EngineError::Inference(e.to_string())
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;

    use super::*;
    use crate::assets::ORT_DLL;
    use crate::error::EngineError;
    use crate::testutil::assets_or_skip;
    use crate::testutil::onnx::{self, F16, F32, rvc_inputs, rvc_output};

    fn io(name: &str, elem: ElemType, dims: &[i64]) -> IoSpec {
        IoSpec {
            name: name.to_string(),
            elem,
            dims: dims.to_vec(),
        }
    }

    fn webui_inputs() -> Vec<IoSpec> {
        vec![
            io("phone", ElemType::F32, &[1, -1, 768]),
            io("phone_lengths", ElemType::I64, &[1]),
            io("pitch", ElemType::I64, &[1, -1]),
            io("pitchf", ElemType::F32, &[1, -1]),
            io("ds", ElemType::I64, &[1]),
            io("rnd", ElemType::F32, &[1, 192, -1]),
        ]
    }

    fn webui_outputs() -> Vec<IoSpec> {
        vec![io("audio", ElemType::F32, &[1, 1, -1])]
    }

    /// The `UnsupportedModel` reason, or a panic for anything else.
    fn reason(r: Result<impl std::fmt::Debug>) -> String {
        match r {
            Err(EngineError::UnsupportedModel { reason }) => reason,
            other => panic!("expected UnsupportedModel, got {other:?}"),
        }
    }

    #[test]
    fn valid_signature_ok() {
        validate_signature(&webui_inputs(), &webui_outputs()).unwrap();
    }

    #[test]
    fn fp16_rejected() {
        let mut inputs = webui_inputs();
        inputs[0].elem = ElemType::F16;
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "fp16 export"
        );
        let mut outputs = webui_outputs();
        outputs[0].elem = ElemType::F16;
        assert_eq!(
            reason(validate_signature(&webui_inputs(), &outputs)),
            "fp16 export"
        );
    }

    #[test]
    fn v1_rejected() {
        let mut inputs = webui_inputs();
        inputs[0].dims = vec![1, -1, 256];
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "RVC v1 (256-dim)"
        );
        inputs[0].dims = vec![1, -1, 512];
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "phone has 512 features, expected 768"
        );
    }

    #[test]
    fn no_pitch_rejected() {
        let inputs: Vec<IoSpec> = webui_inputs()
            .into_iter()
            .filter(|i| !i.name.starts_with("pitch"))
            .collect();
        assert!(reason(validate_signature(&inputs, &webui_outputs())).contains("no F0 inputs"));
    }

    #[test]
    fn missing_phone_rejected() {
        let inputs: Vec<IoSpec> = webui_inputs()
            .into_iter()
            .filter(|i| i.name != "phone")
            .collect();
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "not an RVC WebUI ONNX export (missing input `phone`)"
        );
    }

    #[test]
    fn missing_rnd_rejected() {
        let inputs: Vec<IoSpec> = webui_inputs()
            .into_iter()
            .filter(|i| i.name != "rnd")
            .collect();
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "missing input `rnd`"
        );
        assert_eq!(
            reason(validate_signature(&webui_inputs(), &[])),
            "missing output `audio`"
        );
    }

    #[test]
    fn wrong_rank_rejected() {
        let mut inputs = webui_inputs();
        inputs[3].dims = vec![-1];
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "`pitchf` is F32 rank 1, expected F32 rank 2"
        );
        let mut inputs = webui_inputs();
        inputs[4].elem = ElemType::Other;
        assert_eq!(
            reason(validate_signature(&inputs, &webui_outputs())),
            "`ds` is Other rank 1, expected I64 rank 1"
        );
    }

    #[test]
    fn sr_override_wins() {
        assert_eq!(resolve_sr(Some("40000"), Some(48000)).unwrap(), 48000);
    }

    #[test]
    fn sr_from_metadata() {
        assert_eq!(resolve_sr(Some("32000"), None).unwrap(), 32000);
    }

    #[test]
    fn sr_missing_names_flag() {
        assert!(reason(resolve_sr(None, None)).contains("--sr"));
        assert!(reason(resolve_sr(Some("forty"), None)).contains("--sr"));
    }

    #[test]
    fn sr_44100_rejected() {
        assert_eq!(
            reason(resolve_sr(None, Some(44100))),
            "unsupported sample rate 44100 (expected 32000, 40000 or 48000)"
        );
    }

    #[test]
    fn box_muller_mean_and_var() {
        let mut rng = StdRng::seed_from_u64(0);
        let x = standard_normal(&mut rng, 10_000);
        assert_eq!(x.len(), 10_000);
        assert!(x.iter().all(|v| v.is_finite()));
        let n = x.len() as f64;
        let mean = x.iter().map(|&v| v as f64).sum::<f64>() / n;
        let var = x.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / n;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((var - 1.0).abs() < 0.05, "variance {var}");
    }

    fn tiny(phone_dim: i64, float: i64, pitch: bool, metadata: &[(&str, &str)]) -> Vec<u8> {
        onnx::model(
            &rvc_inputs(phone_dim, float, pitch),
            &rvc_output(float),
            "rnd",
            metadata,
        )
    }

    #[test]
    fn tiny_valid_loads() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let model = tiny(768, F32, true, &[("sr", "40000")]);
        let g = Generator::load_from_memory(&model, Provider::Cpu, None).unwrap();
        assert_eq!(g.sample_rate(), 40000);
        assert_eq!(g.active_provider(), Provider::Cpu);
    }

    #[test]
    fn tiny_run_feeds_every_input() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        // The tiny graph returns `rnd`, so the output is the [1,192,T] noise.
        let model = tiny(768, F32, true, &[("sr", "40000")]);
        let mut g = Generator::load_from_memory(&model, Provider::Cpu, None).unwrap();
        let frames = 4;
        let phone = vec![0.0; frames * DIM];
        let input = GenInput {
            phone: &phone,
            frames,
            pitch: &[1; 4],
            pitchf: &[0.0; 4],
            speaker_id: 0,
        };
        let audio = g.run(&input).unwrap();
        assert_eq!(audio.len(), NOISE_CHANNELS * frames);
        assert_eq!(
            audio,
            standard_normal(&mut StdRng::seed_from_u64(0), audio.len())
        );
    }

    #[test]
    fn tiny_fp16_rejected() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let model = tiny(768, F16, true, &[("sr", "40000")]);
        let r = Generator::load_from_memory(&model, Provider::Cpu, None).map(|_| ());
        assert_eq!(reason(r), "fp16 export");
    }

    #[test]
    fn tiny_v1_rejected() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let model = tiny(256, F32, true, &[("sr", "40000")]);
        let r = Generator::load_from_memory(&model, Provider::Cpu, None).map(|_| ());
        assert_eq!(reason(r), "RVC v1 (256-dim)");
    }

    #[test]
    fn tiny_no_pitch_rejected() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let model = tiny(768, F32, false, &[("sr", "40000")]);
        let r = Generator::load_from_memory(&model, Provider::Cpu, None).map(|_| ());
        assert!(reason(r).contains("no F0 inputs"));
    }

    #[test]
    fn tiny_without_sr_needs_flag() {
        if !assets_or_skip(&[ORT_DLL]) {
            return;
        }
        let model = tiny(768, F32, true, &[]);
        let r = Generator::load_from_memory(&model, Provider::Cpu, None).map(|_| ());
        assert!(reason(r).contains("--sr"));
        let g = Generator::load_from_memory(&model, Provider::Cpu, Some(48000)).unwrap();
        assert_eq!(g.sample_rate(), 48000);
    }
}
