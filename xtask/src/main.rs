//! Developer tasks: `cargo xtask <command>`.

// Only the tests use it until `fetch-models` is implemented.
#[allow(dead_code)]
mod manifest;

use std::process::ExitCode;

const USAGE: &str = "usage: cargo xtask fetch-models";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("fetch-models") => {
            eprintln!("not implemented");
            ExitCode::from(2)
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
