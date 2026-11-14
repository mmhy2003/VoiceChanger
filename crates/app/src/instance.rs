//! Single instance: a named mutex says a copy is running, and a named event asks that copy to
//! show its window. Both names are per Windows session.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SetEvent,
    WaitForSingleObject,
};

/// The name every copy of the app uses.
pub const APP_NAME: &str = "VoiceChanger";

/// What `acquire` found.
pub enum Instance {
    /// No other copy is running. Keep the guard until exit.
    Primary(Primary),
    /// Another copy is running and was asked to show its window.
    Secondary,
}

/// The running copy's guard: it holds the mutex and hears later launches.
pub struct Primary {
    _mutex: OwnedHandle,
    show: Arc<OwnedHandle>,
    requested: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
}

/// `Local\` keeps the object inside the current Windows session.
fn object_name(name: &str, kind: &str) -> Vec<u16> {
    format!("Local\\{name}.{kind}")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// Takes ownership of a handle from a Create or Open call; null means the call failed.
fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle is valid and nothing else owns it.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// Claims `name` for this process, or signals the copy that already has it.
pub fn acquire(name: &str) -> io::Result<Instance> {
    let (mutex_name, show_name) = (object_name(name, "instance"), object_name(name, "show"));
    // SAFETY: the name is NUL-terminated and outlives the call.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
    // Read at once: any later call may change it.
    // SAFETY: no arguments.
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let mutex = owned(handle)?;
    if exists {
        // The other copy may be starting or exiting: then there is no event to signal.
        // SAFETY: as above.
        if let Ok(show) = owned(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, show_name.as_ptr()) }) {
            // SAFETY: `show` is an open event handle.
            unsafe { SetEvent(show.as_raw_handle()) };
        }
        return Ok(Instance::Secondary);
    }
    // Auto-reset and not signalled.
    // SAFETY: as above.
    let show = owned(unsafe { CreateEventW(std::ptr::null(), 0, 0, show_name.as_ptr()) })?;
    Ok(Instance::Primary(Primary {
        _mutex: mutex,
        show: Arc::new(show),
        requested: Arc::default(),
        stop: Arc::default(),
        listener: None,
    }))
}

impl Primary {
    /// Starts the "vc-instance" thread. Each later launch sets the flag read by `take_show`
    /// and calls `wake`.
    pub fn listen(&mut self, wake: impl Fn() + Send + 'static) -> io::Result<()> {
        let (show, requested, stop) = (
            Arc::clone(&self.show),
            Arc::clone(&self.requested),
            Arc::clone(&self.stop),
        );
        let listener = thread::Builder::new()
            .name("vc-instance".into())
            .spawn(move || {
                loop {
                    // SAFETY: the thread keeps the event handle open.
                    let wait = unsafe { WaitForSingleObject(show.as_raw_handle(), INFINITE) };
                    if wait != WAIT_OBJECT_0 || stop.load(Ordering::Relaxed) {
                        break;
                    }
                    requested.store(true, Ordering::Relaxed);
                    wake();
                }
            })?;
        self.listener = Some(listener);
        Ok(())
    }

    /// Whether another copy was launched since the last call.
    pub fn take_show(&self) -> bool {
        self.requested.swap(false, Ordering::Relaxed)
    }
}

impl Drop for Primary {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.take() {
            self.stop.store(true, Ordering::Relaxed);
            // SAFETY: the event handle is open until `self` is gone.
            unsafe { SetEvent(self.show.as_raw_handle()) };
            let _ = listener.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// A name no running VoiceChanger or other test uses.
    fn name(label: &str) -> String {
        format!("VoiceChanger.test.{}.{label}", std::process::id())
    }

    #[test]
    fn first_acquire_is_primary() {
        assert!(matches!(
            acquire(&name("first")).unwrap(),
            Instance::Primary(_)
        ));
    }

    #[test]
    fn second_acquire_is_secondary_and_signals_the_primary() {
        let name = name("second");
        let Instance::Primary(mut primary) = acquire(&name).unwrap() else {
            panic!("the first acquire must be primary");
        };
        let (tx, rx) = mpsc::channel();
        primary
            .listen(move || {
                let _ = tx.send(());
            })
            .unwrap();
        assert!(!primary.take_show());
        assert!(matches!(acquire(&name).unwrap(), Instance::Secondary));
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the primary was not signalled");
        assert!(primary.take_show());
        assert!(!primary.take_show());
    }

    #[test]
    fn primary_again_after_drop() {
        let name = name("again");
        let first = acquire(&name).unwrap();
        assert!(matches!(first, Instance::Primary(_)));
        assert!(matches!(acquire(&name).unwrap(), Instance::Secondary));
        drop(first);
        assert!(matches!(acquire(&name).unwrap(), Instance::Primary(_)));
    }
}
