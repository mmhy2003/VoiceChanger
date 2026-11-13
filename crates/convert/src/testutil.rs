//! Test helpers: a pickle writer that emits protocol 2 as `torch.save` does,
//! and a temporary folder removed on drop.

use std::path::{Path, PathBuf};

use crate::checkpoint::ArchConfig;
use crate::onnx::{FLOAT, GraphProto, ModelProto, TensorProto};
use crate::pickle::DType;
use crate::template::{Template, TemplateInfo};

/// A value to pickle. `Global` emits a bare GLOBAL and `Raw` is copied as is,
/// so tests can build input the writer would never produce.
#[derive(Clone, Debug)]
pub enum P {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<P>),
    Tuple(Vec<P>),
    Dict(Vec<(P, P)>),
    OrderedDict(Vec<(P, P)>),
    Tensor {
        key: String,
        dtype: DType,
        numel: usize,
        offset: usize,
        shape: Vec<usize>,
        stride: Vec<usize>,
    },
    Global(String, String),
    Raw(Vec<u8>),
}

/// `p` as a protocol 2 pickle: PROTO 2, the value, STOP.
pub fn pickle(p: &P) -> Vec<u8> {
    let mut w = Writer {
        out: vec![0x80, 2],
        memo: 0,
    };
    w.write(p);
    w.out.push(b'.');
    w.out
}

/// Contiguous (row-major) strides for `shape`, as torch computes them.
pub fn contiguous(shape: &[usize]) -> Vec<usize> {
    let mut stride = vec![0; shape.len()];
    let mut next = 1;
    for (s, &d) in stride.iter_mut().zip(shape).rev() {
        *s = next;
        next *= d.max(1);
    }
    stride
}

struct Writer {
    out: Vec<u8>,
    memo: u32,
}

impl Writer {
    fn write(&mut self, p: &P) {
        match p {
            P::None => self.out.push(b'N'),
            P::Bool(b) => self.out.push(if *b { 0x88 } else { 0x89 }),
            P::Int(i) => self.int(*i),
            P::Float(f) => {
                self.out.push(b'G');
                self.out.extend(f.to_be_bytes());
            }
            P::Str(s) => self.str(s),
            P::Bytes(b) => {
                self.out.push(b'B');
                self.out.extend(len32(b.len()));
                self.out.extend(b);
            }
            P::List(items) => {
                self.out.push(b']');
                self.put();
                if !items.is_empty() {
                    self.out.push(b'(');
                    items.iter().for_each(|i| self.write(i));
                    self.out.push(b'e');
                }
            }
            P::Tuple(items) => {
                if items.is_empty() {
                    self.out.push(b')');
                    return;
                }
                if items.len() > 3 {
                    self.out.push(b'(');
                }
                items.iter().for_each(|i| self.write(i));
                self.out.push(match items.len() {
                    1 => 0x85,
                    2 => 0x86,
                    3 => 0x87,
                    _ => b't',
                });
                self.put();
            }
            P::Dict(pairs) => {
                self.out.push(b'}');
                self.put();
                self.set_items(pairs);
            }
            P::OrderedDict(pairs) => {
                self.global("collections", "OrderedDict");
                self.out.extend(b")R");
                self.put();
                self.set_items(pairs);
            }
            P::Tensor {
                key,
                dtype,
                numel,
                offset,
                shape,
                stride,
            } => {
                self.global("torch._utils", "_rebuild_tensor_v2");
                self.out.push(b'(');
                self.out.push(b'(');
                self.str("storage");
                self.global("torch", dtype.storage_name());
                self.str(key);
                self.str("cpu");
                self.int(int(*numel));
                self.out.push(b't');
                self.put();
                self.out.push(b'Q');
                self.int(int(*offset));
                self.write(&P::Tuple(shape.iter().map(|&d| P::Int(int(d))).collect()));
                self.write(&P::Tuple(stride.iter().map(|&d| P::Int(int(d))).collect()));
                self.out.push(0x89);
                self.write(&P::OrderedDict(Vec::new()));
                self.out.push(b't');
                self.put();
                self.out.push(b'R');
            }
            P::Global(module, name) => self.global(module, name),
            P::Raw(bytes) => self.out.extend(bytes),
        }
    }

    fn set_items(&mut self, pairs: &[(P, P)]) {
        if !pairs.is_empty() {
            self.out.push(b'(');
            for (k, v) in pairs {
                self.write(k);
                self.write(v);
            }
            self.out.push(b'u');
        }
    }

