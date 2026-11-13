//! `assets/base-models.toml`: the pinned downloads `fetch-models` fetches.

use serde::Deserialize;

/// Every pinned download, split into ONNX models and runtime packages.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub model: Vec<Entry>,
    #[serde(default)]
    pub runtime: Vec<Entry>,
}

/// One pinned download. `extract` is the file to take out of a zip package.
#[derive(Debug, Deserialize)]
pub struct Entry {
    pub name: String,
    pub url: String,
    pub sha256: String,
    /// Documents the licence for reviewers; `fetch-models` does not use it.
    #[allow(dead_code)]
    pub licence: String,
    pub dest: String,
    #[serde(default)]
    pub extract: Option<String>,
    /// The licence file inside a runtime package, copied to
    /// `vendor/licences/<dest file stem>.txt`.
    #[serde(default)]
    pub licence_extract: Option<String>,
}

impl Entry {
    /// Where `licence_extract` lands under `vendor/`, named after `dest`.
    pub fn licence_dest(&self) -> Option<String> {
        self.licence_extract.as_ref()?;
        let stem = std::path::Path::new(&self.dest).file_stem()?;
        Some(format!("licences/{}.txt", stem.to_string_lossy()))
    }
}

/// Parses and validates a manifest.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
    for entry in &manifest.model {
        validate(entry)?;
        if entry.licence_extract.is_some() {
            return Err(format!(
                "{}: only runtime entries can have `licence_extract`",
                entry.name
            ));
        }
    }
    for entry in &manifest.runtime {
        validate(entry)?;
        if entry.extract.as_deref().is_none_or(str::is_empty) {
            return Err(format!("{}: runtime entries need `extract`", entry.name));
        }
        if entry.licence_extract.as_deref() == Some("") {
            return Err(format!(
                "{}: `licence_extract` must not be empty",
                entry.name
            ));
        }
    }
    Ok(manifest)
}

