//! The weight-free ONNX templates embedded in the binary: matching a
//! checkpoint's config, checking its weights' shapes, filling the weights in
//! and writing the conversion metadata.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::checkpoint::{ArchConfig, Checkpoint};
use crate::onnx::{self, FLOAT, ModelProto, TensorProto};
use crate::pickle::TensorRef;
use crate::{CONVERTER, Cancel, ConvertError};

/// The template initializer whose first dim is the checkpoint's speaker count.
const EMB_G: &str = "emb_g.weight";

/// The embedded template files, by the name `templates.toml` gives them.
const FILES: [(&str, &[u8]); 3] = [
    (
        "rvc-v2-f0-32k.onnx",
        include_bytes!("../../../assets/templates/rvc-v2-f0-32k.onnx"),
    ),
    (
        "rvc-v2-f0-40k.onnx",
        include_bytes!("../../../assets/templates/rvc-v2-f0-40k.onnx"),
    ),
    (
        "rvc-v2-f0-48k.onnx",
        include_bytes!("../../../assets/templates/rvc-v2-f0-48k.onnx"),
    ),
];

/// One `[[template]]` entry of `assets/templates/templates.toml`.
#[derive(Clone, Debug, Deserialize)]
pub struct TemplateInfo {
    pub file: String,
    pub sha256: String,
    pub rvc_commit: String,
    pub torch: String,
    pub config: ArchConfig,
}

/// A weight-free ONNX generator and the config it was exported for.
pub struct Template {
    pub info: TemplateInfo,
    pub bytes: &'static [u8],
}

#[derive(Deserialize)]
struct Manifest {
    template: Vec<TemplateInfo>,
}

/// The templates embedded in the binary, in manifest order.
pub fn templates() -> &'static [Template] {
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let manifest: Manifest =
            toml::from_str(include_str!("../../../assets/templates/templates.toml"))
                .expect("templates.toml is valid");
        manifest
            .template
            .into_iter()
            .map(|info| {
                let bytes = FILES
                    .iter()
                    .find(|(file, _)| *file == info.file)
                    .unwrap_or_else(|| panic!("template {} is not embedded", info.file))
                    .1;
                Template { info, bytes }
            })
            .collect()
    })
}

/// The template exported for exactly `cfg`.
pub fn match_template<'a>(
    templates: &'a [Template],
    cfg: &ArchConfig,
) -> Result<&'a Template, ConvertError> {
    templates
        .iter()
        .find(|t| t.info.config == *cfg)
        .ok_or_else(|| {
            tracing::warn!(?cfg, "no template matches the model's config");
            ConvertError::NoTemplate(format!("{cfg:?}"))
        })
}

/// A FLOAT initializer whose weights were removed at export, keeping its dims.
pub fn is_stripped(t: &TensorProto) -> bool {
    t.data_type == Some(FLOAT)
        && t.dims.iter().product::<i64>() > 0
        && t.raw_data.as_ref().is_none_or(Vec::is_empty)
        && t.float_data.is_empty()
}

/// A tensor shape as ONNX dims, `None` if a dim doesn't fit in `i64`.
fn dims(shape: &[usize]) -> Option<Vec<i64>> {
    shape.iter().map(|&d| i64::try_from(d).ok()).collect()
}

fn stripped(model: &ModelProto) -> impl Iterator<Item = &TensorProto> {
    model
        .graph
        .iter()
        .flat_map(|g| &g.initializer)
        .filter(|t| is_stripped(t))
}

/// Every stripped initializer has a weight of the same name and shape.
/// `emb_g.weight` may have any number of speakers (dim 0), but the same
/// embedding size (dim 1). Weights the template doesn't use are logged.
pub fn check_shapes(
    model: &ModelProto,
    weights: &BTreeMap<String, TensorRef>,
) -> Result<(), ConvertError> {
    let mut used = 0;
    for init in stripped(model) {
        let name = init.name.as_deref().unwrap_or_default();
        let weight = weights
            .get(name)
            .ok_or_else(|| ConvertError::NoTemplate(format!("{name}: missing from the model")))?;
        let shape = dims(&weight.shape).unwrap_or_default();
        let fits = if name == EMB_G {
            matches!((shape.as_slice(), init.dims.as_slice()),
                ([speakers, dim], [_, template_dim]) if *speakers >= 1 && dim == template_dim)
        } else {
            shape == init.dims
        };
        if !fits {
            return Err(ConvertError::NoTemplate(format!(
                "{name}: shape {:?}, template {:?}",
                weight.shape, init.dims
            )));
        }
        used += 1;
    }
    if used < weights.len() {
        let names: Vec<&str> = stripped(model).filter_map(|t| t.name.as_deref()).collect();
        let extra: Vec<&String> = weights
            .keys()
            .filter(|k| !names.contains(&k.as_str()))
            .collect();
        tracing::info!(?extra, "weights the template doesn't use");
    }
    Ok(())
}