    fn int(&mut self, i: i64) {
        if let Ok(b) = u8::try_from(i) {
            self.out.extend([b'K', b]);
        } else if let Ok(i) = i32::try_from(i) {
            self.out.push(b'J');
            self.out.extend(i.to_le_bytes());
        } else {
            self.out.extend([0x8a, 8]);
            self.out.extend(i.to_le_bytes());
        }
    }

    fn str(&mut self, s: &str) {
        self.out.push(b'X');
        self.out.extend(len32(s.len()));
        self.out.extend(s.as_bytes());
    }

    fn global(&mut self, module: &str, name: &str) {
        self.out.push(b'c');
        self.out.extend(format!("{module}\n{name}\n").as_bytes());
    }

    fn put(&mut self) {
        match u8::try_from(self.memo) {
            Ok(i) => self.out.extend([b'q', i]),
            Err(_) => {
                self.out.push(b'r');
                self.out.extend(self.memo.to_le_bytes());
            }
        }
        self.memo += 1;
    }
}

fn int(n: usize) -> i64 {
    i64::try_from(n).expect("fits in i64")
}

fn len32(n: usize) -> [u8; 4] {
    u32::try_from(n).expect("fits in u32").to_le_bytes()
}

/// `temp_dir()/vc-convert-test-{pid}-{name}`, created empty and removed on drop.
#[expect(dead_code, reason = "used by the .pth archive tests")]
pub struct TempDir(PathBuf);

#[expect(dead_code, reason = "used by the .pth archive tests")]
impl TempDir {
    pub fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("vc-convert-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A torch archive as `torch.save` writes it, with stored entries
/// `<prefix>/data.pkl`, `<prefix>/data/<key>` and `<prefix>/version`.
pub fn torch_zip(prefix: &str, pkl: &[u8], storages: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut files: Vec<(String, &[u8])> = vec![(format!("{prefix}/data.pkl"), pkl)];
    for (key, bytes) in storages {
        files.push((format!("{prefix}/data/{key}"), bytes));
    }
    files.push((format!("{prefix}/version"), b"3"));
    let files: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), *b)).collect();
    zip_of(&files)
}

/// A zip with stored (uncompressed) entries.
pub fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in files {
        zip.start_file(*name, options).expect("start zip entry");
        std::io::Write::write_all(&mut zip, bytes).expect("write zip entry");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// `values` as little-endian IEEE half floats.
pub fn f16_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|&v| half::f16::from_f32(v).to_le_bytes())
        .collect()
}

/// `values` as little-endian bfloat16.
pub fn bf16_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|&v| half::bf16::from_f32(v).to_le_bytes())
        .collect()
}

/// `values` as little-endian f32.
pub fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn ints(values: &[i64]) -> P {
    P::List(values.iter().map(|&v| P::Int(v)).collect())
}

/// An RVC config list (Appendix C order) with the given sample-rate fields.
fn config(spec: i64, rates: &[i64], kernels: &[i64], sr: i64) -> Vec<P> {
    vec![
        P::Int(spec),
        P::Int(32),
        P::Int(192),
        P::Int(192),
        P::Int(768),
        P::Int(2),
        P::Int(6),
        P::Int(3),
        P::Int(0),
        P::Str("1".into()),
        ints(&[3, 7, 11]),
        P::List(vec![ints(&[1, 3, 5]), ints(&[1, 3, 5]), ints(&[1, 3, 5])]),
        ints(rates),
        P::Int(512),
        ints(kernels),
        P::Int(109),
        P::Int(256),
        P::Int(sr),
    ]
}

/// The RVC v2 40k config list.
pub fn config_40k() -> Vec<P> {
    config(1025, &[10, 10, 2, 2], &[16, 16, 4, 4], 40000)
}

/// The RVC v2 48k config list.
pub fn config_48k() -> Vec<P> {
    config(1025, &[12, 10, 2, 2], &[24, 20, 4, 4], 48000)
}

/// The RVC v2 32k config list.
pub fn config_32k() -> Vec<P> {
    config(513, &[10, 8, 2, 2], &[20, 16, 4, 4], 32000)
}

