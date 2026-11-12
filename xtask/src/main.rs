//! Developer tasks: `cargo xtask <command>`.

mod fetch;
mod manifest;

use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask fetch-models";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("fetch-models") => match fetch_models() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(1)
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn fetch_models() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("xtask has no parent directory")?;
    let path = root.join("assets/base-models.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let manifest = manifest::parse(&text)?;
    fetch::fetch_all(root, &manifest)
}
