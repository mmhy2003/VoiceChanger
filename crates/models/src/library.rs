//! The model library: `.onnx` files in one folder, indexed by `library.toml`.

use std::fs;
use std::io::{self, Write};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{back_up_bad, with_suffix, write_atomic};

/// The index file inside the library folder.
pub const INDEX_FILE: &str = "library.toml";
/// The index format this build writes; newer indexes are not read.
pub const INDEX_VERSION: u32 = 1;
const ICONS: &str = "icons";
/// The default pitch a model can have, in semitones.
pub const PITCH_RANGE: RangeInclusive<i32> = -24..=24;

/// One model in the library.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub name: String,
    pub file: String,
    pub sample_rate: u32,
    #[serde(default = "yes")]
    pub f0: bool,
    #[serde(default)]
    pub default_pitch: i32,
    #[serde(default)]
    pub icon: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    models: Vec<ModelEntry>,
}

/// A model copied into the library folder but not indexed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedModel {
    pub id: String,
    pub name: String,
    pub file: String,
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("no model with id \"{0}\"")]
    NotFound(String),
    #[error("the icon must be a PNG or JPEG image")]
    IconType,
    #[error("the name cannot be empty")]
    EmptyName,
}

/// The library folder and its index, in display order.
pub struct Library {
    root: PathBuf,
    entries: Vec<ModelEntry>,
}

impl Library {
    /// Reads `root/library.toml`. If it is missing, the library is empty. If it is corrupt or from a newer version, it is renamed to
    /// `library.toml.bad` and the library is empty, with a note. Model files are never touched.
    pub fn open(root: PathBuf) -> (Self, Option<String>) {
        let (entries, note) = read_index(&root.join(INDEX_FILE));
        (Self { root, entries }, note)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn entries(&self) -> &[ModelEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.id.clone()).collect()
    }

    pub fn model_path(&self, e: &ModelEntry) -> PathBuf {
        self.root.join(&e.file)
    }

    pub fn icon_path(&self, e: &ModelEntry) -> Option<PathBuf> {
        e.icon.as_ref().map(|icon| self.root.join(ICONS).join(icon))
    }

    pub fn file_exists(&self, e: &ModelEntry) -> bool {
        self.model_path(e).is_file()
    }

    /// Indexes a staged model at the end of the list, with a default pitch of 0.
    pub fn add(
        &mut self,
        staged: StagedModel,
        sample_rate: u32,
        f0: bool,
    ) -> Result<ModelEntry, LibraryError> {
        let entry = ModelEntry {
            id: staged.id,
            name: staged.name,
            file: staged.file,
            sample_rate,
            f0,
            default_pitch: 0,
            icon: None,
        };
        let mut entries = self.entries.clone();
        entries.push(entry.clone());
        self.commit(entries)?;
        Ok(entry)
    }

    /// Copies `src` into the library and indexes it. If indexing fails, the copy is deleted.
    pub fn import(
        &mut self,
        src: &Path,
        sample_rate: u32,
        f0: bool,
    ) -> Result<ModelEntry, LibraryError> {
        let staged = copy_model(&self.root, src, &self.ids())?;
        self.add(staged.clone(), sample_rate, f0).inspect_err(|_| {
            discard_staged(&self.root, &staged);
        })
    }

    /// Writes `bytes` into the library as a model called `name` and indexes it. If indexing fails, the file is deleted.
    pub fn add_bytes(
        &mut self,
        name: &str,
        bytes: &[u8],
        sample_rate: u32,
        f0: bool,
    ) -> Result<ModelEntry, LibraryError> {
        let staged = stage_bytes(&self.root, name, bytes, &self.ids())?;
        self.add(staged.clone(), sample_rate, f0).inspect_err(|_| {
            discard_staged(&self.root, &staged);
        })
    }

