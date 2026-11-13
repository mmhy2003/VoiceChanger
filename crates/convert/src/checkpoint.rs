//! An RVC inference checkpoint: its architecture config, its weights and
//! whether the converter can handle it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ConvertError;
use crate::pickle::{DType, TensorRef, Value};
use crate::pth::is_training_name;

/// Length of the v2 `config` list.
const CONFIG_LEN: usize = 18;
/// Sample rates the templates cover.
const SAMPLE_RATES: [u32; 3] = [32000, 40000, 48000];

/// The generator's architecture: the `config` list without the training-only
/// fields (segment_size, p_dropout) and spk_embed_dim, which comes from
/// `emb_g.weight`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ArchConfig {
    pub spec_channels: u32,
    pub inter_channels: u32,
    pub hidden_channels: u32,
    pub filter_channels: u32,
    pub n_heads: u32,
    pub n_layers: u32,
    pub kernel_size: u32,
    pub resblock: String,
    pub resblock_kernel_sizes: Vec<u32>,
    pub resblock_dilation_sizes: Vec<Vec<u32>>,
    pub upsample_rates: Vec<u32>,
    pub upsample_initial_channel: u32,
    pub upsample_kernel_sizes: Vec<u32>,
    pub gin_channels: u32,
    pub sr: u32,
}

impl ArchConfig {
    /// Reads RVC's `config` list (18 items in v2).
    pub fn from_values(list: &[Value]) -> Result<Self, ConvertError> {
        let [
            spec_channels,
            _segment_size,
            inter_channels,
            hidden_channels,
            filter_channels,
            n_heads,
            n_layers,
            kernel_size,
            _p_dropout,
            resblock,
            resblock_kernel_sizes,
            resblock_dilation_sizes,
            upsample_rates,
            upsample_initial_channel,
            upsample_kernel_sizes,
            _spk_embed_dim,
            gin_channels,
            sr,
        ] = list
        else {
            return Err(config_error(format!(
                "{} items, expected {CONFIG_LEN}",
                list.len()
            )));
        };
        let resblock = match resblock {
            Value::Str(s) => s.clone(),
            Value::Int(i) => i.to_string(),
            other => return Err(config_error(format!("resblock {other:?}"))),
        };
        Ok(Self {
            spec_channels: uint(spec_channels, "spec_channels")?,
            inter_channels: uint(inter_channels, "inter_channels")?,
            hidden_channels: uint(hidden_channels, "hidden_channels")?,
            filter_channels: uint(filter_channels, "filter_channels")?,
            n_heads: uint(n_heads, "n_heads")?,
            n_layers: uint(n_layers, "n_layers")?,
            kernel_size: uint(kernel_size, "kernel_size")?,
            resblock,
            resblock_kernel_sizes: uints(resblock_kernel_sizes, "resblock_kernel_sizes")?,
            resblock_dilation_sizes: resblock_dilation_sizes
                .as_list()
                .ok_or_else(|| config_error("resblock_dilation_sizes is not a list"))?
                .iter()
                .map(|v| uints(v, "resblock_dilation_sizes"))
                .collect::<Result<_, _>>()?,
            upsample_rates: uints(upsample_rates, "upsample_rates")?,
            upsample_initial_channel: uint(upsample_initial_channel, "upsample_initial_channel")?,
            upsample_kernel_sizes: uints(upsample_kernel_sizes, "upsample_kernel_sizes")?,
            gin_channels: uint(gin_channels, "gin_channels")?,
            sr: uint(sr, "sr")?,
        })
    }

    /// Samples per feature frame: the product of the upsample rates.
    pub fn hop(&self) -> u32 {
        self.upsample_rates.iter().product()
    }
}

fn config_error(detail: impl std::fmt::Display) -> ConvertError {
    ConvertError::NoTemplate(format!("config: {detail}"))
}

fn uint(v: &Value, field: &str) -> Result<u32, ConvertError> {
    v.as_int()
        .and_then(|i| u32::try_from(i).ok())
        .ok_or_else(|| config_error(format!("{field} {v:?}")))
}

fn uints(v: &Value, field: &str) -> Result<Vec<u32>, ConvertError> {
    v.as_list()
        .ok_or_else(|| config_error(format!("{field} is not a list")))?
        .iter()
        .map(|i| uint(i, field))
        .collect()
}