fn validate(entry: &Entry) -> Result<(), String> {
    let name = &entry.name;
    if !entry.url.starts_with("https://") {
        return Err(format!("{name}: url must start with https://"));
    }
    let sha = &entry.sha256;
    if sha.len() != 64 || !sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(format!(
            "{name}: sha256 must be 64 lowercase hex characters"
        ));
    }
    let dest = &entry.dest;
    let absolute = dest.starts_with(['/', '\\']) || dest.contains(':');
    if dest.is_empty() || absolute || dest.split(['/', '\\']).any(|part| part == "..") {
        return Err(format!(
            "{name}: dest must be a non-empty relative path without `..`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681";

    fn model(url: &str, sha256: &str, dest: &str) -> String {
        format!(
            "[[model]]\nname = \"m\"\nurl = \"{url}\"\nsha256 = \"{sha256}\"\nlicence = \"MIT\"\ndest = \"{dest}\"\n"
        )
    }

    #[test]
    fn parses_models_and_runtimes() {
        let text = format!(
            "{}\n[[runtime]]\nname = \"r\"\nurl = \"https://example.com/r.nupkg\"\nsha256 = \"{SHA}\"\nlicence = \"MIT\"\nextract = \"lib/r.dll\"\ndest = \"runtime/r.dll\"\n",
            model("https://example.com/m.onnx", SHA, "models/m.onnx")
        );
        let m = parse(&text).unwrap();
        assert_eq!(m.model.len(), 1);
        assert_eq!(m.model[0].name, "m");
        assert_eq!(m.model[0].url, "https://example.com/m.onnx");
        assert_eq!(m.model[0].sha256, SHA);
        assert_eq!(m.model[0].licence, "MIT");
        assert_eq!(m.model[0].dest, "models/m.onnx");
        assert_eq!(m.model[0].extract, None);
        assert_eq!(m.runtime.len(), 1);
        assert_eq!(m.runtime[0].extract.as_deref(), Some("lib/r.dll"));
        assert_eq!(m.runtime[0].dest, "runtime/r.dll");
        assert_eq!(m.runtime[0].licence_extract, None);
        assert_eq!(m.runtime[0].licence_dest(), None);
    }

    fn runtime_with(extra: &str) -> String {
        format!(
            "[[runtime]]\nname = \"r\"\nurl = \"https://example.com/r.nupkg\"\nsha256 = \"{SHA}\"\nlicence = \"MIT\"\nextract = \"lib/r.dll\"\ndest = \"runtime/r.dll\"\n{extra}\n"
        )
    }

    #[test]
    fn parses_licence_extract() {
        let m = parse(&runtime_with("licence_extract = \"LICENSE.txt\"")).unwrap();
        assert_eq!(m.runtime[0].licence_extract.as_deref(), Some("LICENSE.txt"));
        assert_eq!(
            m.runtime[0].licence_dest().as_deref(),
            Some("licences/r.txt")
        );
    }

    #[test]
    fn rejects_empty_licence_extract() {
        let err = parse(&runtime_with("licence_extract = \"\"")).unwrap_err();
        assert!(err.contains("licence_extract"), "{err}");
    }

    #[test]
    fn model_rejects_licence_extract() {
        let text = format!(
            "{}licence_extract = \"LICENSE\"\n",
            model("https://example.com/m.onnx", SHA, "models/m.onnx")
        );
        let err = parse(&text).unwrap_err();
        assert!(err.contains("licence_extract"), "{err}");
    }

    #[test]
    fn rejects_http_url() {
        let err = parse(&model("http://example.com/m.onnx", SHA, "models/m.onnx")).unwrap_err();
        assert!(err.contains("https://"), "{err}");
    }

    #[test]
    fn rejects_bad_sha() {
        for sha in [&SHA[..63], &SHA.to_uppercase(), &format!("{}g", &SHA[..63])] {
            let err =
                parse(&model("https://example.com/m.onnx", sha, "models/m.onnx")).unwrap_err();
            assert!(err.contains("sha256"), "{err}");
        }
    }

    #[test]
    fn rejects_parent_dest() {
        for dest in [
            "../m.onnx",
            "models/../../m.onnx",
            "",
            "/abs/m.onnx",
            "C:/abs/m.onnx",
        ] {
            let err = parse(&model("https://example.com/m.onnx", SHA, dest)).unwrap_err();
            assert!(err.contains("dest"), "{dest}: {err}");
        }
    }

    #[test]
    fn runtime_needs_extract() {
        let text = format!(
            "[[runtime]]\nname = \"r\"\nurl = \"https://example.com/r.nupkg\"\nsha256 = \"{SHA}\"\nlicence = \"MIT\"\ndest = \"runtime/r.dll\"\n"
        );
        let err = parse(&text).unwrap_err();
        assert!(err.contains("extract"), "{err}");
    }

    #[test]
    fn repo_manifest_is_valid() {
        let m = parse(include_str!("../../assets/base-models.toml")).unwrap();
        assert_eq!(m.model.len(), 2);
        assert_eq!(m.runtime.len(), 2);
        let mut dests = std::collections::HashSet::new();
        for entry in &m.runtime {
            let dest = entry
                .licence_dest()
                .unwrap_or_else(|| panic!("{}: no licence_extract", entry.name));
            assert!(dests.insert(dest), "{}: duplicate licence dest", entry.name);
        }
    }

    #[test]
    fn repo_base_models_are_mit() {
        let m = parse(include_str!("../../assets/base-models.toml")).unwrap();
        for entry in &m.model {
            assert!(
                entry.licence.starts_with("MIT"),
                "{}: installer bundles it, but its licence is {}",
                entry.name,
                entry.licence
            );
        }
    }

    #[test]
    fn repo_test_manifest_is_valid() {
        let m = parse(include_str!("../../assets/test-models.toml")).unwrap();
        assert!(m.model.len() >= 20, "only {} models", m.model.len());
        assert!(m.runtime.is_empty());
        let mut dests = std::collections::HashSet::new();
        for entry in &m.model {
            let name = &entry.name;
            let dest = &entry.dest;
            assert!(dest.starts_with("test-models/"), "{name}: {dest}");
            assert!(
                dest.ends_with(".pth") || dest.ends_with(".zip"),
                "{name}: {dest}"
            );
            assert!(dests.insert(dest), "{name}: duplicate dest {dest}");
            let revision = entry
                .url
                .split_once("/resolve/")
                .and_then(|(_, rest)| rest.split('/').next())
                .unwrap_or_default();
            assert!(
                revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()),
                "{name}: url not pinned to a commit: {}",
                entry.url
            );
        }
    }
}