/// Writes each stripped initializer's weight as fp32 raw data, checking
/// `cancel` before each load and reporting the fraction done after it.
pub fn fill(
    model: &mut ModelProto,
    weights: &BTreeMap<String, TensorRef>,
    load: &mut dyn FnMut(&TensorRef) -> Result<Vec<f32>, ConvertError>,
    cancel: &Cancel,
    progress: &mut dyn FnMut(f32),
) -> Result<(), ConvertError> {
    let Some(graph) = model.graph.as_mut() else {
        return Ok(());
    };
    let targets: Vec<&mut TensorProto> = graph
        .initializer
        .iter_mut()
        .filter(|t| is_stripped(t))
        .collect();
    let n = targets.len();
    for (i, init) in targets.into_iter().enumerate() {
        cancel.check()?;
        let name = init.name.as_deref().unwrap_or_default();
        let weight = weights
            .get(name)
            .ok_or_else(|| ConvertError::NoTemplate(format!("{name}: missing from the model")))?;
        let values = load(weight)?;
        init.dims = dims(&weight.shape)
            .ok_or_else(|| ConvertError::NoTemplate(format!("{name}: shape too large")))?;
        init.data_type = Some(FLOAT);
        init.raw_data = Some(values.iter().flat_map(|v| v.to_le_bytes()).collect());
        init.float_data.clear();
        progress((i + 1) as f32 / n as f32);
    }
    Ok(())
}

