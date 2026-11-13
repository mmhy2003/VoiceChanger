//! The model library: `.onnx` files in one folder, indexed by `library.toml`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fsutil::{back_up_bad, with_suffix, write_atomic};

/// The index file inside the library folder.
pub const INDEX_FILE: &str = "library.toml";
/// The index format this build writes; newer indexes are not read.
pub const INDEX_VERSION: u32 = 1;
const ICONS: &str = "icons";

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
    let base = slug(&name);
    let free =
        |id: &str| !taken.iter().any(|t| t == id) && !root.join(format!("{id}.onnx")).exists();
    let id = std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{base}-{n}")))
        .find(|id| free(id))
        .expect("an unbounded sequence has a free id");
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
}
