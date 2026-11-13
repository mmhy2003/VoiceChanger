//! Converts a WAV file with an RVC v2 ONNX model, or streams it block by block as a benchmark.

use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

use vc_engine::stream::stream_file;
use vc_engine::{ConvertOptions, Provider, StreamSettings, convert_file};

const USAGE: &str = "usage: vc-convert <in.wav> <out.wav> --model m.onnx [--sr N] [--pitch N]                      [--gate-db N] [--provider dml|cuda|cpu] [--device N] [--out-rate N]                      [--stream [--block-ms N] [--context-ms N] [--crossfade-ms N]]";

/// Parsed command line.
#[derive(Debug)]
struct Cli {
    input: PathBuf,
    output: PathBuf,
    opts: ConvertOptions,
    /// Clamped stream settings with `--stream`; offline conversion otherwise.
    stream: Option<StreamSettings>,
}

/// Parses the arguments after the program name.
fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut paths = Vec::new();
    let mut model = None;
    let mut sr = None;
    let mut pitch = None;
    let mut gate_db = None;
    let mut provider = "dml".to_string();
    let mut device = 0;
    let mut out_rate = None;
    let mut stream = false;
    let mut settings = StreamSettings::default();

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if !arg.starts_with("--") {
            paths.push(PathBuf::from(arg));
            continue;
        }
        if arg == "--stream" {
            stream = true;
            continue;
        }
        let value = it.next().ok_or_else(|| {
            format!(
                "{arg} needs a value
{USAGE}"
            )
        })?;
        match arg.as_str() {
            "--model" => model = Some(PathBuf::from(value)),
            "--sr" => sr = Some(number(arg, value)?),
            "--pitch" => pitch = Some(number(arg, value)?),
            "--gate-db" => gate_db = Some(number(arg, value)?),
            "--provider" => provider = value.clone(),
            "--device" => device = number(arg, value)?,
            "--out-rate" => out_rate = Some(number(arg, value)?),
            "--block-ms" => settings.block_ms = number(arg, value)?,
            "--context-ms" => settings.context_ms = number(arg, value)?,
            "--crossfade-ms" => settings.crossfade_ms = number(arg, value)?,
            _ => {
                return Err(format!(
                    "unknown flag {arg}
{USAGE}"
                ));
            }
        }
    }

    let [input, output] = <[PathBuf; 2]>::try_from(paths).map_err(|_| {
        format!(
            "expected an input and an output WAV path
{USAGE}"
        )
    })?;
    let model = model.ok_or_else(|| {
        format!(
            "--model is required
{USAGE}"
        )
    })?;
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
    Ok(Cli {
        input,
        output,
        opts,
        stream: stream.then(|| settings.clamped()),
    })
}

fn number<T: FromStr>(flag: &str, value: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag}: invalid number {value}"))
}

fn run(cli: Cli) -> Result<(), String> {
    let Cli {
        input,
        output,
        opts,
        stream,
    } = cli;
    if let Some(settings) = stream {
        let report = stream_file(&input, &output, &opts, settings).map_err(|e| e.to_string())?;
        println!("provider: {}", report.active_provider);
        println!("warm-up: {} ms", report.warmup.as_millis());
        println!("block: {} ms", report.block.as_millis());
        println!(
            "inference p50 {} ms · p95 {} ms · max {} ms",
            report.p50().as_millis(),
            report.p95().as_millis(),
            report.max().as_millis()
        );
        println!("p95 / block: {:.0}%", report.p95_ratio() * 100.0);
    } else {
        let report = convert_file(&input, &output, &opts).map_err(|e| e.to_string())?;
        println!("provider: {}", report.active_provider);
        println!("warm-up: {} ms", report.warmup.as_millis());
        println!("real-time factor: {:.2}", report.real_time_factor());
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
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
        let cli = parse_args(&args("in.wav out.wav --model m.onnx")).unwrap();
        let opts = cli.opts;
        assert_eq!(cli.input, PathBuf::from("in.wav"));
        assert_eq!(cli.output, PathBuf::from("out.wav"));
        assert_eq!(opts.model, PathBuf::from("m.onnx"));
        assert_eq!(opts.sr_override, None);
        assert_eq!(opts.out_rate, None);
        assert_eq!(opts.params.pitch_semitones, 0.0);
    }

    #[test]
    fn parses_all_flags() {
        let opts = parse_args(&args(
            "in.wav out.wav --model m.onnx --sr 40000 --pitch -12 --gate-db -45 \
             --provider cuda --device 1 --out-rate 48000",
        ))
        .unwrap()
        .opts;
        assert_eq!(opts.sr_override, Some(40_000));
        assert_eq!(opts.params.pitch_semitones, -12.0);
        assert_eq!(opts.params.gate_db, -45.0);
        assert_eq!(opts.provider, Provider::Cuda { device: 1 });
        assert_eq!(opts.out_rate, Some(48_000));
    }

    #[test]
    fn default_provider_is_dml0() {
        let opts = parse_args(&args("in.wav out.wav --model m.onnx"))
            .unwrap()
            .opts;
        assert_eq!(opts.provider, Provider::DirectMl { device: 0 });
        let opts = parse_args(&args(
            "in.wav out.wav --model m.onnx --provider cpu --device 3",
        ))
        .unwrap()
        .opts;
        assert_eq!(opts.provider, Provider::Cpu);
    }

    #[test]
    fn parses_stream_flags() {
        let cli = parse_args(&args(
            "in.wav out.wav --model m.onnx --stream --block-ms 120",
        ))
        .unwrap();
        let defaults = StreamSettings::default();
        assert_eq!(
            cli.stream,
            Some(StreamSettings {
                block_ms: 120,
                ..defaults
            })
        );
    }

    #[test]
    fn stream_absent_by_default() {
        let cli = parse_args(&args("in.wav out.wav --model m.onnx --block-ms 120")).unwrap();
        assert_eq!(cli.stream, None);
    }

    #[test]
    fn stream_flag_takes_no_value() {
        let cli = parse_args(&args("--stream in.wav out.wav --model m.onnx")).unwrap();
        assert_eq!(cli.input, PathBuf::from("in.wav"));
        assert_eq!(cli.output, PathBuf::from("out.wav"));
        assert_eq!(cli.stream, Some(StreamSettings::default()));
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
