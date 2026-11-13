//! Reading `.pth` files: the torch zip archive, a `.zip` holding one, and
//! the tensors' storages as `f32`.

use std::io::{Cursor, Read};

use zip::ZipArchive;
use zip::read::ZipFile;

use crate::ConvertError;
use crate::pickle::{self, DType, TensorRef, Value};

/// Most entries in one zip.
pub const MAX_ENTRIES: usize = 100_000;
/// Largest sum of the declared (uncompressed) entry sizes in one zip.
pub const MAX_TOTAL: u64 = 4 << 30;
/// Largest `data.pkl`.
pub const MAX_PKL: u64 = 64 << 20;
/// Largest `.pth` inside an outer zip.
pub const MAX_PTH: u64 = 2 << 30;

/// An opened torch archive: its unpickled root and access to its storages.
pub struct PthArchive {
    zip: ZipArchive<Cursor<Vec<u8>>>,
    /// The folder holding `data.pkl`, without the trailing `/`.
    prefix: String,
    /// The `.pth` file's name: `file_name`, or the base name inside an outer zip.
    pub name: String,
    pub root: Value,
}

impl PthArchive {
    /// Opens a torch archive, or an outer zip holding exactly one inference
    /// `.pth` (training checkpoints and `.index` files are skipped).
    pub fn open(bytes: Vec<u8>, file_name: &str) -> Result<Self, ConvertError> {
        let mut zip = open_zip(bytes)?;
        if let Some(prefix) = torch_prefix(&zip) {
            return Self::from_torch(zip, prefix, file_name.to_string());
        }

        let mut inference = Vec::new();
        let mut training = false;
        for i in 0..zip.len() {
            let entry = zip.by_index_raw(i).map_err(not_pytorch)?;
            let name = entry.name().map_err(not_pytorch)?;
            if entry.is_dir() || !name.to_ascii_lowercase().ends_with(".pth") {
                continue;
            }
            if is_training_name(&name) {
                training = true;
            } else {
                inference.push(i);
            }
        }
        let index = match inference[..] {
            [i] => i,
            [] if training => return Err(ConvertError::TrainingCheckpoint),
            [] => return Err(ConvertError::NotPyTorch),
            _ => return Err(ConvertError::SeveralModels),
        };

        let entry = zip.by_index(index).map_err(not_pytorch)?;
        let name = base_name(&entry.name().map_err(not_pytorch)?).to_string();
        let inner = read_limited(entry, MAX_PTH)?;
        let inner_zip = open_zip(inner)?;
        let prefix = torch_prefix(&inner_zip).ok_or(ConvertError::NotPyTorch)?;
        Self::from_torch(inner_zip, prefix, name)
    }

    fn from_torch(
        mut zip: ZipArchive<Cursor<Vec<u8>>>,
        prefix: String,
        name: String,
    ) -> Result<Self, ConvertError> {
        let pkl = zip
            .by_name(&format!("{prefix}/data.pkl"))
            .map_err(not_pytorch)?;
        let pkl = read_limited(pkl, MAX_PKL)?;
        let root = pickle::load(&pkl)?;
        Ok(Self {
            zip,
            prefix,
            name,
            root,
        })
    }

    /// The elements of `t` as `f32`. Only float tensors are accepted.
    pub fn tensor_f32(&mut self, t: &TensorRef) -> Result<Vec<f32>, ConvertError> {
        if !matches!(t.dtype, DType::F32 | DType::F16 | DType::BF16) {
            return Err(ConvertError::NoTemplate(format!(
                "tensor dtype {:?}",
                t.dtype
            )));
        }
        let size = t.dtype.size();
        let path = format!("{}/data/{}", self.prefix, t.key);
        let storage_len = t.storage_numel.checked_mul(size);
        let numel = t.shape.iter().try_fold(1usize, |n, &d| n.checked_mul(d));
        let range = numel
            .and_then(|n| t.offset.checked_add(n).map(|end| (t.offset, end)))
            .and_then(|(start, end)| Some((start.checked_mul(size)?, end.checked_mul(size)?)));
        let (Some(storage_len), Some((start, end))) = (storage_len, range) else {
            tracing::warn!("{path}: tensor size overflows");
            return Err(ConvertError::NotPyTorch);
        };
        if end > storage_len {
            tracing::warn!("{path}: tensor ends at byte {end}, past the storage ({storage_len})");
            return Err(ConvertError::NotPyTorch);
        }

        let entry = match self.zip.by_name(&path) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!("{path}: {e}");
                return Err(ConvertError::NotPyTorch);
            }
        };
        let mut data = Vec::new();
        let wanted = u64::try_from(storage_len).map_err(|_| ConvertError::NotPyTorch)?;
        entry.take(wanted).read_to_end(&mut data).map_err(|e| {
            tracing::warn!("{path}: {e}");
            ConvertError::NotPyTorch
        })?;
        if data.len() < storage_len {
            tracing::warn!(
                "{path}: storage has {} bytes, expected {storage_len}",
                data.len()
            );
            return Err(ConvertError::NotPyTorch);
        }

        let bytes = data.get(start..end).ok_or(ConvertError::NotPyTorch)?;
        Ok(match t.dtype {
            DType::F16 => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&b| half::f16::from_le_bytes(b).to_f32())
                .collect(),
            DType::BF16 => bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&b| half::bf16::from_le_bytes(b).to_f32())
                .collect(),
            _ => bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&b| f32::from_le_bytes(b))
                .collect(),
        })
    }
}