/// Records where the model came from: `sr`, `f0`, `version`, `speakers`,
/// `source_sha256` and `converter`.
pub fn write_metadata(model: &mut ModelProto, ckpt: &Checkpoint, sha256: &str) {
    onnx::set_metadata(model, "sr", &ckpt.sample_rate.to_string());
    onnx::set_metadata(model, "f0", if ckpt.f0 { "1" } else { "0" });
    onnx::set_metadata(model, "version", &ckpt.version);
    onnx::set_metadata(model, "speakers", &ckpt.speakers.to_string());
    onnx::set_metadata(model, "source_sha256", sha256);
    onnx::set_metadata(model, "converter", CONVERTER);
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use sha2::{Digest, Sha256};

    use super::*;
    use crate::pth::PthArchive;
    use crate::testutil::{Fixture, rvc_pth, toy_template};

    fn open(f: &Fixture) -> (PthArchive, Checkpoint) {
        let archive = PthArchive::open(rvc_pth(f), "voice.pth").expect("open fixture");
        let ckpt = Checkpoint::from_value(&archive.root, &archive.name).expect("classify");
        (archive, ckpt)
    }

    fn toy() -> (Template, PthArchive, Checkpoint) {
        let (archive, ckpt) = open(&Fixture::v2_40k());
        (toy_template(ckpt.config.clone()), archive, ckpt)
    }

    fn model(t: &Template) -> ModelProto {
        onnx::decode(t.bytes).expect("decode template")
    }

    fn initializer<'a>(m: &'a ModelProto, name: &str) -> &'a TensorProto {
        m.graph
            .as_ref()
            .expect("graph")
            .initializer
            .iter()
            .find(|t| t.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no initializer {name}"))
    }

    fn stripped_names(m: &ModelProto) -> Vec<String> {
        m.graph
            .as_ref()
            .expect("graph")
            .initializer
            .iter()
            .filter(|t| is_stripped(t))
            .filter_map(|t| t.name.clone())
            .collect()
    }

    fn assert_no_template(r: Result<(), ConvertError>, needle: &str) {
        match r {
            Err(ConvertError::NoTemplate(detail)) => {
                assert!(detail.contains(needle), "{detail:?} lacks {needle:?}");
            }
            other => panic!("expected NoTemplate, got {other:?}"),
        }
    }

    fn fill_toy(
        m: &mut ModelProto,
        weights: &BTreeMap<String, TensorRef>,
        archive: &mut PthArchive,
    ) -> Result<(), ConvertError> {
        fill(
            m,
            weights,
            &mut |t| archive.tensor_f32(t),
            &Cancel::new(),
            &mut |_| {},
        )
    }

    #[test]
    fn embedded_templates_match_manifest() {
        let all = templates();
        assert_eq!(all.len(), 3);
        let rates: BTreeSet<u32> = all.iter().map(|t| t.info.config.sr).collect();
        assert_eq!(rates, BTreeSet::from([32000, 40000, 48000]));
        for t in all {
            let digest = format!("{:x}", Sha256::digest(t.bytes));
            assert_eq!(digest, t.info.sha256, "{}", t.info.file);
            onnx::decode(t.bytes).unwrap_or_else(|e| panic!("{}: {e}", t.info.file));
        }
    }

    #[test]
    fn embedded_templates_are_weight_free() {
        for t in templates() {
            let m = model(t);
            let names = stripped_names(&m);
            assert!(
                names.iter().any(|n| n == "emb_g.weight"),
                "{}: emb_g.weight not stripped",
                t.info.file
            );
            for name in &names {
                let i = initializer(&m, name);
                assert_eq!(i.data_type, Some(FLOAT), "{name}");
                assert!(i.raw_data.as_ref().is_none_or(Vec::is_empty), "{name}");
                assert!(i.float_data.is_empty(), "{name}");
            }
            // What is left is small constants, not the weights (tens of MB).
            assert!(
                t.bytes.len() < 2 << 20,
                "{}: {} bytes",
                t.info.file,
                t.bytes.len()
            );
        }
    }

    #[test]
    fn match_each_manifest_config() {
        let all = templates();
        for t in all {
            let found = match_template(all, &t.info.config).expect("match");
            assert_eq!(found.info.file, t.info.file);
        }
    }

    #[test]
    fn match_ignores_nothing_else() {
        let all = templates();
        let base = all[0].info.config.clone();

        let mut cfg = base.clone();
        cfg.upsample_rates[0] += 1;
        assert!(matches!(
            match_template(all, &cfg),
            Err(ConvertError::NoTemplate(_))
        ));

        let mut cfg = base;
        cfg.gin_channels = 128;
        assert!(matches!(
            match_template(all, &cfg),
            Err(ConvertError::NoTemplate(_))
        ));
    }

    #[test]
    fn fill_writes_fp32_raw_data() {
        let (t, mut archive, ckpt) = toy();
        let mut m = model(&t);
        let before_c = initializer(&m, "c").clone();
        fill_toy(&mut m, &ckpt.weights, &mut archive).expect("fill");

        for name in [
            "enc_p.emb_phone.weight",
            "emb_g.weight",
            "dec.conv_pre.weight",
        ] {
            let i = initializer(&m, name);
            let expected = archive.tensor_f32(&ckpt.weights[name]).expect("load");
            let bytes: Vec<u8> = expected.iter().flat_map(|v| v.to_le_bytes()).collect();
            assert_eq!(i.data_type, Some(FLOAT), "{name}");
            assert_eq!(i.raw_data.as_deref(), Some(bytes.as_slice()), "{name}");
            assert!(i.float_data.is_empty(), "{name}");
            assert!(!is_stripped(i), "{name}");
        }
        assert_eq!(initializer(&m, "c"), &before_c);
    }

    #[test]
    fn fill_sets_emb_g_speakers() {
        let (t, mut archive, ckpt) = toy();
        let mut m = model(&t);
        assert_eq!(initializer(&m, "emb_g.weight").dims, [109, 256]);
        fill_toy(&mut m, &ckpt.weights, &mut archive).expect("fill");
        assert_eq!(initializer(&m, "emb_g.weight").dims, [3, 256]);
    }

    #[test]
    fn check_shapes_accepts_fixture() {
        let (t, _archive, ckpt) = toy();
        check_shapes(&model(&t), &ckpt.weights).expect("shapes match");
    }

    #[test]
    fn check_shapes_missing_tensor() {
        let mut f = Fixture::v2_40k();
        f.weights.retain(|w| w.0 != "dec.conv_pre.weight");
        let (_archive, ckpt) = open(&f);
        let t = toy_template(ckpt.config.clone());
        assert_no_template(
            check_shapes(&model(&t), &ckpt.weights),
            "dec.conv_pre.weight",
        );
    }

    #[test]
    fn check_shapes_wrong_shape() {
        let mut f = Fixture::v2_40k();
        f.weights[2].1 = vec![512, 192, 5];
        let (_archive, ckpt) = open(&f);
        let t = toy_template(ckpt.config.clone());
        assert_no_template(
            check_shapes(&model(&t), &ckpt.weights),
            "dec.conv_pre.weight",
        );
    }

    #[test]
    fn check_shapes_emb_g_wrong_gin() {
        let mut f = Fixture::v2_40k();
        f.weights[1].1 = vec![3, 128];
        let (_archive, ckpt) = open(&f);
        let t = toy_template(ckpt.config.clone());
        assert_no_template(check_shapes(&model(&t), &ckpt.weights), "emb_g.weight");
    }

    #[test]
    fn check_shapes_emb_g_wrong_rank() {
        let mut f = Fixture::v2_40k();
        f.weights[1].1 = vec![3, 256, 1];
        let (_archive, ckpt) = open(&f);
        let t = toy_template(ckpt.config.clone());
        assert_no_template(check_shapes(&model(&t), &ckpt.weights), "emb_g.weight");
    }

    #[test]
    fn extra_keys_are_ignored() {
        let (t, mut archive, ckpt) = toy();
        assert!(ckpt.weights.keys().any(|k| k.starts_with("enc_q.")));
        let mut m = model(&t);
        check_shapes(&m, &ckpt.weights).expect("extra weights are fine");
        fill_toy(&mut m, &ckpt.weights, &mut archive).expect("fill");
        assert!(
            m.graph
                .as_ref()
                .expect("graph")
                .initializer
                .iter()
                .all(|i| !i.name.as_deref().unwrap_or("").starts_with("enc_q."))
        );
    }

    #[test]
    fn fill_honours_cancel() {
        let (t, _archive, ckpt) = toy();
        let mut m = model(&t);
        let cancel = Cancel::new();
        cancel.cancel();
        let mut loads = 0;
        let r = fill(
            &mut m,
            &ckpt.weights,
            &mut |_| {
                loads += 1;
                Ok(Vec::new())
            },
            &cancel,
            &mut |_| {},
        );
        assert!(matches!(r, Err(ConvertError::Cancelled)), "got {r:?}");
        assert_eq!(loads, 0);
    }

    #[test]
    fn fill_reports_progress() {
        let (t, mut archive, ckpt) = toy();
        let mut m = model(&t);
        let mut fractions = Vec::new();
        fill(
            &mut m,
            &ckpt.weights,
            &mut |t| archive.tensor_f32(t),
            &Cancel::new(),
            &mut |f| fractions.push(f),
        )
        .expect("fill");
        assert_eq!(fractions.len(), 3);
        assert!(fractions.windows(2).all(|w| w[0] < w[1]), "{fractions:?}");
        assert!(fractions[0] > 0.0);
        assert_eq!(fractions.last(), Some(&1.0));
    }

    #[test]
    fn write_metadata_keys() {
        let (t, _archive, ckpt) = toy();
        let mut m = model(&t);
        let sha = "ab".repeat(32);
        write_metadata(&mut m, &ckpt, &sha);
        let m = onnx::decode(&onnx::encode(&m)).expect("round trip");
        assert_eq!(onnx::metadata(&m, "sr"), Some("40000"));
        assert_eq!(onnx::metadata(&m, "f0"), Some("1"));
        assert_eq!(onnx::metadata(&m, "version"), Some("v2"));
        assert_eq!(onnx::metadata(&m, "speakers"), Some("3"));
        assert_eq!(onnx::metadata(&m, "source_sha256"), Some(sha.as_str()));
        assert_eq!(onnx::metadata(&m, "converter"), Some(CONVERTER));
        assert_eq!(m.metadata_props.len(), 6);
    }

    #[test]
    fn is_stripped_rules() {
        let stripped = TensorProto {
            dims: vec![2, 3],
            data_type: Some(FLOAT),
            ..Default::default()
        };
        assert!(is_stripped(&stripped));
        let with_raw = TensorProto {
            raw_data: Some(vec![0; 24]),
            ..stripped.clone()
        };
        assert!(!is_stripped(&with_raw));
        let with_floats = TensorProto {
            float_data: vec![0.0; 6],
            ..stripped.clone()
        };
        assert!(!is_stripped(&with_floats));
        let empty = TensorProto {
            dims: vec![0, 3],
            ..stripped
        };
        assert!(!is_stripped(&empty));
    }
}