/// The parts of an RVC inference checkpoint, for [`rvc_pth`].
/// `None` leaves the key out.
pub struct Fixture {
    pub version: Option<&'static str>,
    pub f0: Option<i64>,
    pub config: Vec<P>,
    /// Name, shape and dtype of each weight.
    pub weights: Vec<(String, Vec<usize>, DType)>,
    /// More top-level keys, written last.
    pub extra: Vec<(&'static str, P)>,
}

impl Fixture {
    /// A small fp16 v2 40k model with 3 speakers.
    pub fn v2_40k() -> Self {
        Self {
            version: Some("v2"),
            f0: Some(1),
            config: config_40k(),
            weights: vec![
                ("enc_p.emb_phone.weight".into(), vec![192, 768], DType::F16),
                ("emb_g.weight".into(), vec![3, 256], DType::F16),
                ("dec.conv_pre.weight".into(), vec![512, 192, 7], DType::F16),
                ("enc_q.enc.in_layers.0.bias".into(), vec![384], DType::F16),
            ],
            extra: Vec::new(),
        }
    }
}

/// `f` as the bytes of a `.pth` (torch zip with prefix `archive`), the way
/// RVC saves inference models. Weight `i` lives in storage `i` and holds
/// small deterministic values.
pub fn rvc_pth(f: &Fixture) -> Vec<u8> {
    let mut weights = Vec::new();
    let mut storages = Vec::new();
    for (i, (name, shape, dtype)) in f.weights.iter().enumerate() {
        let key = i.to_string();
        let numel = shape.iter().product::<usize>();
        let values: Vec<f32> = (0..numel)
            .map(|j| f32::from(u8::try_from(j % 8).expect("< 8")) * 0.125 - 0.5)
            .collect();
        let bytes = match dtype {
            DType::F32 => f32_bytes(&values),
            DType::F16 => f16_bytes(&values),
            DType::BF16 => bf16_bytes(&values),
            other => vec![0; numel * other.size()],
        };
        weights.push((
            P::Str(name.clone()),
            P::Tensor {
                key: key.clone(),
                dtype: *dtype,
                numel,
                offset: 0,
                shape: shape.clone(),
                stride: contiguous(shape),
            },
        ));
        storages.push((key, bytes));
    }

    let mut root = vec![
        (P::Str("weight".into()), P::Dict(weights)),
        (P::Str("config".into()), P::List(f.config.clone())),
        (P::Str("info".into()), P::Str("300epoch".into())),
        (P::Str("sr".into()), P::Str("40k".into())),
    ];
    if let Some(f0) = f.f0 {
        root.push((P::Str("f0".into()), P::Int(f0)));
    }
    if let Some(version) = f.version {
        root.push((P::Str("version".into()), P::Str(version.into())));
    }
    for (k, v) in &f.extra {
        root.push((P::Str((*k).into()), v.clone()));
    }
    let storages: Vec<(&str, Vec<u8>)> = storages
        .iter()
        .map(|(k, b)| (k.as_str(), b.clone()))
        .collect();
    torch_zip("archive", &pickle(&P::OrderedDict(root)), &storages)
}

/// A tiny template for `cfg`: stripped FLOAT initializers
/// `enc_p.emb_phone.weight` [192, 768], `emb_g.weight` [109, 256] and
/// `dec.conv_pre.weight` [512, 192, 7], as in [`Fixture::v2_40k`], and one
/// constant `c` that keeps its data.
pub fn toy_template(cfg: ArchConfig) -> Template {
    let stripped = |name: &str, dims: &[i64]| TensorProto {
        name: Some(name.into()),
        dims: dims.to_vec(),
        data_type: Some(FLOAT),
        ..Default::default()
    };
    let model = ModelProto {
        ir_version: Some(8),
        graph: Some(GraphProto {
            initializer: vec![
                stripped("enc_p.emb_phone.weight", &[192, 768]),
                stripped("emb_g.weight", &[109, 256]),
                TensorProto {
                    name: Some("c".into()),
                    dims: vec![2],
                    data_type: Some(FLOAT),
                    raw_data: Some(f32_bytes(&[1.0, 2.0])),
                    ..Default::default()
                },
                stripped("dec.conv_pre.weight", &[512, 192, 7]),
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    Template {
        info: TemplateInfo {
            file: "toy.onnx".into(),
            sha256: String::new(),
            rvc_commit: String::new(),
            torch: String::new(),
            config: cfg,
        },
        bytes: Vec::leak(crate::onnx::encode(&model)),
    }
}
