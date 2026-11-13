//! Keeps COM up for the whole process once audio is used.
//!
//! cpal creates one WASAPI device enumerator per process, on the first thread that touches it, and
//! its per-thread COM guard shuts COM down when that thread exits. If no other thread holds COM,
//! the enumerator is left pointing at freed state and the next cpal call can crash.

/// Holds the multithreaded apartment open until the process exits. Call before any cpal call.
#[cfg(windows)]
pub fn keep_alive() {
    use std::sync::Once;
    use windows::Win32::System::Com::CoIncrementMTAUsage;

    static KEEP: Once = Once::new();
    KEEP.call_once(|| {
        // SAFETY: no arguments; the cookie is never released, so COM stays up.
        if let Err(e) = unsafe { CoIncrementMTAUsage() } {
            tracing::warn!(%e, "could not keep COM initialised");
        }
    });
}

#[cfg(not(windows))]
pub fn keep_alive() {}