    /// Renames a model. The name is trimmed and cannot be empty.
    pub fn rename(&mut self, id: &str, name: &str) -> Result<(), LibraryError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(LibraryError::EmptyName);
        }
        self.edit(id, |e| e.name = name.to_string())
    }

    /// Deletes a model's file, then removes it from the index and deletes its icon. Files already
    /// gone are ignored. If the file cannot be deleted (it may be in use), the model stays listed.
    /// A leftover icon is not an error.
    pub fn remove(&mut self, id: &str) -> Result<(), LibraryError> {
        let i = self.position(id)?;
        let mut entries = self.entries.clone();
        let entry = entries.remove(i);
        remove_if_exists(&self.model_path(&entry))?;
        self.commit(entries)?;
        if let Some(icon) = self.icon_path(&entry) {
            let _ = remove_if_exists(&icon);
        }
        Ok(())
    }

    /// Swaps a model with the one above it; the first model stays where it is.
    pub fn move_up(&mut self, id: &str) -> Result<(), LibraryError> {
        let i = self.position(id)?;
        if i == 0 {
            return Ok(());
        }
        self.swap(i - 1, i)
    }

    /// Swaps a model with the one below it; the last model stays where it is.
    pub fn move_down(&mut self, id: &str) -> Result<(), LibraryError> {
        let i = self.position(id)?;
        if i + 1 == self.entries.len() {
            return Ok(());
        }
        self.swap(i, i + 1)
    }

    /// Sets the pitch a model starts with, clamped to [`PITCH_RANGE`].
    pub fn set_default_pitch(&mut self, id: &str, pitch: i32) -> Result<(), LibraryError> {
        let pitch = pitch.clamp(*PITCH_RANGE.start(), *PITCH_RANGE.end());
        self.edit(id, |e| e.default_pitch = pitch)
    }

    /// Copies a PNG or JPEG image to `icons/<id>.png` or `icons/<id>.jpg` and makes it the model's icon. An older icon with the
    /// other name is deleted.
    pub fn set_icon(&mut self, id: &str, src: &Path) -> Result<(), LibraryError> {
        let ext = src
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase());
        let ext = match ext.as_deref() {
            Some("png") => "png",
            Some("jpg" | "jpeg") => "jpg",
            _ => return Err(LibraryError::IconType),
        };
        let i = self.position(id)?;
        let file = format!("{id}.{ext}");
        let bytes = fs::read(src)?;
        write_atomic(&self.root.join(ICONS).join(&file), &bytes)?;
        let old = self.icon_path(&self.entries[i]);
        self.edit(id, |e| e.icon = Some(file.clone()))?;
        if let Some(old) = old.filter(|old| old.file_name() != Some(file.as_ref())) {
            remove_if_exists(&old)?;
        }
        Ok(())
    }

    /// The index of `id`, or `NotFound`.
    fn position(&self, id: &str) -> Result<usize, LibraryError> {
        self.entries
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| LibraryError::NotFound(id.to_string()))
    }

    /// Applies `change` to the entry `id` and saves the index.
    fn edit(&mut self, id: &str, change: impl FnOnce(&mut ModelEntry)) -> Result<(), LibraryError> {
        let i = self.position(id)?;
        let mut entries = self.entries.clone();
        change(&mut entries[i]);
        self.commit(entries)
    }

    fn swap(&mut self, a: usize, b: usize) -> Result<(), LibraryError> {
        let mut entries = self.entries.clone();
        entries.swap(a, b);
        self.commit(entries)
    }

    /// Writes `entries` as the index, then makes them the library's entries.
    fn commit(&mut self, entries: Vec<ModelEntry>) -> Result<(), LibraryError> {
        let index = Index {
            version: INDEX_VERSION,
            models: entries,
        };
        let text = toml::to_string_pretty(&index).map_err(io::Error::other)?;
        write_atomic(&self.root.join(INDEX_FILE), text.as_bytes())?;
        self.entries = index.models;
        Ok(())
    }
}

/// Deletes `path`; a file that is already gone is fine.
fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// The entries in `path`, plus a note when the index had to be set aside.
fn read_index(path: &Path) -> (Vec<ModelEntry>, Option<String>) {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Vec::new(), None),
        Err(e) => {
            return (
                Vec::new(),
                Some(format!(
                    "The model library index could not be read ({e}); the library starts empty. Model files were kept."
                )),
            );
        }
    };
    let parsed = String::from_utf8(bytes)
        .ok()
        .and_then(|text| toml::from_str::<Index>(&text).ok());
    let note = match parsed {
        Some(index) if index.version <= INDEX_VERSION => return (index.models, None),
        Some(_) => {
            "The model library was saved by a newer VoiceChanger (saved as library.toml.bad); the library starts empty. Model files were kept."
        }
        None => {
            "The model library index was unreadable (saved as library.toml.bad); the library starts empty. Model files were kept."
        }
    };
    match back_up_bad(path) {
        Ok(_) => (Vec::new(), Some(note.to_string())),
        Err(e) => (
            Vec::new(),
            Some(format!(
                "The model library index could not be read or backed up ({e}); the library starts empty. Model files were kept."
            )),
        ),
    }
}

