//! A running audio session: WASAPI streams, the processing worker and clean shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

use crate::devices::Selection;
use crate::processor::Processor;
use crate::ring::{self, HeapCons, HeapProd};
use crate::shared::{Endpoint, Shared, StreamStatus};
use crate::worker::{self, OutputConfig, OutputKind, State, WorkerConfig};

/// Why a session could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// No device has the selected name.
    #[error("device \"{0}\" was not found")]
    DeviceNotFound(String),
    /// The device's default format cannot be used.
    #[error("device \"{device}\" has an unsupported format: {reason}")]
    UnsupportedConfig {
        /// Device name.
        device: String,
        /// What is wrong with the format.
        reason: String,
    },
    /// A stream (or the processing thread) could not be built or started.
    #[error("could not start \"{device}\": {reason}")]
    BuildStream {
        /// Device name.
        device: String,
        /// The underlying error.
        reason: String,
    },
}

/// Sample rates of a running session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// Input device rate.
    pub input_rate: u32,
    /// Cable output device rate.
    pub cable_rate: u32,
    /// Monitor device rate, if the monitor is on.
    pub monitor_rate: Option<u32>,
}

/// A running session. Dropping it (or calling `stop`, which returns the processor) stops the
/// streams and joins the worker.
pub struct AudioSession {
    streams: Vec<cpal::Stream>, // input first, then cable, then monitor
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Box<dyn Processor>>>,
    info: SessionInfo,
}

/// Maps a stream error to the session status it causes.
pub fn status_for(which: Endpoint, err: &cpal::StreamError) -> StreamStatus {
    match err {
        cpal::StreamError::DeviceNotAvailable => StreamStatus::DeviceLost { which },
        _ => StreamStatus::Failed { which },
    }
}

struct Opened {
    device: cpal::Device,
    name: String,
    format: SampleFormat,
    channels: usize,
    rate: u32,
    config: cpal::StreamConfig,
}

fn find<I>(devices: Result<I, cpal::DevicesError>, name: &str) -> Result<cpal::Device, StartError>
where
    I: Iterator<Item = cpal::Device>,
{
    devices
        .ok()
        .and_then(|mut ds| ds.find(|d| d.name().is_ok_and(|n| n == name)))
        .ok_or_else(|| StartError::DeviceNotFound(name.to_string()))
}

fn open(device: cpal::Device, name: &str, input: bool) -> Result<Opened, StartError> {
    let config = if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    };
    let config = config.map_err(|e| StartError::UnsupportedConfig {
        device: name.to_string(),
        reason: e.to_string(),
    })?;
    Ok(Opened {
        device,
        name: name.to_string(),
        format: config.sample_format(),
        channels: usize::from(config.channels()),
        rate: config.sample_rate().0,
        config: config.config(),
    })
}

fn unsupported(name: &str, format: SampleFormat) -> StartError {
    StartError::UnsupportedConfig {
        device: name.to_string(),
        reason: format!("sample format {format:?}"),
    }
}

fn build_error(name: &str, e: impl ToString) -> StartError {
    StartError::BuildStream {
        device: name.to_string(),
        reason: e.to_string(),
    }
}

fn build_input<T>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    mut prod: HeapProd<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let shared_err = shared.clone();
    dev.build_input_stream(
        cfg,
        move |data: &[T], _: &cpal::InputCallbackInfo| ring::push_input(data, &mut prod, &shared),
        move |err| shared_err.report(status_for(Endpoint::Input, &err)),
        None,
    )
}

fn build_output<T>(
    dev: &cpal::Device,
    cfg: &cpal::StreamConfig,
    mut cons: HeapCons<f32>,
    shared: Arc<Shared>,
    which: Endpoint,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(cfg.channels);
    let shared_err = shared.clone();
    dev.build_output_stream(
        cfg,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            ring::fill_output(data, channels, &mut cons, &shared)
        },
        move |err| shared_err.report(status_for(which, &err)),
        None,
    )
}

fn input_stream(
    o: &Opened,
    prod: HeapProd<f32>,
    shared: &Arc<Shared>,
) -> Result<cpal::Stream, StartError> {
    let shared = shared.clone();
    match o.format {
        SampleFormat::I16 => build_input::<i16>(&o.device, &o.config, prod, shared),
        SampleFormat::U16 => build_input::<u16>(&o.device, &o.config, prod, shared),
        SampleFormat::F32 => build_input::<f32>(&o.device, &o.config, prod, shared),
        other => return Err(unsupported(&o.name, other)),
    }
    .map_err(|e| build_error(&o.name, e))
}

