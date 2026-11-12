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
    pub licence: String,
    pub dest: String,
    #[serde(default)]
    pub extract: Option<String>,
}

/// Parses and validates a manifest.
pub fn parse(text: &str) -> Result<Manifest, String> {
    let manifest: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
    for entry in &manifest.model {
        validate(entry)?;
    }
    for entry in &manifest.runtime {
        validate(entry)?;
        if entry.extract.as_deref().is_none_or(str::is_empty) {
            return Err(format!("{}: runtime entries need `extract`", entry.name));
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
    }
}
