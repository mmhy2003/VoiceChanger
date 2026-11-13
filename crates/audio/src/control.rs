//! The audio-control thread: every cpal call and the running session live here, off the UI thread.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SendError, Sender};
use std::thread::{self, JoinHandle};

use crate::devices::{DeviceList, Selection, list_devices};
use crate::processor::Processor;
use crate::session::{AudioSession, SessionInfo, StartError, StartFailure};
use crate::shared::Shared;

enum Request {
    List(Sender<DeviceList>),
    Start {
        selection: Selection,
        processor: Box<dyn Processor>,
        reply: Sender<Result<SessionInfo, StartFailure>>,
    },
    Stop(Sender<Option<Box<dyn Processor>>>),
    Shutdown,
}

/// Handle to the `vc-audio-ctl` thread, which makes every cpal call and owns the running session.
/// Dropping it stops any session and joins the thread.
pub struct AudioControl {
    tx: Sender<Request>,
    thread: Option<JoinHandle<()>>,
}

impl AudioControl {
    /// Spawns `vc-audio-ctl`. If it cannot be spawned, the control is dead from the start.
    pub fn spawn(shared: Arc<Shared>) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("vc-audio-ctl".into())
            .spawn(move || run(rx, shared))
            .inspect_err(|e| tracing::error!(%e, "could not start the audio thread"))
            .ok();
        Self { tx, thread }
    }

    /// Whether the thread is running.
    pub fn is_alive(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Lists the devices on the thread. `None` if the thread is not running.
    pub fn list(&self) -> Option<DeviceList> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Request::List(tx)).ok()?;
        rx.recv().ok()
    }

    /// Starts a session on the thread. On failure the processor comes back when it could be
    /// recovered, including when the thread is not running.
    pub fn start(
        &self,
        selection: &Selection,
        processor: Box<dyn Processor>,
    ) -> Result<SessionInfo, StartFailure> {
        let (reply, rx) = mpsc::channel();
        let req = Request::Start {
            selection: selection.clone(),
            processor,
            reply,
        };
        if let Err(SendError(Request::Start { processor, .. })) = self.tx.send(req) {
            return Err((dead(), Some(processor)));
        }
        rx.recv().unwrap_or_else(|_| Err((dead(), None)))
    }

    /// Stops the running session and gives the processor back. `None` if there was no session,
    /// the worker panicked or the thread is not running.
    pub fn stop(&self) -> Option<Box<dyn Processor>> {
        let (tx, rx) = mpsc::channel();
        self.tx.send(Request::Stop(tx)).ok()?;
        rx.recv().ok().flatten()
    }

    /// Stops any session and joins the thread. Idempotent.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Request::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for AudioControl {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn dead() -> StartError {
    StartError::BuildStream {
        device: "audio control thread".into(),
        reason: "not running".into(),
    }
}

fn run(rx: Receiver<Request>, shared: Arc<Shared>) {
    let mut session: Option<AudioSession> = None;
    while let Ok(req) = rx.recv() {
        match req {
            Request::List(reply) => {
                let _ = reply.send(list_devices());
            }
            Request::Start {
                selection,
                processor,
                reply,
            } => {
                let result = if session.is_some() {
                    let e = StartError::BuildStream {
                        device: "audio session".into(),
                        reason: "already running".into(),
                    };
                    Err((e, Some(processor)))
                } else {
                    AudioSession::start(&selection, shared.clone(), processor).map(|s| {
                        let info = s.info();
                        session = Some(s);
                        info
                    })
                };
                let _ = reply.send(result);
            }
            Request::Stop(reply) => {
                let _ = reply.send(session.take().and_then(AudioSession::stop));
            }
            Request::Shutdown => break,
        }
    }
} // dropping `session` stops the streams

#[cfg(test)]
mod tests {
    use super::*;

    use crate::devices::DeviceRef;
    use crate::processor::Passthrough;
    use crate::session::StartError;

    fn control() -> AudioControl {
        AudioControl::spawn(Arc::new(Shared::new()))
    }

    fn unknown_input() -> Selection {
        Selection {
            input: DeviceRef {
                id: String::new(),
                name: "No Such Device 7f3a".into(),
            },
            ..Selection::default()
        }
    }

    #[test]
    fn list_goes_through_thread() {
        assert!(control().list().is_some());
    }

    #[test]
    fn stop_without_session_is_none() {
        assert!(control().stop().is_none());
    }

    #[test]
    fn start_unknown_device_returns_processor() {
        let processor = Box::new(Passthrough::new(48_000));
        match control().start(&unknown_input(), processor) {
            Err((StartError::DeviceNotFound(name), Some(_))) => {
                assert_eq!(name, "No Such Device 7f3a")
            }
            Err((e, p)) => panic!("unexpected error: {e} (processor back: {})", p.is_some()),
            Ok(_) => panic!("started with an unknown device"),
        }
    }

    #[test]
    fn shutdown_joins_and_kills() {
        let mut ctl = control();
        ctl.shutdown();
        assert!(!ctl.is_alive());
        assert!(ctl.list().is_none());
        let processor = Box::new(Passthrough::new(48_000));
        match ctl.start(&unknown_input(), processor) {
            Err((StartError::BuildStream { device, reason }, Some(_))) => {
                assert_eq!(device, "audio control thread");
                assert_eq!(reason, "not running");
            }
            Err((e, p)) => panic!("unexpected error: {e} (processor back: {})", p.is_some()),
            Ok(_) => panic!("started after shutdown"),
        }
    }

    #[test]
    fn shutdown_twice_is_fine() {
        let mut ctl = control();
        ctl.shutdown();
        ctl.shutdown();
        assert!(!ctl.is_alive());
    }
}