fn output_stream(
    o: &Opened,
    cons: HeapCons<f32>,
    shared: &Arc<Shared>,
    which: Endpoint,
) -> Result<cpal::Stream, StartError> {
    let shared = shared.clone();
    match o.format {
        SampleFormat::I16 => build_output::<i16>(&o.device, &o.config, cons, shared, which),
        SampleFormat::U16 => build_output::<u16>(&o.device, &o.config, cons, shared, which),
        SampleFormat::F32 => build_output::<f32>(&o.device, &o.config, cons, shared, which),
        other => return Err(unsupported(&o.name, other)),
    }
    .map_err(|e| build_error(&o.name, e))
}

/// The input device and the outputs (cable first, then the monitor if it is on).
type OpenedDevices = (Opened, Vec<(OutputKind, Endpoint, Opened)>);

fn open_devices(sel: &Selection) -> Result<OpenedDevices, StartError> {
    let host = cpal::host_from_id(cpal::HostId::Wasapi).map_err(|e| build_error("WASAPI", e))?;

    let input_dev = find(host.input_devices(), &sel.input)?;
    let cable_dev = find(host.output_devices(), &sel.cable)?;
    let monitor_dev = match (&sel.monitor, sel.monitor_enabled) {
        (Some(name), true) => Some((find(host.output_devices(), name)?, name)),
        _ => None,
    };

    let input = open(input_dev, &sel.input, true)?;
    let mut outputs = vec![(
        OutputKind::Cable,
        Endpoint::Cable,
        open(cable_dev, &sel.cable, false)?,
    )];
    if let Some((dev, name)) = monitor_dev {
        let monitor = open(dev, name, false)?;
        outputs.push((OutputKind::Monitor, Endpoint::Monitor, monitor));
    }
    Ok((input, outputs))
}

/// Why `AudioSession::start` failed, with the processor when it could be recovered.
pub type StartFailure = (StartError, Option<Box<dyn Processor>>);

impl AudioSession {
    /// Opens the selected WASAPI devices at their default formats, starts the processing worker
    /// with `processor` and plays every stream. Resets `shared` first. On failure the processor
    /// comes back unless the worker thread could not be spawned.
    pub fn start(
        sel: &Selection,
        shared: Arc<Shared>,
        processor: Box<dyn Processor>,
    ) -> Result<AudioSession, StartFailure> {
        let (input, outputs) = match open_devices(sel) {
            Ok(devices) => devices,
            Err(e) => return Err((e, Some(processor))),
        };

        shared.reset();

        let (in_prod, in_cons) = ring::ring(input.rate as usize * input.channels);
        let mut out_prods = Vec::with_capacity(outputs.len());
        let mut out_conses = Vec::with_capacity(outputs.len());
        let mut max_backlogs = Vec::with_capacity(outputs.len());
        for (_, _, o) in &outputs {
            let (prod, cons) = ring::ring(o.rate as usize);
            out_prods.push(prod);
            out_conses.push(cons);
            max_backlogs.push(o.rate as usize / 5);
        }

        let proc_rate = processor.out_rate();
        let state = State::new(
            &WorkerConfig {
                in_rate: input.rate,
                in_channels: input.channels,
                proc_rate,
                outputs: outputs
                    .iter()
                    .map(|(kind, _, o)| OutputConfig {
                        kind: *kind,
                        rate: o.rate,
                    })
                    .collect(),
            },
            processor,
        )
        .map_err(|(reason, p)| {
            let e = StartError::UnsupportedConfig {
                device: sel.input.clone(),
                reason,
            };
            (e, Some(p))
        })?;

        // Built before the worker is spawned, so a failure still gives the processor back.
        let streams = match build_streams(&input, &outputs, in_prod, out_conses, &shared) {
            Ok(streams) => streams,
            Err(e) => return Err((e, Some(state.into_processor()))),
        };

        let stop = Arc::new(AtomicBool::new(false));
        let worker = worker::spawn(
            state,
            in_cons,
            out_prods,
            max_backlogs,
            shared.clone(),
            stop.clone(),
        )
        .map_err(|e| (build_error("processing thread", e), None))?;

        let info = SessionInfo {
            input_rate: input.rate,
            cable_rate: outputs[0].2.rate,
            monitor_rate: outputs.get(1).map(|(_, _, o)| o.rate),
        };
        // Built before playing so shutdown stops the streams and joins the worker if play fails.
        let mut session = AudioSession {
            streams,
            stop,
            worker: Some(worker),
            info,
        };
        let names = std::iter::once(&input.name).chain(outputs.iter().map(|(_, _, o)| &o.name));
        let play_error = session
            .streams
            .iter()
            .zip(names)
            .find_map(|(stream, name)| stream.play().err().map(|e| build_error(name, e)));
        if let Some(e) = play_error {
            let p = session.shutdown();
            return Err((e, p));
        }
        Ok(session)
    }

