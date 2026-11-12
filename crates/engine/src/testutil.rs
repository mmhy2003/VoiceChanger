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
