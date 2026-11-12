//! Converts a WAV file with an RVC v2 ONNX model.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use vc_engine::{ConvertOptions, Provider, convert_file};

const USAGE: &str = "usage: vc-convert <in.wav> <out.wav> --model m.onnx [--sr N] [--pitch N] \
                     [--gate-db N] [--provider dml|cuda|cpu] [--device N] [--out-rate N]";

/// Parses the arguments after the program name into the input, the output and the options.
fn parse_args(args: &[String]) -> Result<(PathBuf, PathBuf, ConvertOptions), String> {
    let mut paths = Vec::new();
    let mut model = None;
    let mut sr = None;
    let mut pitch = None;
    let mut gate_db = None;
    let mut provider = "dml".to_string();
    let mut device = 0;
    let mut out_rate = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            paths.push(PathBuf::from(arg));
            continue;
        }
        let value = it
            .next()
            .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))?;
        match arg.as_str() {
            "--model" => model = Some(PathBuf::from(value)),
            "--sr" => sr = Some(number(arg, value)?),
            "--pitch" => pitch = Some(number(arg, value)?),
            "--gate-db" => gate_db = Some(number(arg, value)?),
            "--provider" => provider = value.clone(),
            "--device" => device = number(arg, value)?,
            "--out-rate" => out_rate = Some(number(arg, value)?),
            _ => return Err(format!("unknown flag {arg}\n{USAGE}")),
        }
    }

    let [input, output] = <[PathBuf; 2]>::try_from(paths)
        .map_err(|_| format!("expected an input and an output WAV path\n{USAGE}"))?;
    let model = model.ok_or_else(|| format!("--model is required\n{USAGE}"))?;
    let mut opts = ConvertOptions::new(model);
    opts.provider = match provider.as_str() {
        "dml" => Provider::DirectMl { device },
        "cuda" => Provider::Cuda { device },
        "cpu" => Provider::Cpu,
        other => return Err(format!("unknown provider {other}: use dml, cuda or cpu")),
    };
    opts.sr_override = sr;
    opts.out_rate = out_rate;
    if let Some(p) = pitch {
        opts.params.pitch_semitones = p;
    }
    if let Some(g) = gate_db {
        opts.params.gate_db = g;
    }
    Ok((input, output, opts))
}

fn number<T: FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag}: invalid number {value}"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args).and_then(|(input, output, opts)| {
        convert_file(&input, &output, &opts).map_err(|e| e.to_string())
    });
    match result {
        Ok(report) => {
            println!("provider: {}", report.active_provider);
            println!("warm-up: {} ms", report.warmup.as_millis());
            println!("real-time factor: {:.2}", report.real_time_factor());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn parses_minimal() {
        let (input, output, opts) = parse_args(&args("in.wav out.wav --model m.onnx")).unwrap();
        assert_eq!(input, PathBuf::from("in.wav"));
        assert_eq!(output, PathBuf::from("out.wav"));
        assert_eq!(opts.model, PathBuf::from("m.onnx"));
        assert_eq!(opts.sr_override, None);
        assert_eq!(opts.out_rate, None);
        assert_eq!(opts.params.pitch_semitones, 0.0);
    }

    #[test]
    fn parses_all_flags() {
        let (_, _, opts) = parse_args(&args(
            "in.wav out.wav --model m.onnx --sr 40000 --pitch -12 --gate-db -45 \
             --provider cuda --device 1 --out-rate 48000",
        ))
        .unwrap();
        assert_eq!(opts.sr_override, Some(40_000));
        assert_eq!(opts.params.pitch_semitones, -12.0);
        assert_eq!(opts.params.gate_db, -45.0);
        assert_eq!(opts.provider, Provider::Cuda { device: 1 });
        assert_eq!(opts.out_rate, Some(48_000));
    }

    #[test]
    fn default_provider_is_dml0() {
        let (_, _, opts) = parse_args(&args("in.wav out.wav --model m.onnx")).unwrap();
        assert_eq!(opts.provider, Provider::DirectMl { device: 0 });
        let (_, _, opts) = parse_args(&args(
            "in.wav out.wav --model m.onnx --provider cpu --device 3",
        ))
        .unwrap();
        assert_eq!(opts.provider, Provider::Cpu);
    }

    #[test]
    fn rejects_missing_model() {
        let err = parse_args(&args("in.wav out.wav --sr 40000")).unwrap_err();
        assert!(err.contains("--model"), "{err}");
    }

    #[test]
    fn rejects_missing_value() {
        let err = parse_args(&args("in.wav out.wav --model")).unwrap_err();
        assert!(err.contains("--model"), "{err}");
    }

    #[test]
    fn rejects_bad_provider() {
        let err = parse_args(&args("in.wav out.wav --model m.onnx --provider tpu")).unwrap_err();
        assert!(err.contains("tpu"), "{err}");
    }

    #[test]
    fn rejects_unknown_flag() {
        let err = parse_args(&args("in.wav out.wav --model m.onnx --fast")).unwrap_err();
        assert!(err.contains("--fast"), "{err}");
    }

    #[test]
    fn rejects_missing_paths() {
        assert!(parse_args(&args("in.wav --model m.onnx")).is_err());
    }
}
