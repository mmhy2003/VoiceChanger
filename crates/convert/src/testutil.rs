//! Test helpers: a pickle writer that emits protocol 2 as `torch.save` does,
//! and a temporary folder removed on drop.

use std::path::{Path, PathBuf};

use crate::pickle::DType;

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
