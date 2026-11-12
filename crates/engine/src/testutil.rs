//! Test helpers: vendor asset gating and temporary paths.

use std::path::PathBuf;

use crate::assets;

/// True when every asset resolves. Otherwise panics if `VC_REQUIRE_ASSETS=1`,
/// or prints a skip notice and returns false.
pub fn assets_or_skip(rel: &[&str]) -> bool {
    let missing: Vec<&str> = rel
        .iter()
        .copied()
        .filter(|r| assets::asset(r).is_err())
        .collect();
    if missing.is_empty() {
        return true;
    }
    if std::env::var("VC_REQUIRE_ASSETS").as_deref() == Ok("1") {
        panic!(
            "missing {}: run `cargo xtask fetch-models`",
            missing.join(", ")
        );
    }
    println!("skipping: missing {}", missing.join(", "));
    false
}

/// A per-process path in the system temp directory.
pub fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("vc-engine-test-{}-{name}", std::process::id()))
}

pub mod onnx {
    //! Minimal ONNX protobuf writer: one Identity node, typed inputs and outputs, metadata.

    fn varint(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                break;
            }
            out.push(b | 0x80);
        }
    }

    fn int_field(field: u64, v: i64, out: &mut Vec<u8>) {
        varint(field << 3, out);
        varint(v as u64, out);
    }

    fn bytes_field(field: u64, b: &[u8], out: &mut Vec<u8>) {
        varint((field << 3) | 2, out);
        varint(b.len() as u64, out);
        out.extend_from_slice(b);
    }

    pub const F32: i64 = 1;
    pub const I64: i64 = 7;
    pub const F16: i64 = 10;

    pub enum Dim {
        Fixed(i64),
        Sym(&'static str),
    }

    pub struct Io {
        pub name: &'static str,
        pub elem: i64,
        pub dims: Vec<Dim>,
    }

    fn value_info(io: &Io) -> Vec<u8> {
        let mut shape = Vec::new();
        for d in &io.dims {
            let mut dim = Vec::new();
            match d {
                Dim::Fixed(v) => int_field(1, *v, &mut dim),
                Dim::Sym(s) => bytes_field(2, s.as_bytes(), &mut dim),
            }
            bytes_field(1, &dim, &mut shape);
        }
        let mut tensor = Vec::new();
        int_field(1, io.elem, &mut tensor);
        bytes_field(2, &shape, &mut tensor);
        let mut ty = Vec::new();
        bytes_field(1, &tensor, &mut ty);
        let mut vi = Vec::new();
        bytes_field(1, io.name.as_bytes(), &mut vi);
        bytes_field(2, &ty, &mut vi);
        vi
    }

    /// ModelProto (IR 8, opset 17) whose output is `Identity(identity_from)`.
    pub fn model(
        inputs: &[Io],
        output: &Io,
        identity_from: &str,
        metadata: &[(&str, &str)],
    ) -> Vec<u8> {
        let mut node = Vec::new();
        bytes_field(1, identity_from.as_bytes(), &mut node);
        bytes_field(2, output.name.as_bytes(), &mut node);
        bytes_field(4, b"Identity", &mut node);
        let mut graph = Vec::new();
        bytes_field(1, &node, &mut graph);
        bytes_field(2, b"test", &mut graph);
        for i in inputs {
            bytes_field(11, &value_info(i), &mut graph);
        }
        bytes_field(12, &value_info(output), &mut graph);
        let mut opset = Vec::new();
        bytes_field(1, b"", &mut opset);
        int_field(2, 17, &mut opset);
        let mut m = Vec::new();
        int_field(1, 8, &mut m);
        bytes_field(7, &graph, &mut m);
        bytes_field(8, &opset, &mut m);
        for (k, v) in metadata {
            let mut e = Vec::new();
            bytes_field(1, k.as_bytes(), &mut e);
            bytes_field(2, v.as_bytes(), &mut e);
            bytes_field(14, &e, &mut m);
        }
        m
    }

    /// WebUI-shaped generator inputs. `phone_dim` = 768 (v2) or 256 (v1); `pitch` = false drops pitch/pitchf.
    pub fn rvc_inputs(phone_dim: i64, float: i64, pitch: bool) -> Vec<Io> {
        use Dim::{Fixed, Sym};
        let mut inputs = vec![
            Io {
                name: "phone",
                elem: float,
                dims: vec![Fixed(1), Sym("T"), Fixed(phone_dim)],
            },
            Io {
                name: "phone_lengths",
                elem: I64,
                dims: vec![Fixed(1)],
            },
        ];
        if pitch {
            inputs.push(Io {
                name: "pitch",
                elem: I64,
                dims: vec![Fixed(1), Sym("T")],
            });
            inputs.push(Io {
                name: "pitchf",
                elem: float,
                dims: vec![Fixed(1), Sym("T")],
            });
        }
        inputs.push(Io {
            name: "ds",
            elem: I64,
            dims: vec![Fixed(1)],
        });
        inputs.push(Io {
            name: "rnd",
            elem: float,
            dims: vec![Fixed(1), Fixed(192), Sym("T")],
        });
        inputs
    }

    /// The WebUI `audio [1,1,N]` output in `float`.
    pub fn rvc_output(float: i64) -> Io {
        Io {
            name: "audio",
            elem: float,
            dims: vec![Dim::Fixed(1), Dim::Fixed(1), Dim::Sym("N")],
        }
    }
}