    /// Sample rates of the session.
    pub fn info(&self) -> SessionInfo {
        self.info
    }

    /// Stops the session and gives the processor back (`None` if the worker panicked).
    pub fn stop(mut self) -> Option<Box<dyn Processor>> {
        self.shutdown()
    }

    /// Pauses and drops the streams, then stops and joins the worker. Idempotent.
    fn shutdown(&mut self) -> Option<Box<dyn Processor>> {
        for s in &self.streams {
            let _ = s.pause();
        }
        self.streams.clear(); // drops the cpal streams (callbacks stop)
        self.stop.store(true, Relaxed);
        self.worker.take().and_then(|h| h.join().ok())
    }
}

impl Drop for AudioSession {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// Builds the input stream, then one output stream per output (same order).
fn build_streams(
    input: &Opened,
    outputs: &[(OutputKind, Endpoint, Opened)],
    in_prod: HeapProd<f32>,
    out_conses: Vec<HeapCons<f32>>,
    shared: &Arc<Shared>,
) -> Result<Vec<cpal::Stream>, StartError> {
    let mut streams = Vec::with_capacity(1 + outputs.len());
    streams.push(input_stream(input, in_prod, shared)?);
    for ((_, which, o), cons) in outputs.iter().zip(out_conses) {
        streams.push(output_stream(o, cons, shared, *which)?);
    }
    Ok(streams)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::devices::Selection;
    use crate::processor::Passthrough;
    use crate::shared::{Endpoint, Shared, StreamStatus};

    #[test]
    fn status_for_device_not_available() {
        assert_eq!(
            status_for(Endpoint::Monitor, &cpal::StreamError::DeviceNotAvailable),
            StreamStatus::DeviceLost {
                which: Endpoint::Monitor
            }
        );
    }

    #[test]
    fn status_for_backend_error() {
        let err = cpal::StreamError::BackendSpecific {
            err: cpal::BackendSpecificError {
                description: "x".into(),
            },
        };
        assert_eq!(
            status_for(Endpoint::Cable, &err),
            StreamStatus::Failed {
                which: Endpoint::Cable
            }
        );
    }

    #[test]
    fn start_unknown_input_is_device_not_found() {
        let sel = Selection {
            input: "No Such Device 7f3a".into(),
            ..Selection::default()
        };
        let processor = Box::new(Passthrough::new(48_000));
        match AudioSession::start(&sel, Arc::new(Shared::new()), processor) {
            Err((StartError::DeviceNotFound(name), Some(_))) => {
                assert_eq!(name, "No Such Device 7f3a")
            }
            Err((e, p)) => panic!("unexpected error: {e} (processor back: {})", p.is_some()),
            Ok(_) => panic!("started with an unknown device"),
        }
    }

    #[test]
    fn start_error_messages() {
        assert_eq!(
            StartError::DeviceNotFound("Mic".into()).to_string(),
            "device \"Mic\" was not found"
        );
        assert_eq!(
            StartError::UnsupportedConfig {
                device: "Mic".into(),
                reason: "sample format I8".into()
            }
            .to_string(),
            "device \"Mic\" has an unsupported format: sample format I8"
        );
        assert_eq!(
            StartError::BuildStream {
                device: "Mic".into(),
                reason: "busy".into()
            }
            .to_string(),
            "could not start \"Mic\": busy"
        );
    }
}
