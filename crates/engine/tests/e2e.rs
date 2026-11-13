//! End-to-end conversion with a user-supplied RVC v2 model (`VC_TEST_MODEL`).

use std::f32::consts::TAU;
use std::sync::Arc;

use vc_engine::offline::{read_wav_mono, write_wav_f32};
use vc_engine::{
    Controls, ConvertOptions, Pipeline, PipelineConfig, Provider, StreamSettings, StreamStats,
    Streamer, convert_file,
};

const SR: u32 = 16_000;

/// 3 s of a 150 Hz pulse train with ±5% vibrato, through a one-pole low-pass, peaking near 0.3.
fn voiced_signal() -> Vec<f32> {
    let n = 3 * SR as usize;
    let mut phase = 0.0f32;
    let mut lp = 0.0f32;
    let mut raw: Vec<f32> = (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let f0 = 150.0 * (1.0 + 0.05 * (TAU * 5.0 * t).sin());
            phase += f0 / SR as f32;
            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0
            } else {
                0.0
            };
            lp += 0.1 * (pulse - lp);
            lp
        })
        .collect();
    let peak = raw.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    raw.iter_mut().for_each(|v| *v *= 0.3 / peak);
    raw
}

fn test_model() -> String {
    std::env::var("VC_TEST_MODEL")
        .expect("set VC_TEST_MODEL to an RVC v2 WebUI ONNX export (and VC_TEST_MODEL_SR if needed)")
}

fn test_model_sr() -> Option<u32> {
    std::env::var("VC_TEST_MODEL_SR")
        .ok()
        .map(|s| s.parse().expect("VC_TEST_MODEL_SR must be a sample rate"))
}

#[test]
#[ignore = "needs VC_TEST_MODEL (an RVC v2 WebUI ONNX export) and `cargo xtask fetch-models`"]
fn converts_voiced_signal() {
    let mut opts = ConvertOptions::new(test_model());
    opts.provider = Provider::Cpu;
    opts.sr_override = test_model_sr();

    let dir = std::env::temp_dir();
    let input = dir.join(format!("vc-engine-e2e-{}-in.wav", std::process::id()));
    let output = dir.join(format!("vc-engine-e2e-{}-out.wav", std::process::id()));
    write_wav_f32(&input, &voiced_signal(), SR).unwrap();

    let report = convert_file(&input, &output, &opts).unwrap();
    let (y, rate) = read_wav_mono(&output).unwrap();
    std::fs::remove_file(&input).unwrap();
    std::fs::remove_file(&output).unwrap();
    println!(
        "provider: {}, real-time factor: {:.2}",
        report.active_provider,
        report.real_time_factor()
    );

    assert_eq!(rate, SR);
    assert!(y.len().abs_diff(48_000) <= 320, "length {}", y.len());
    assert!(y.iter().all(|v| v.is_finite()));
    let rms = (y.iter().map(|v| v * v).sum::<f32>() / y.len() as f32).sqrt();
    assert!(rms > 1e-3, "rms {rms}");
}

#[test]
#[ignore = "needs VC_TEST_MODEL (an RVC v2 WebUI ONNX export) and `cargo xtask fetch-models`"]
fn streams_voiced_signal() {
    let settings = StreamSettings::default();
    let pipeline = Pipeline::new(&PipelineConfig {
        model: test_model().into(),
        sr_override: test_model_sr(),
        provider: Provider::Cpu,
        warmup_len: settings.window_16k(),
        warmup_context: 0,
    })
    .unwrap();
    let sr = pipeline.sample_rate() as usize;
    let mut streamer = Streamer::new(
        pipeline,
        settings,
        Arc::new(Controls::new()),
        Arc::new(StreamStats::new()),
    );
    streamer.prepare(SR).unwrap();

    let mut out = Vec::new();
    for chunk in voiced_signal().chunks(320) {
        streamer.process(chunk, &mut out).unwrap();
    }

    assert_eq!(out.len(), 12 * (4000 * sr / 16_000));
    assert!(out.iter().all(|v| v.is_finite()));
    let half = &out[out.len() / 2..];
    let rms = (half.iter().map(|v| v * v).sum::<f32>() / half.len() as f32).sqrt();
    assert!(rms > 1e-3, "rms {rms}");
}