/// An RVC v2 inference model with F0, ready to match against a template.
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    pub version: String,
    pub sample_rate: u32,
    pub f0: bool,
    pub speakers: u32,
    /// Any weight is stored as fp16 or bf16.
    pub fp16: bool,
    pub info: Option<String>,
    pub config: ArchConfig,
    pub weights: BTreeMap<String, TensorRef>,
}

impl Checkpoint {
    /// Reads and classifies the unpickled root of `pth_name`. Training
    /// checkpoints, v1 models, models without F0 and configs no template
    /// covers are errors.
    pub fn from_value(root: &Value, pth_name: &str) -> Result<Self, ConvertError> {
        if !matches!(root, Value::Dict(_)) {
            return Err(ConvertError::NoTemplate("not an RVC model".into()));
        }
        if is_training_name(pth_name)
            || ["model", "optimizer", "iteration"]
                .iter()
                .any(|k| root.get(k).is_some())
        {
            return Err(ConvertError::TrainingCheckpoint);
        }

        let weights = weights(root.get("weight"))
            .ok_or_else(|| ConvertError::NoTemplate("no weight dict".into()))?;

        let version = root.get("version").and_then(Value::as_str);
        let phone_dim = weights
            .get("enc_p.emb_phone.weight")
            .and_then(|t| t.shape.last());
        let version = match version {
            None | Some("v1") => return Err(ConvertError::RvcV1),
            Some(_) if phone_dim == Some(&256) => return Err(ConvertError::RvcV1),
            Some(v) => v.to_string(),
        };

        if root.get("f0").and_then(Value::as_int) == Some(0) {
            return Err(ConvertError::NoF0);
        }

        let list = root
            .get("config")
            .and_then(Value::as_list)
            .ok_or_else(|| config_error("missing or not a list"))?;
        let config = ArchConfig::from_values(list)?;
        if !SAMPLE_RATES.contains(&config.sr) {
            return Err(config_error(format!("sample rate {}", config.sr)));
        }

        let speakers = weights
            .get("emb_g.weight")
            .and_then(|t| t.shape.first())
            .and_then(|&n| u32::try_from(n).ok())
            .ok_or_else(|| ConvertError::NoTemplate("no emb_g.weight".into()))?;
        let fp16 = weights
            .values()
            .any(|t| matches!(t.dtype, DType::F16 | DType::BF16));

        Ok(Self {
            version,
            sample_rate: config.sr,
            f0: true,
            speakers,
            fp16,
            info: root.get("info").and_then(Value::as_str).map(str::to_string),
            config,
            weights,
        })
    }
}