/// True for training checkpoint names: the base name starts with `G_` or
/// `D_` (ASCII case-insensitive).
pub fn is_training_name(name: &str) -> bool {
    let base = base_name(name).as_bytes();
    matches!(base, [b'G' | b'g' | b'D' | b'd', b'_', ..])
}

/// The part of a zip entry name after the last `/` or `\`.
fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Opens `bytes` as a zip and checks the entry count and declared sizes.
fn open_zip(bytes: Vec<u8>) -> Result<ZipArchive<Cursor<Vec<u8>>>, ConvertError> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(not_pytorch)?;
    if zip.len() > MAX_ENTRIES {
        return Err(ConvertError::UnsafeContent("zip limits".into()));
    }
    let mut total = 0u64;
    for i in 0..zip.len() {
        let size = zip.by_index_raw(i).map_err(not_pytorch)?.size();
        total = total.saturating_add(size);
    }
    if total > MAX_TOTAL {
        return Err(ConvertError::UnsafeContent("zip limits".into()));
    }
    Ok(zip)
}

/// `<prefix>` when an entry is `<prefix>/data.pkl` with exactly one `/`.
fn torch_prefix(zip: &ZipArchive<Cursor<Vec<u8>>>) -> Option<String> {
    zip.file_names()
        .filter_map(Result::ok)
        .find_map(|name| match name.split_once('/') {
            Some((prefix, "data.pkl")) if !prefix.is_empty() => Some(prefix.to_string()),
            _ => None,
        })
}

/// Reads an entry whose declared and actual size are at most `limit`.
fn read_limited(entry: ZipFile<'_, Cursor<Vec<u8>>>, limit: u64) -> Result<Vec<u8>, ConvertError> {
    if entry.size() > limit {
        return Err(ConvertError::UnsafeContent("zip limits".into()));
    }
    let mut data = Vec::new();
    let read = entry
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(not_pytorch)?;
    if u64::try_from(read).map_or(true, |n| n > limit) {
        return Err(ConvertError::UnsafeContent("zip limits".into()));
    }
    Ok(data)
}

