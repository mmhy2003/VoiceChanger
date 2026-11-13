//! Developer tasks: `cargo xtask <command>`.

mod fetch;
mod gen_onnx;
mod manifest;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask <fetch-models|gen-onnx>";

fn main() -> ExitCode {
    let result = match std::env::args().nth(1).as_deref() {
        Some("fetch-models") => fetch_models(),
        Some("gen-onnx") => gen_onnx(),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn root() -> Result<&'static Path, String> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| "xtask has no parent directory".into())
}

fn fetch_models() -> Result<(), String> {
    let root = root()?;
    let path = root.join("assets/base-models.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest = manifest::parse(&text)?;
    fetch::fetch_all(root, &manifest)
}

/// Regenerates `crates/convert/src/onnx/onnx.rs` from the vendored `onnx.proto`.
fn gen_onnx() -> Result<(), String> {
    let root = root()?;
    let tmp = std::env::temp_dir().join(format!("vc-xtask-gen-onnx-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let result = gen_onnx::generate(&root.join("crates/convert/proto"), &tmp).and_then(|src| {
        let dest = root.join("crates/convert/src/onnx/onnx.rs");
        let dir = dest.parent().ok_or("onnx.rs has no parent directory")?;
        std::fs::create_dir_all(dir)
            .and_then(|()| std::fs::copy(&src, &dest))
            .map_err(|e| format!("{}: {e}", dest.display()))?;
        println!("wrote {}", dest.display());
        Ok(())
    });
    let _ = std::fs::remove_dir_all(&tmp);
    result
}