/// The `weight` dict, when it maps string names to tensors only.
fn weights(value: Option<&Value>) -> Option<BTreeMap<String, TensorRef>> {
    let Some(Value::Dict(items)) = value else {
        return None;
    };
    items
        .iter()
        .map(|(k, v)| match (k, v) {
            (Value::Str(name), Value::Tensor(t)) => Some((name.clone(), t.clone())),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pth::PthArchive;
    use crate::testutil::{
        Fixture, P, config_32k, config_40k, config_48k, pickle, rvc_pth, torch_zip,
    };

    fn read(f: &Fixture, name: &str) -> Result<Checkpoint, ConvertError> {
        let archive = PthArchive::open(rvc_pth(f), name).expect("open fixture");
        Checkpoint::from_value(&archive.root, &archive.name)
    }

    fn read_ok(f: &Fixture) -> Checkpoint {
        match read(f, "voice.pth") {
            Ok(c) => c,
            Err(e) => panic!("classification failed: {e}"),
        }
    }

    fn read_err(f: &Fixture, name: &str) -> ConvertError {
        match read(f, name) {
            Ok(c) => panic!("classified as a model: {c:?}"),
            Err(e) => e,
        }
    }

    fn values(list: Vec<P>) -> Vec<Value> {
        match crate::pickle::load(&pickle(&P::List(list))).expect("load list") {
            Value::List(items) => items,
            other => panic!("not a list: {other:?}"),
        }
    }

    fn arch(list: Vec<P>) -> ArchConfig {
        ArchConfig::from_values(&values(list)).expect("config")
    }

    fn reference_40k() -> ArchConfig {
        ArchConfig {
            spec_channels: 1025,
            inter_channels: 192,
            hidden_channels: 192,
            filter_channels: 768,
            n_heads: 2,
            n_layers: 6,
            kernel_size: 3,
            resblock: "1".into(),
            resblock_kernel_sizes: vec![3, 7, 11],
            resblock_dilation_sizes: vec![vec![1, 3, 5], vec![1, 3, 5], vec![1, 3, 5]],
            upsample_rates: vec![10, 10, 2, 2],
            upsample_initial_channel: 512,
            upsample_kernel_sizes: vec![16, 16, 4, 4],
            gin_channels: 256,
            sr: 40000,
        }
    }

    fn assert_no_template(e: ConvertError) {
        assert!(matches!(e, ConvertError::NoTemplate(_)), "got {e:?}");
    }

    #[test]
    fn reads_v2_40k() {
        let c = read_ok(&Fixture::v2_40k());
        assert_eq!(c.version, "v2");
        assert_eq!(c.sample_rate, 40000);
        assert!(c.f0);
        assert_eq!(c.speakers, 3);
        assert!(c.fp16);
        assert_eq!(c.info.as_deref(), Some("300epoch"));
        assert_eq!(c.config, reference_40k());
        assert_eq!(c.weights.len(), 4);
        let emb = &c.weights["emb_g.weight"];
        assert_eq!(emb.shape, [3, 256]);
        assert_eq!(emb.dtype, DType::F16);
    }

    #[test]
    fn fp32_checkpoint_not_fp16() {
        let mut f = Fixture::v2_40k();
        for w in &mut f.weights {
            w.2 = DType::F32;
        }
        assert!(!read_ok(&f).fp16);
    }

    #[test]
    fn config_skips_training_fields() {
        let mut list = config_40k();
        list[1] = P::Int(64);
        list[8] = P::Float(0.1);
        list[15] = P::Int(5);
        assert_eq!(arch(list), reference_40k());
    }

    #[test]
    fn hop_is_product_of_upsample_rates() {
        assert_eq!(arch(config_40k()).hop(), 400);
        assert_eq!(arch(config_48k()).hop(), 480);
        assert_eq!(arch(config_32k()).hop(), 320);
    }

    #[test]
    fn v1_by_missing_version() {
        let mut f = Fixture::v2_40k();
        f.version = None;
        assert!(matches!(read_err(&f, "voice.pth"), ConvertError::RvcV1));
    }

    #[test]
    fn v1_by_version_string() {
        let mut f = Fixture::v2_40k();
        f.version = Some("v1");
        assert!(matches!(read_err(&f, "voice.pth"), ConvertError::RvcV1));
    }

    #[test]
    fn v1_by_256_dim() {
        let mut f = Fixture::v2_40k();
        f.weights[0].1 = vec![192, 256];
        assert!(matches!(read_err(&f, "voice.pth"), ConvertError::RvcV1));
    }

    #[test]
    fn training_by_name() {
        let mut f = Fixture::v2_40k();
        f.version = None;
        assert!(matches!(
            read_err(&f, "G_2333.pth"),
            ConvertError::TrainingCheckpoint
        ));
    }

    #[test]
    fn training_by_keys() {
        for key in ["model", "optimizer", "iteration"] {
            let mut f = Fixture::v2_40k();
            f.version = Some("v1");
            f.extra.push((key, P::Int(1)));
            assert!(
                matches!(read_err(&f, "voice.pth"), ConvertError::TrainingCheckpoint),
                "key {key}"
            );
        }
    }

    #[test]
    fn no_f0() {
        let mut f = Fixture::v2_40k();
        f.f0 = Some(0);
        assert!(matches!(read_err(&f, "voice.pth"), ConvertError::NoF0));
        f.f0 = None;
        assert!(read_ok(&f).f0);
    }

    #[test]
    fn bad_config_length() {
        let mut f = Fixture::v2_40k();
        f.config.pop();
        assert_no_template(read_err(&f, "voice.pth"));
    }

    #[test]
    fn unsupported_sr() {
        let mut f = Fixture::v2_40k();
        f.config[17] = P::Int(44100);
        assert_no_template(read_err(&f, "voice.pth"));
    }

    #[test]
    fn missing_emb_g() {
        let mut f = Fixture::v2_40k();
        f.weights.retain(|w| w.0 != "emb_g.weight");
        assert_no_template(read_err(&f, "voice.pth"));
    }

    #[test]
    fn root_not_dict() {
        let pkl = pickle(&P::List(vec![P::Int(1)]));
        let archive =
            PthArchive::open(torch_zip("archive", &pkl, &[]), "voice.pth").expect("open archive");
        assert_no_template(
            Checkpoint::from_value(&archive.root, &archive.name).expect_err("not a model"),
        );
    }

    #[test]
    fn resblock_int_becomes_string() {
        let mut list = config_40k();
        list[9] = P::Int(2);
        assert_eq!(arch(list).resblock, "2");
    }
}