fn not_pytorch(e: impl std::fmt::Display) -> ConvertError {
    tracing::warn!("not a PyTorch archive: {e}");
    ConvertError::NotPyTorch
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pickle::{DType, TensorRef, Value};
    use crate::testutil::{
        P, bf16_bytes, contiguous, f16_bytes, f32_bytes, pickle, torch_zip, zip_of,
    };

    fn tensor(key: &str, dtype: DType, numel: usize, offset: usize, shape: &[usize]) -> P {
        P::Tensor {
            key: key.into(),
            dtype,
            numel,
            offset,
            shape: shape.to_vec(),
            stride: contiguous(shape),
        }
    }

    /// A torch archive whose root is `{"w": tensor}` over storage `0`.
    fn one_tensor(t: P, storage: Vec<u8>) -> Vec<u8> {
        let pkl = pickle(&P::Dict(vec![(P::Str("w".into()), t)]));
        torch_zip("archive", &pkl, &[("0", storage)])
    }

    /// A small inference model, as the bytes of a `.pth`.
    fn model() -> Vec<u8> {
        let values = [1.0, 2.0];
        one_tensor(tensor("0", DType::F32, 2, 0, &[2]), f32_bytes(&values))
    }

    fn open_ok(bytes: Vec<u8>, name: &str) -> PthArchive {
        match PthArchive::open(bytes, name) {
            Ok(a) => a,
            Err(e) => panic!("open failed: {e}"),
        }
    }

    fn open_err(bytes: Vec<u8>, name: &str) -> ConvertError {
        match PthArchive::open(bytes, name) {
            Ok(_) => panic!("open succeeded"),
            Err(e) => e,
        }
    }

    fn w(archive: &PthArchive) -> TensorRef {
        match archive.root.get("w") {
            Some(Value::Tensor(t)) => t.clone(),
            other => panic!("no tensor w: {other:?}"),
        }
    }

    /// Opens `bytes` and reads tensor `w` as f32.
    fn read_w(bytes: Vec<u8>) -> Result<Vec<f32>, ConvertError> {
        let mut archive = open_ok(bytes, "m.pth");
        let t = w(&archive);
        archive.tensor_f32(&t)
    }

    fn roundtrip(dtype: DType, values: &[f32], bytes: Vec<u8>) {
        let t = tensor("0", dtype, values.len(), 0, &[values.len()]);
        let got = read_w(one_tensor(t, bytes)).expect("read tensor");
        assert_eq!(
            got.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
            values.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn opens_torch_archive() {
        let archive = open_ok(model(), "voice.pth");
        assert_eq!(archive.name, "voice.pth");
        let t = w(&archive);
        assert_eq!(t.dtype, DType::F32);
        assert_eq!(t.shape, [2]);
    }

    #[test]
    fn fp16_to_fp32_exact() {
        let values = [0.0, 1.0, -2.5, 65504.0, 6.103_515_6e-5];
        roundtrip(DType::F16, &values, f16_bytes(&values));
    }

    #[test]
    fn bf16_to_fp32_exact() {
        let values = [1.0, -3.0, 0.15625];
        roundtrip(DType::BF16, &values, bf16_bytes(&values));
    }

    #[test]
    fn f32_passthrough() {
        let values = [0.1, -7.25, f32::MAX, 1e-30];
        roundtrip(DType::F32, &values, f32_bytes(&values));
    }

    #[test]
    fn offset_slice() {
        let storage = f32_bytes(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        let t = tensor("0", DType::F32, 6, 2, &[2]);
        assert_eq!(read_w(one_tensor(t, storage)).unwrap(), [2.0, 3.0]);
    }

    #[test]
    fn not_zip_is_not_pytorch() {
        let legacy = b"\x80\x02\x8a\x0al\xfc\x9cF\xf9 j\xa8P\x19.".to_vec();
        assert!(matches!(
            open_err(legacy, "m.pth"),
            ConvertError::NotPyTorch
        ));
        let text = b"hello, this is not a model at all".to_vec();
        assert!(matches!(open_err(text, "m.pth"), ConvertError::NotPyTorch));
    }

    #[test]
    fn zip_without_data_pkl_is_not_pytorch() {
        let bytes = zip_of(&[("archive/version", b"3"), ("readme.txt", b"hi")]);
        assert!(matches!(open_err(bytes, "m.pth"), ConvertError::NotPyTorch));
    }

    #[test]
    fn nested_zip_with_one_pth() {
        let pth = model();
        let outer = zip_of(&[("voice/voice.pth", &pth), ("voice/added_x.index", b"idx")]);
        let archive = open_ok(outer, "voice.zip");
        assert_eq!(archive.name, "voice.pth");
        let mut archive = archive;
        let t = w(&archive);
        assert_eq!(archive.tensor_f32(&t).unwrap(), [1.0, 2.0]);
    }

    #[test]
    fn nested_zip_several_models() {
        let pth = model();
        let outer = zip_of(&[("a.pth", &pth), ("b.PTH", &pth)]);
        assert!(matches!(
            open_err(outer, "x.zip"),
            ConvertError::SeveralModels
        ));
    }

    #[test]
    fn nested_zip_only_training() {
        let pth = model();
        let outer = zip_of(&[("G_100.pth", &pth), ("D_100.pth", &pth)]);
        assert!(matches!(
            open_err(outer, "x.zip"),
            ConvertError::TrainingCheckpoint
        ));
    }

    #[test]
    fn nested_zip_no_pth() {
        let outer = zip_of(&[("added_x.index", b"idx"), ("notes.txt", b"hi")]);
        assert!(matches!(open_err(outer, "x.zip"), ConvertError::NotPyTorch));
    }

    #[test]
    fn nested_twice_rejected() {
        let inner = zip_of(&[("voice.pth", &model())]);
        let middle = zip_of(&[("voice.pth", &inner)]);
        let outer = zip_of(&[("voice.pth", &middle)]);
        assert!(matches!(
            open_err(middle, "x.zip"),
            ConvertError::NotPyTorch
        ));
        assert!(matches!(open_err(outer, "x.zip"), ConvertError::NotPyTorch));
    }

    #[test]
    fn missing_storage_entry_is_not_pytorch() {
        let pkl = pickle(&P::Dict(vec![(
            P::Str("w".into()),
            tensor("0", DType::F32, 2, 0, &[2]),
        )]));
        let bytes = torch_zip("archive", &pkl, &[]);
        assert!(matches!(read_w(bytes), Err(ConvertError::NotPyTorch)));
    }

    #[test]
    fn short_storage_is_not_pytorch() {
        let t = tensor("0", DType::F32, 4, 0, &[2]);
        let bytes = one_tensor(t, f32_bytes(&[1.0, 2.0, 3.0]));
        assert!(matches!(read_w(bytes), Err(ConvertError::NotPyTorch)));
    }

    #[test]
    fn int_tensor_is_no_template() {
        let t = tensor("0", DType::I64, 2, 0, &[2]);
        let bytes = one_tensor(t, vec![0; 16]);
        assert!(matches!(read_w(bytes), Err(ConvertError::NoTemplate(_))));
    }

    #[test]
    fn too_many_entries_is_unsafe() {
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("e{i}")).collect();
        let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
        match open_err(zip_of(&files), "x.zip") {
            ConvertError::UnsafeContent(m) => assert_eq!(m, "zip limits"),
            other => panic!("expected UnsafeContent, got {other:?}"),
        }
    }

    #[test]
    fn is_training_name_cases() {
        assert!(is_training_name("G_1.pth"));
        assert!(is_training_name("d_2.PTH"));
        assert!(is_training_name("sub/G_3.pth"));
        assert!(!is_training_name("voice.pth"));
        assert!(!is_training_name("Gx.pth"));
    }
}