/// Copies `src` into `root` as `<id>.onnx`. `id` is `slug(stem)`, made unique against `taken` and the files that already exist
/// (`base`, `base-2`, `base-3`, …). `name` is the file stem. A failed copy leaves no file behind.
pub fn copy_model(root: &Path, src: &Path, taken: &[String]) -> Result<StagedModel, LibraryError> {
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = free_id(root, &slug(&name), taken);
    let file = format!("{id}.onnx");
    let dest = root.join(&file);
    let part = with_suffix(&dest, ".part");
    let copied = fs::create_dir_all(root)
        .and_then(|()| fs::copy(src, &part))
        .and_then(|_| fs::rename(&part, &dest));
    if let Err(e) = copied {
        let _ = fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(StagedModel { id, name, file })
}

/// Writes `bytes` into `root` as `<id>.onnx`. The name is trimmed and cannot be empty; `id` is `slug(name)`, made unique as in
/// [`copy_model`]. A failed write leaves no file behind and keeps the original I/O error.
pub fn stage_bytes(
    root: &Path,
    name: &str,
    bytes: &[u8],
    taken: &[String],
) -> Result<StagedModel, LibraryError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(LibraryError::EmptyName);
    }
    let id = free_id(root, &slug(name), taken);
    let file = format!("{id}.onnx");
    let dest = root.join(&file);
    let part = with_suffix(&dest, ".part");
    let written = fs::create_dir_all(root)
        .and_then(|()| fs::File::create(&part))
        .and_then(|mut f| {
            f.write_all(bytes)?;
            f.sync_all()
        })
        .and_then(|()| fs::rename(&part, &dest));
    if let Err(e) = written {
        let _ = fs::remove_file(&part);
        return Err(e.into());
    }
    Ok(StagedModel {
        id,
        name: name.to_string(),
        file,
    })
}

/// The first of `base`, `base-2`, `base-3`, … that is not in `taken` and has no `<id>.onnx` in `root`.
fn free_id(root: &Path, base: &str, taken: &[String]) -> String {
    let free =
        |id: &str| !taken.iter().any(|t| t == id) && !root.join(format!("{id}.onnx")).exists();
    std::iter::once(base.to_string())
        .chain((2..).map(|n| format!("{base}-{n}")))
        .find(|id| free(id))
        .expect("an unbounded sequence has a free id")
}

/// Deletes a staged file that was never added.
pub fn discard_staged(root: &Path, staged: &StagedModel) {
    let _ = fs::remove_file(root.join(&staged.file));
}

