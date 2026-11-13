//! Converts every public model in `assets/test-models.toml` and reports the
//! pass rate. Local only: the models come from `cargo xtask fetch-test-models`.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Instant;

use vc_convert::{Cancel, Inspection, convert, inspect};

/// The minimum share of corpus models that must convert and validate.
const MIN_PASS_PCT: f64 = 95.0;

/// The PRD's "typical" `.pth` size, whose conversion time is reported.
const TYPICAL_MB: f64 = 55.0;

struct Row {
    name: String,
    khz: Option<u32>,
    mb: f64,
    result: Result<(), String>,
    secs: f64,
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `(name, path)` for each `[[model]]`, panicking if a file isn't fetched.
fn corpus() -> Vec<(String, PathBuf)> {
    let root = workspace();
    let text = std::fs::read_to_string(root.join("assets/test-models.toml"))
        .expect("read assets/test-models.toml");
    let table: toml::Table = text.parse().expect("parse assets/test-models.toml");
    let models = table["model"].as_array().expect("[[model]] entries");
    models
        .iter()
        .map(|m| {
            let field = |k: &str| m[k].as_str().unwrap_or_else(|| panic!("{k} in {m}"));
            let path = root.join("vendor").join(field("dest"));
            assert!(
                path.is_file(),
                "{} is missing: run `cargo xtask fetch-test-models`",
                path.display()
            );
            (field("name").to_owned(), path)
        })
        .collect()
}

fn run(name: String, path: &Path) -> Row {
    let mb = std::fs::metadata(path).map_or(0.0, |m| m.len() as f64 / 1e6);
    let t = Instant::now();
    let mut khz = None;
    let result = inspect(path).and_then(|i: Inspection| {
        khz = Some(i.sample_rate / 1000);
        let (tx, _rx) = mpsc::channel();
        convert(&i, &Cancel::new(), &tx).map(drop)
    });
    Row {
        name,
        khz,
        mb,
        result: result.map_err(|e| e.to_string()),
        secs: t.elapsed().as_secs_f64(),
    }
}

#[test]
#[ignore = "needs cargo xtask fetch-test-models"]
fn public_models_convert() {
    let rows: Vec<Row> = corpus()
        .into_iter()
        .map(|(name, path)| run(name, &path))
        .collect();

    println!("name | kHz | MB | result | s");
    for r in &rows {
        let khz = r.khz.map_or("?".into(), |k| k.to_string());
        let result = r.result.as_ref().err().map_or("ok", String::as_str);
        println!(
            "{} | {khz} | {:.1} | {result} | {:.1}",
            r.name, r.mb, r.secs
        );
    }

    let pass = rows.iter().filter(|r| r.result.is_ok()).count();
    let pct = 100.0 * pass as f64 / rows.len() as f64;
    println!("pass {pass}/{} = {pct:.1}%", rows.len());

    // Reported, not gated: conversion time depends on the hardware.
    if let Some(r) = rows.iter().min_by(|a, b| {
        (a.mb - TYPICAL_MB)
            .abs()
            .total_cmp(&(b.mb - TYPICAL_MB).abs())
    }) {
        println!(
            "closest to {TYPICAL_MB} MB: {} ({:.1} MB) in {:.1} s",
            r.name, r.mb, r.secs
        );
    }

    assert!(pct >= MIN_PASS_PCT, "pass rate {pct:.1}% < {MIN_PASS_PCT}%");
}
