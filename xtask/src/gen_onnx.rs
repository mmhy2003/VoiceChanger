//! Generates the committed prost types for ONNX from the vendored `onnx.proto`.

use std::path::{Path, PathBuf};

/// Compiles `proto_dir/onnx.proto` (pure Rust, no `protoc`) and writes `onnx.rs` into `out_dir`.
/// Returns the path of the written file.
pub fn generate(proto_dir: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    let fds = protox::compile([proto_dir.join("onnx.proto")], [proto_dir])
        .map_err(|e| format!("{}: {e}", proto_dir.display()))?;
    prost_build::Config::new()
        .out_dir(out_dir)
        .compile_fds(fds)
        .map_err(|e| format!("prost-build: {e}"))?;
    Ok(out_dir.join("onnx.rs"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_onnx_matches_committed() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let out = std::env::temp_dir().join(format!("vc-xtask-gen-onnx-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        let generated = generate(&root.join("crates/convert/proto"), &out);
        let generated = generated.map(|p| std::fs::read(p).unwrap());
        let _ = std::fs::remove_dir_all(&out);
        let committed = std::fs::read(root.join("crates/convert/src/onnx/onnx.rs"))
            .expect("crates/convert/src/onnx/onnx.rs is missing; run `cargo xtask gen-onnx`");
        assert!(
            generated.unwrap() == committed,
            "onnx.rs is out of date; run `cargo xtask gen-onnx`"
        );
    }
}