/// Lowercase ASCII letters and digits; other runs become one `-`, which is trimmed at the ends; empty gives "model". "Ana v2" gives "ana-v2".
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.push(c.to_ascii_lowercase());
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        "model".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    const NOTE_CORRUPT: &str = "The model library index was unreadable (saved as library.toml.bad); the library starts empty. Model files were kept.";
    const NOTE_NEWER: &str = "The model library was saved by a newer VoiceChanger (saved as library.toml.bad); the library starts empty. Model files were kept.";

    /// Writes `bytes` as `<src folder>/<file_name>` and returns its path.
    fn source(dir: &TempDir, file_name: &str, bytes: &[u8]) -> PathBuf {
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        let path = src.join(file_name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug("Ana v2"), "ana-v2");
        assert_eq!(slug("  "), "model");
        assert_eq!(slug("É_x!!"), "x");
        assert_eq!(slug("--A  b--"), "a-b");
    }

    #[test]
    fn open_missing_index_is_empty() {
        let dir = TempDir::new("open-missing");
        let (lib, note) = Library::open(dir.path().join("models"));
        assert!(lib.entries().is_empty());
        assert_eq!(note, None);
        assert_eq!(lib.root(), dir.path().join("models"));
    }

    #[test]
    fn import_copies_file_and_indexes() {
        let dir = TempDir::new("import");
        let root = dir.path().join("models");
        let src = source(&dir, "Ana v2.onnx", b"onnx bytes");
        let (mut lib, _) = Library::open(root.clone());

        let entry = lib.import(&src, 40_000, true).unwrap();

        let expected = ModelEntry {
            id: "ana-v2".into(),
            name: "Ana v2".into(),
            file: "ana-v2.onnx".into(),
            sample_rate: 40_000,
            f0: true,
            default_pitch: 0,
            icon: None,
        };
        assert_eq!(entry, expected);
        assert_eq!(lib.entries(), std::slice::from_ref(&expected));
        assert_eq!(lib.get("ana-v2"), Some(&expected));
        assert_eq!(lib.ids(), vec!["ana-v2".to_string()]);
        assert_eq!(lib.model_path(&entry), root.join("ana-v2.onnx"));
        assert_eq!(lib.icon_path(&entry), None);
        assert!(lib.file_exists(&entry));
        assert_eq!(
            std::fs::read(root.join("ana-v2.onnx")).unwrap(),
            b"onnx bytes"
        );
        assert_eq!(file_names(&root), vec!["ana-v2.onnx", INDEX_FILE]);

        let (reopened, note) = Library::open(root);
        assert_eq!(note, None);
        assert_eq!(reopened.entries(), std::slice::from_ref(&expected));
    }

    #[test]
    fn import_gives_unique_names() {
        let dir = TempDir::new("unique");
        let root = dir.path().join("models");
        let src = source(&dir, "Ana v2.onnx", b"x");
        let (mut lib, _) = Library::open(root.clone());

        let first = lib.import(&src, 40_000, true).unwrap();
        let second = lib.import(&src, 48_000, true).unwrap();
        assert_eq!(first.id, "ana-v2");
        assert_eq!(second.id, "ana-v2-2");
        assert_eq!(second.file, "ana-v2-2.onnx");
        assert_eq!(second.name, "Ana v2");

        // A file left in the folder without an entry also takes the name.
        std::fs::write(root.join("ana-v2-3.onnx"), b"orphan").unwrap();
        let staged = copy_model(&root, &src, &lib.ids()).unwrap();
        assert_eq!(staged.id, "ana-v2-4");
        assert_eq!(
            std::fs::read(root.join("ana-v2-3.onnx")).unwrap(),
            b"orphan"
        );
    }

    #[test]
    fn copy_failure_leaves_no_file() {
        let dir = TempDir::new("copy-failure");
        let root = dir.path().join("models");
        let missing = dir.path().join("missing.onnx");

        assert!(matches!(
            copy_model(&root, &missing, &[]),
            Err(LibraryError::Io(_))
        ));
        let (mut lib, _) = Library::open(root.clone());
        assert!(lib.import(&missing, 40_000, true).is_err());
        assert!(lib.entries().is_empty());
        assert!(!root.exists() || file_names(&root).is_empty());
    }

    #[test]
    fn discard_staged_removes_file() {
        let dir = TempDir::new("discard");
        let root = dir.path().join("models");
        let src = source(&dir, "Bob.onnx", b"x");

        let staged = copy_model(&root, &src, &[]).unwrap();
        assert_eq!(
            staged,
            StagedModel {
                id: "bob".into(),
                name: "Bob".into(),
                file: "bob.onnx".into(),
            }
        );
        assert!(root.join("bob.onnx").is_file());
        discard_staged(&root, &staged);
        assert!(file_names(&root).is_empty());
    }

    #[test]
    fn add_bytes_indexes_model() {
        let dir = TempDir::new("add-bytes");
        let root = dir.path().join("models");
        let (mut lib, _) = Library::open(root.clone());

        let entry = lib.add_bytes("Ana v2", b"onnx", 48_000, true).unwrap();

        let expected = ModelEntry {
            id: "ana-v2".into(),
            name: "Ana v2".into(),
            file: "ana-v2.onnx".into(),
            sample_rate: 48_000,
            f0: true,
            default_pitch: 0,
            icon: None,
        };
        assert_eq!(entry, expected);
        assert_eq!(lib.entries(), std::slice::from_ref(&expected));
        assert_eq!(std::fs::read(root.join("ana-v2.onnx")).unwrap(), b"onnx");
        assert_eq!(file_names(&root), vec!["ana-v2.onnx", INDEX_FILE]);

        let (reopened, note) = Library::open(root);
        assert_eq!(note, None);
        assert_eq!(reopened.entries(), std::slice::from_ref(&expected));
    }

    #[test]
    fn add_bytes_gives_unique_names() {
        let dir = TempDir::new("add-bytes-unique");
        let root = dir.path().join("models");
        let (mut lib, _) = Library::open(root.clone());

        let first = lib.add_bytes("Ana v2", b"a", 40_000, true).unwrap();
        let second = lib.add_bytes("Ana v2", b"b", 40_000, true).unwrap();
        assert_eq!(first.id, "ana-v2");
        assert_eq!(second.id, "ana-v2-2");
        assert_eq!(second.file, "ana-v2-2.onnx");

        // A file left in the folder without an entry also takes the name.
        std::fs::write(root.join("ana-v2-3.onnx"), b"orphan").unwrap();
        let third = lib.add_bytes("Ana v2", b"c", 40_000, true).unwrap();
        assert_eq!(third.id, "ana-v2-4");
        assert_eq!(
            std::fs::read(root.join("ana-v2-3.onnx")).unwrap(),
            b"orphan"
        );
        assert_eq!(std::fs::read(root.join("ana-v2-4.onnx")).unwrap(), b"c");
    }

    #[test]
    fn add_bytes_rejects_empty_name() {
        let dir = TempDir::new("add-bytes-empty");
        let root = dir.path().join("models");
        let (mut lib, _) = Library::open(root.clone());

        assert!(matches!(
            lib.add_bytes("  ", b"x", 40_000, true),
            Err(LibraryError::EmptyName)
        ));
        assert!(lib.entries().is_empty());
        assert!(!root.exists() || file_names(&root).is_empty());
    }

    #[test]
    fn stage_bytes_failure_leaves_no_file() {
        let dir = TempDir::new("stage-failure");
        // A file in place of the library folder makes creating it fail.
        let root = dir.path().join("models");
        std::fs::write(&root, b"not a folder").unwrap();

        assert!(matches!(
            stage_bytes(&root, "Ana", b"x", &[]),
            Err(LibraryError::Io(_))
        ));
        assert_eq!(file_names(dir.path()), vec!["models"]);
        assert_eq!(std::fs::read(&root).unwrap(), b"not a folder");
    }

    #[test]
    fn stage_bytes_trims_name() {
        let dir = TempDir::new("stage-trim");
        let root = dir.path().join("models");

        let staged = stage_bytes(&root, "  Ana v2 \t", b"x", &[]).unwrap();
        assert_eq!(
            staged,
            StagedModel {
                id: "ana-v2".into(),
                name: "Ana v2".into(),
                file: "ana-v2.onnx".into(),
            }
        );
        assert_eq!(file_names(&root), vec!["ana-v2.onnx"]);
    }

    #[test]
    fn corrupt_index_backed_up() {
        let dir = TempDir::new("corrupt");
        let root = dir.path().join("models");
        let src = source(&dir, "Ana.onnx", b"model");
        let (mut lib, _) = Library::open(root.clone());
        lib.import(&src, 40_000, true).unwrap();
        std::fs::write(root.join(INDEX_FILE), "this is [not toml").unwrap();

        let (lib, note) = Library::open(root.clone());
        assert!(lib.entries().is_empty());
        assert_eq!(note.as_deref(), Some(NOTE_CORRUPT));
        assert_eq!(
            std::fs::read_to_string(root.join("library.toml.bad")).unwrap(),
            "this is [not toml"
        );
        assert!(!root.join(INDEX_FILE).exists());
        assert_eq!(std::fs::read(root.join("ana.onnx")).unwrap(), b"model");
    }

    #[test]
    fn newer_index_backed_up() {
        let dir = TempDir::new("newer");
        let root = dir.path().join("models");
        std::fs::create_dir_all(&root).unwrap();
        let newer = format!("version = {}\n", INDEX_VERSION + 1);
        std::fs::write(root.join(INDEX_FILE), &newer).unwrap();

        let (lib, note) = Library::open(root.clone());
        assert!(lib.entries().is_empty());
        assert_eq!(note.as_deref(), Some(NOTE_NEWER));
        assert_eq!(
            std::fs::read_to_string(root.join("library.toml.bad")).unwrap(),
            newer
        );
    }

    #[test]
    fn missing_file_entry() {
        let dir = TempDir::new("missing-file");
        let root = dir.path().join("models");
        let src = source(&dir, "Ana.onnx", b"model");
        let (mut lib, _) = Library::open(root.clone());
        let entry = lib.import(&src, 40_000, true).unwrap();
        std::fs::remove_file(lib.model_path(&entry)).unwrap();

        let (lib, note) = Library::open(root);
        assert_eq!(note, None);
        assert_eq!(lib.entries(), std::slice::from_ref(&entry));
        assert!(!lib.file_exists(&entry));
    }

    /// A library in `dir/models` with one fake model per name, in that order.
    fn library_with(dir: &TempDir, names: &[&str]) -> Library {
        let (mut lib, _) = Library::open(dir.path().join("models"));
        for name in names {
            let src = source(dir, &format!("{name}.onnx"), b"model");
            lib.import(&src, 40_000, true).unwrap();
        }
        lib
    }

    #[test]
    fn rename_trims_and_rejects_empty() {
        let dir = TempDir::new("rename");
        let mut lib = library_with(&dir, &["a"]);

        lib.rename("a", "  Alice v2 \t").unwrap();
        assert_eq!(lib.get("a").unwrap().name, "Alice v2");

        assert!(matches!(
            lib.rename("a", "   "),
            Err(LibraryError::EmptyName)
        ));
        assert_eq!(lib.get("a").unwrap().name, "Alice v2");
        assert!(matches!(
            lib.rename("zz", "X"),
            Err(LibraryError::NotFound(id)) if id == "zz"
        ));
    }

    #[test]
    fn remove_deletes_model_and_icon() {
        let dir = TempDir::new("remove");
        let mut lib = library_with(&dir, &["a", "b"]);
        let icon = source(&dir, "face.png", b"png");
        lib.set_icon("a", &icon).unwrap();
        let root = lib.root().to_path_buf();
        assert!(root.join("icons").join("a.png").is_file());

        lib.remove("a").unwrap();
        assert_eq!(lib.ids(), vec!["b".to_string()]);
        assert!(!root.join("a.onnx").exists());
        assert!(!root.join("icons").join("a.png").exists());
        assert!(root.join("b.onnx").is_file());

        // A model whose file is already gone is still removed.
        std::fs::remove_file(root.join("b.onnx")).unwrap();
        lib.remove("b").unwrap();
        assert!(lib.entries().is_empty());
    }

    #[test]
    fn remove_keeps_entry_when_file_cannot_be_deleted() {
        let dir = TempDir::new("remove-locked");
        let mut lib = library_with(&dir, &["a", "b"]);
        let root = lib.root().to_path_buf();
        // A directory in place of the model file makes deleting it fail.
        std::fs::remove_file(root.join("a.onnx")).unwrap();
        std::fs::create_dir(root.join("a.onnx")).unwrap();

        assert!(matches!(lib.remove("a"), Err(LibraryError::Io(_))));
        assert_eq!(lib.ids(), vec!["a".to_string(), "b".to_string()]);
        let (reopened, _) = Library::open(root);
        assert_eq!(reopened.ids(), lib.ids());
    }

    #[test]
    fn remove_unknown_is_not_found() {
        let dir = TempDir::new("remove-unknown");
        let mut lib = library_with(&dir, &["a"]);
        assert!(matches!(
            lib.remove("zz"),
            Err(LibraryError::NotFound(id)) if id == "zz"
        ));
        assert_eq!(lib.ids(), vec!["a".to_string()]);
    }

    #[test]
    fn reorder_edges() {
        let dir = TempDir::new("reorder");
        let mut lib = library_with(&dir, &["a", "b", "c"]);
        let order = |lib: &Library| lib.ids().join(",");

        lib.move_up("a").unwrap();
        assert_eq!(order(&lib), "a,b,c");
        lib.move_down("c").unwrap();
        assert_eq!(order(&lib), "a,b,c");
        lib.move_down("a").unwrap();
        assert_eq!(order(&lib), "b,a,c");
        lib.move_up("c").unwrap();
        assert_eq!(order(&lib), "b,c,a");
        assert!(matches!(lib.move_up("zz"), Err(LibraryError::NotFound(_))));
        assert!(matches!(
            lib.move_down("zz"),
            Err(LibraryError::NotFound(_))
        ));
    }

    #[test]
    fn default_pitch_clamped_and_saved() {
        let dir = TempDir::new("pitch");
        let mut lib = library_with(&dir, &["a"]);

        lib.set_default_pitch("a", 30).unwrap();
        assert_eq!(lib.get("a").unwrap().default_pitch, 24);
        lib.set_default_pitch("a", -30).unwrap();
        assert_eq!(lib.get("a").unwrap().default_pitch, -24);
        lib.set_default_pitch("a", 5).unwrap();
        assert_eq!(lib.get("a").unwrap().default_pitch, 5);
        assert!(matches!(
            lib.set_default_pitch("zz", 1),
            Err(LibraryError::NotFound(_))
        ));

        let (reopened, _) = Library::open(lib.root().to_path_buf());
        assert_eq!(reopened.get("a").unwrap().default_pitch, 5);
    }

    #[test]
    fn set_icon_copies_and_replaces() {
        let dir = TempDir::new("icon");
        let mut lib = library_with(&dir, &["a"]);
        let icons = lib.root().join("icons");

        lib.set_icon("a", &source(&dir, "face.PNG", b"png bytes"))
            .unwrap();
        let entry = lib.get("a").unwrap().clone();
        assert_eq!(entry.icon.as_deref(), Some("a.png"));
        assert_eq!(lib.icon_path(&entry), Some(icons.join("a.png")));
        assert_eq!(std::fs::read(icons.join("a.png")).unwrap(), b"png bytes");

        lib.set_icon("a", &source(&dir, "face.jpeg", b"jpeg bytes"))
            .unwrap();
        assert_eq!(lib.get("a").unwrap().icon.as_deref(), Some("a.jpg"));
        assert_eq!(file_names(&icons), vec!["a.jpg"]);
        assert_eq!(std::fs::read(icons.join("a.jpg")).unwrap(), b"jpeg bytes");

        // The same name again is overwritten in place, not deleted.
        lib.set_icon("a", &source(&dir, "other.jpg", b"new jpg"))
            .unwrap();
        assert_eq!(file_names(&icons), vec!["a.jpg"]);
        assert_eq!(std::fs::read(icons.join("a.jpg")).unwrap(), b"new jpg");

        assert!(matches!(
            lib.set_icon("zz", &source(&dir, "x.png", b"x")),
            Err(LibraryError::NotFound(_))
        ));
    }

    #[test]
    fn set_icon_rejects_other_types() {
        let dir = TempDir::new("icon-type");
        let mut lib = library_with(&dir, &["a"]);

        for name in ["face.gif", "face.bmp", "face"] {
            assert!(matches!(
                lib.set_icon("a", &source(&dir, name, b"x")),
                Err(LibraryError::IconType)
            ));
        }
        assert_eq!(lib.get("a").unwrap().icon, None);
        assert!(!lib.root().join("icons").exists());
    }

    #[test]
    fn edits_persist_after_reopen() {
        let dir = TempDir::new("persist");
        let mut lib = library_with(&dir, &["a", "b"]);
        lib.rename("b", "Bob").unwrap();
        lib.move_up("b").unwrap();
        lib.set_default_pitch("a", -7).unwrap();
        lib.set_icon("a", &source(&dir, "a.jpg", b"jpg")).unwrap();

        let (reopened, note) = Library::open(lib.root().to_path_buf());
        assert_eq!(note, None);
        assert_eq!(reopened.entries(), lib.entries());
        assert_eq!(reopened.ids(), vec!["b".to_string(), "a".to_string()]);
        assert_eq!(reopened.get("b").unwrap().name, "Bob");
        assert_eq!(reopened.get("a").unwrap().default_pitch, -7);
        assert_eq!(reopened.get("a").unwrap().icon.as_deref(), Some("a.jpg"));
    }
}
