# VC-121 plan: system tray icon

## Goal
Closing the VoiceChanger window hides it to a system tray icon (menu: Open VoiceChanger, Start, Stop, Exit; green dot while running) while the app keeps converting, only one copy ever runs, and only Exit, the updater, an installer or Windows really close it.

## Architecture
Three new modules in `crates/app/src/`, each with its pure rules apart from the OS calls, as in `hotkey.rs`:
- `instance.rs`: a named mutex and a named "show" event (both `Local\`, so per Windows session).
- `window.rs`: `Waker` (posts `WM_PAINT` to the hidden window so eframe runs a frame), the `vc-wake` ticker and the end-session subclass, bundled as `MainWindow`.
- `tray.rs`: the pure rules (`close_action`, `tray_state`, tooltip, click mapping, the two 32 px icons) and `Tray`, which owns the `tray-icon` icon and its menu.

`VcApp::update` stays the only place with app logic: it polls tray commands, the "show" signal and the end-session flag, applies the close rule and syncs the tray once per frame.

## Facts checked in the code (repo at `837f6a1`, registry sources)
- Package `voicechanger` (`crates/app`), edition 2024, CI runs `cargo clippy --all-targets --all-features -- -D warnings`. It is a binary crate, so an unused item is a `dead_code` error: every step below wires what it adds, except step 4, which uses `#![cfg_attr(not(test), expect(dead_code, reason = ...))]` for one step.
- Locked: `eframe`/`egui`/`egui-winit` 0.32.3 (glow renderer), `winit` 0.30.13, `windows-sys` 0.61.2, `raw-window-handle` 0.6.2, `image` 0.25.10, `global-hotkey` 0.7.0. In the local registry: `tray-icon` 0.26.1 and `muda` 0.21.2 (both need `windows-sys >=0.60, <=0.61`).
- `winit` `platform_impl/windows/event_loop.rs`: `WM_PAINT` sends `WindowEvent::RedrawRequested`; `request_redraw` is `RedrawWindow(RDW_INTERNALPAINT)`, which gives a hidden window nothing.
- `eframe` `native/epi_integration.rs`: a close request closes the app unless the frame sent `ViewportCommand::CancelClose`; `post_rendering` calls `window.set_visible(true)` after the first frame, and viewport commands are processed after it. `ViewportCommand::Close` only queues a close event, so it needs one more frame.
- `eframe::CreationContext` implements `raw_window_handle::HasWindowHandle`.
- `tray-icon`: `TrayIconEvent::set_event_handler` and `MenuEvent::set_event_handler` take `Fn + Send + Sync` and can be set once per process; `TrayIcon` and `MenuItem` are not `Send` (fine: `VcApp` lives on the UI thread); dropping `TrayIcon` removes the icon; it re-adds the icon on `TaskbarCreated`; the tooltip buffer is 128 UTF-16 units.
- `windows-sys` 0.61.2: `SetWindowSubclass`/`DefSubclassProc`/`RemoveWindowSubclass` are in `Win32::UI::Shell`; `CreateMutexW`/`CreateEventW`/`OpenEventW`/`SetEvent`/`WaitForSingleObject` in `Win32::System::Threading`; `HWND` and `HANDLE` are `*mut c_void`.

## Finding since the design (no behaviour change)
`eframe` `native/run.rs` `check_redraw_requests` switches the event loop to `ControlFlow::Poll` when a requested repaint is due and relies on the paint arriving to go back to `Wait`. A hidden window never gets that paint, so every egui repaint request made while hidden would leave the UI thread in a busy loop until the next posted paint. The plan keeps the design and adds three rules, all in step 5:
1. While `in_tray`, `update` does its polling but draws no UI and makes no `request_repaint_after` call. The ticker drives the frames.
2. Every wake from another thread goes through `Waker::wake()` (hotkey, second launch, and the updater's callback), so a posted paint always follows.
3. `hide_to_tray` posts one paint, so a hidden frame follows at once and returns the loop to `Wait`.
What is left is bounded: a stray request spins for at most one tick (0.5 s).

## File map
| File | Change | Responsibility |
|---|---|---|
| `crates/app/src/instance.rs` | new | `acquire(name) -> io::Result<Instance>`, `Primary::listen`, `Primary::take_show` |
| `crates/app/src/window.rs` | new | `hwnd_of`, `Waker`, `MainWindow` (ticker `vc-wake`, end-session watch) |
| `crates/app/src/tray.rs` | new | `TrayCommand`, `close_action`, `tray_state`, `tooltip`, click and menu mapping, `icon_rgba`, `Tray` |
| `crates/app/src/main.rs` | modify | `mod` lines, `ICON_PNG`, the single-instance check after `--self-test`, `Startup.instance` |
| `crates/app/src/app.rs` | modify | new fields, `poll_tray`, `handle_close`, `show_window`, `hide_to_tray`, `quit`, `sync_tray`, `draw`, `start_blocked`, `repaint_after`, `on_exit` |
| `crates/app/src/hotkey.rs` | modify | `Hotkeys::new(waker: Waker)` |
| `crates/app/src/tabs/settings.rs` | modify | checkbox label "Start in the tray" |
| `crates/app/Cargo.toml` | modify | `tray-icon`, `raw-window-handle`, dev `winit`, `windows-sys` features |
| `Cargo.lock` | modify | by cargo |
| `CHANGELOG.md`, `README.md` | modify | Unreleased → Added entry; one README paragraph |

`settings.toml`, `CONFIG_VERSION`, `installer/` and `deny.toml` do not change.

## Reference code
The code below was written against the sources listed above but not compiled. Where the compiler disagrees, keep the names, signatures and tests and fix the body.

### R1. `crates/app/src/instance.rs` (step 1)
```rust
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
        assert!(matches!(acquire(&name("first")).unwrap(), Instance::Primary(_)));
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
```

### R2. `crates/app/src/window.rs`, first part (step 2)
```rust
//! Win32 glue for the main window: waking it while it is hidden.

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{IsWindowVisible, PostMessageW, WM_PAINT};

/// The Win32 handle of `window`, as an integer so it can cross threads.
pub fn hwnd_of(window: &impl HasWindowHandle) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Makes eframe run a frame, also while the window is hidden.
#[derive(Clone)]
pub struct Waker {
    ctx: egui::Context,
    hwnd: Option<isize>,
}

impl Waker {
    pub fn new(ctx: &egui::Context, hwnd: Option<isize>) -> Self {
        Self {
            ctx: ctx.clone(),
            hwnd,
        }
    }

    /// Repaints. Windows sends a hidden window no paint message, so one is posted: winit turns
    /// it into `RedrawRequested` and eframe runs a normal frame.
    pub fn wake(&self) {
        self.ctx.request_repaint();
        self.wake_if_hidden();
    }

    /// Posts the paint message only while the window is hidden.
    pub fn wake_if_hidden(&self) {
        let Some(hwnd) = self.hwnd else {
            return;
        };
        // SAFETY: takes any handle value, also one whose window is gone.
        if unsafe { IsWindowVisible(hwnd as HWND) } == 0 {
            self.post_paint();
        }
    }

    /// Posts the paint message whatever the window's state.
    pub fn post_paint(&self) {
        if let Some(hwnd) = self.hwnd {
            // SAFETY: takes any handle value, also one whose window is gone.
            unsafe { PostMessageW(hwnd as HWND, WM_PAINT, 0, 0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, MSG, PM_QS_POSTMESSAGE, PM_REMOVE, PeekMessageW,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    use super::*;

    /// A 1x1 top-level window of the system "STATIC" class, owned by the test's thread.
    struct TestWindow(HWND);

    impl TestWindow {
        fn new(visible: bool) -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let style = if visible { WS_POPUP | WS_VISIBLE } else { WS_POPUP };
            // SAFETY: the class name is NUL-terminated and outlives the call; the other
            // arguments are null or plain numbers.
            let hwnd = unsafe {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class.as_ptr(),
                    null(),
                    style,
                    0,
                    0,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    null_mut(),
                    null(),
                )
            };
            assert!(!hwnd.is_null(), "CreateWindowExW: {}", io::Error::last_os_error());
            Self(hwnd)
        }

        fn id(&self) -> Option<isize> {
            Some(self.0 as isize)
        }

        /// Removes one posted paint message; `false` when there is none. Posted messages only,
        /// so the paint Windows owes a visible window does not count.
        fn take_posted_paint(&self) -> bool {
            // SAFETY: MSG is a plain C struct and outlives the call.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                PeekMessageW(&mut msg, self.0, WM_PAINT, WM_PAINT, PM_REMOVE | PM_QS_POSTMESSAGE)
                    != 0
            }
        }
    }

    impl Drop for TestWindow {
        fn drop(&mut self) {
            // SAFETY: the window belongs to this thread.
            unsafe { DestroyWindow(self.0) };
        }
    }

    fn waker(window: &TestWindow) -> Waker {
        Waker::new(&egui::Context::default(), window.id())
    }

    #[test]
    fn wake_posts_paint_to_a_hidden_window() {
        let window = TestWindow::new(false);
        waker(&window).wake();
        assert!(window.take_posted_paint());
        assert!(!window.take_posted_paint(), "one wake posts one message");
    }

    #[test]
    fn wake_does_not_post_to_a_visible_window() {
        let window = TestWindow::new(true);
        waker(&window).wake();
        assert!(!window.take_posted_paint());
    }

    #[test]
    fn wake_without_a_window_does_nothing() {
        Waker::new(&egui::Context::default(), None).wake();
    }

    /// Fails if a winit or eframe upgrade stops turning the posted paint message into
    /// `RedrawRequested`, which is what runs `VcApp::update` in the tray.
    /// winit allows one event loop per process: keep this the only test that makes one.
    #[test]
    fn hidden_winit_window_redraws_on_wake() {
        use winit::application::ApplicationHandler;
        use winit::event::WindowEvent;
        use winit::event_loop::{ActiveEventLoop, EventLoop};
        use winit::platform::pump_events::EventLoopExtPumpEvents;
        use winit::platform::windows::EventLoopBuilderExtWindows;
        use winit::window::{Window, WindowId};

        #[derive(Default)]
        struct Probe {
            window: Option<Window>,
            redraws: u32,
        }

        impl ApplicationHandler for Probe {
            fn resumed(&mut self, event_loop: &ActiveEventLoop) {
                if self.window.is_none() {
                    let hidden = Window::default_attributes().with_visible(false);
                    self.window = Some(event_loop.create_window(hidden).expect("a hidden window"));
                }
            }

            fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
                if matches!(event, WindowEvent::RedrawRequested) {
                    self.redraws += 1;
                }
            }
        }

        let mut event_loop = EventLoop::builder()
            .with_any_thread(true)
            .build()
            .expect("an event loop");
        let mut probe = Probe::default();
        let mut pump = |probe: &mut Probe| {
            let _ = event_loop.pump_app_events(Some(Duration::from_millis(20)), probe);
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while probe.window.is_none() {
            assert!(Instant::now() < deadline, "the window was not created");
            pump(&mut probe);
        }
        // Let the start-up events pass.
        for _ in 0..10 {
            pump(&mut probe);
        }
        let before = probe.redraws;
        let hwnd = hwnd_of(probe.window.as_ref().expect("the window"));
        assert!(hwnd.is_some(), "winit gave no Win32 handle");
        Waker::new(&egui::Context::default(), hwnd).wake();
        let deadline = Instant::now() + Duration::from_secs(5);
        while probe.redraws == before {
            assert!(
                Instant::now() < deadline,
                "wake() did not reach the hidden window as RedrawRequested"
            );
            pump(&mut probe);
        }
    }
}
```

### R3. `crates/app/src/window.rs`, second part (step 3)
Module doc becomes `//! Win32 glue for the main window: waking it while it is hidden, a ticker that keeps the app polling in the tray, and noticing that Windows is ending the session.` Add to the imports: `std::io`, `std::sync::Arc`, `std::sync::atomic::{AtomicBool, Ordering}`, `std::thread::{self, JoinHandle}`, `std::time::Duration`, `LPARAM, LRESULT, WPARAM` from `Foundation`, `DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass` from `Win32::UI::Shell`, and `ENDSESSION_CLOSEAPP, WM_ENDSESSION, WM_NCDESTROY, WM_QUERYENDSESSION` from `WindowsAndMessaging`.
```rust
/// How often the hidden window is woken.
const TICK: Duration = Duration::from_millis(500);
/// Identifies the end-session subclass on the window ("VC").
const SUBCLASS_ID: usize = 0x5643;

/// Whether a window message means Windows wants the app to end: the session is ending, or
/// Restart Manager (an installer) asks the app to close.
fn ends_session(msg: u32, wparam: WPARAM, lparam: LPARAM) -> bool {
    match msg {
        WM_ENDSESSION => wparam != 0,
        WM_QUERYENDSESSION => (lparam as u32) & ENDSESSION_CLOSEAPP != 0,
        _ => false,
    }
}

/// Shared with the window subclass: set when Windows asks the app to end.
struct EndSession {
    requested: AtomicBool,
    waker: Waker,
}

unsafe extern "system" fn end_session_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let watch = data as *const EndSession;
    if ends_session(msg, wparam, lparam) {
        // SAFETY: `data` is the reference `watch_end_session` gave the subclass; it is only
        // released on WM_NCDESTROY below.
        let watch = unsafe { &*watch };
        watch.requested.store(true, Ordering::Relaxed);
        watch.waker.wake();
    }
    if msg == WM_NCDESTROY {
        // SAFETY: the window is going away: remove the subclass and release its reference.
        unsafe {
            RemoveWindowSubclass(hwnd, Some(end_session_proc), SUBCLASS_ID);
            drop(Arc::from_raw(watch));
        }
    }
    // SAFETY: passes the unchanged message to the next handler.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// Subclasses the window. Call on the thread that owns it.
fn watch_end_session(hwnd: isize, waker: Waker) -> Option<Arc<EndSession>> {
    let watch = Arc::new(EndSession {
        requested: AtomicBool::new(false),
        waker,
    });
    let data = Arc::into_raw(Arc::clone(&watch));
    // SAFETY: `data` stays valid until `end_session_proc` releases it on WM_NCDESTROY.
    let installed =
        unsafe { SetWindowSubclass(hwnd as HWND, Some(end_session_proc), SUBCLASS_ID, data as usize) };
    if installed == 0 {
        // SAFETY: the subclass was not installed, so the reference made above is still ours.
        drop(unsafe { Arc::from_raw(data) });
        tracing::warn!("cannot watch for the end of the Windows session");
        return None;
    }
    Some(watch)
}

/// The "vc-wake" thread: wakes the hidden window every `period`.
struct Ticker {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl Ticker {
    fn spawn(waker: Waker, period: Duration) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = thread::Builder::new().name("vc-wake".into()).spawn(move || {
            loop {
                thread::park_timeout(period);
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                waker.wake_if_hidden();
            }
        })?;
        Ok(Self { stop, thread })
    }

    fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.thread().unpark();
        let _ = self.thread.join();
    }
}

/// The main window's helper: the waker, the ticker and the end-session watch.
pub struct MainWindow {
    waker: Waker,
    end_session: Option<Arc<EndSession>>,
    ticker: Option<Ticker>,
}

impl MainWindow {
    /// Call on the UI thread, which owns the window. Without a handle nothing is posted or
    /// watched.
    pub fn new(ctx: &egui::Context, hwnd: Option<isize>) -> Self {
        let waker = Waker::new(ctx, hwnd);
        let end_session = hwnd.and_then(|hwnd| watch_end_session(hwnd, waker.clone()));
        let ticker = hwnd.and_then(|_| {
            Ticker::spawn(waker.clone(), TICK)
                .inspect_err(|e| tracing::error!("cannot start the wake-up thread: {e}"))
                .ok()
        });
        Self {
            waker,
            end_session,
            ticker,
        }
    }

    pub fn waker(&self) -> Waker {
        self.waker.clone()
    }

    /// Whether Windows asked the app to end since the last call.
    pub fn take_end_session(&self) -> bool {
        self.end_session
            .as_ref()
            .is_some_and(|watch| watch.requested.swap(false, Ordering::Relaxed))
    }

    /// Stops the ticker.
    pub fn stop(&mut self) {
        if let Some(ticker) = self.ticker.take() {
            ticker.stop();
        }
    }
}
```
Step 5 adds to `impl MainWindow`:
```rust
    /// Whether the window may be hidden: only when the ticker can keep the app running.
    pub fn can_hide(&self) -> bool {
        self.ticker.is_some()
    }
```
Tests added in step 3 (same `tests` module; add `SendMessageW` to its imports):
```rust
    #[test]
    fn ends_session_cases() {
        assert!(ends_session(WM_ENDSESSION, 1, 0));
        assert!(!ends_session(WM_ENDSESSION, 0, 0), "the shutdown was cancelled");
        assert!(ends_session(WM_QUERYENDSESSION, 0, ENDSESSION_CLOSEAPP as LPARAM));
        assert!(!ends_session(WM_QUERYENDSESSION, 0, 0), "an ordinary shutdown query");
        assert!(!ends_session(WM_PAINT, 1, ENDSESSION_CLOSEAPP as LPARAM));
    }

    #[test]
    fn end_session_watch_sets_its_flag() {
        let window = TestWindow::new(false);
        let main = MainWindow::new(&egui::Context::default(), window.id());
        let send = |msg: u32, wparam: WPARAM, lparam: LPARAM| {
            // SAFETY: the window belongs to this thread.
            unsafe { SendMessageW(window.0, msg, wparam, lparam) };
        };
        assert!(!main.take_end_session());
        send(WM_QUERYENDSESSION, 0, 0);
        assert!(!main.take_end_session(), "an ordinary shutdown query");
        send(WM_QUERYENDSESSION, 0, ENDSESSION_CLOSEAPP as LPARAM);
        assert!(main.take_end_session());
        assert!(!main.take_end_session(), "the flag is taken once");
        send(WM_ENDSESSION, 1, 0);
        assert!(main.take_end_session());
    }

    #[test]
    fn ticker_wakes_a_hidden_window_until_stopped() {
        let window = TestWindow::new(false);
        let ticker = Ticker::spawn(waker(&window), Duration::from_millis(10)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !window.take_posted_paint() {
            assert!(Instant::now() < deadline, "the ticker posted nothing");
            std::thread::sleep(Duration::from_millis(5));
        }
        ticker.stop();
        while window.take_posted_paint() {}
        std::thread::sleep(Duration::from_millis(50));
        assert!(!window.take_posted_paint(), "the ticker went on after stop");
    }
```

### R4. `crates/app/src/tray.rs`, the rules (step 4)
```rust
//! The system tray icon: its menu, its two icons, and the rules for what a click and a close do.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the tray icon in the next VC-121 step")
)]

use image::imageops::{self, FilterType};
use image::{ImageFormat, Rgba, RgbaImage};
use tray_icon::{MouseButton, MouseButtonState};

/// What the user asked for from the tray.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    Start,
    Stop,
    Exit,
}

const OPEN_ID: &str = "open";
const START_ID: &str = "start";
const STOP_ID: &str = "stop";
const EXIT_ID: &str = "exit";

/// The command of the menu item with `id`.
pub fn command_for_menu(id: &str) -> Option<TrayCommand> {
    match id {
        OPEN_ID => Some(TrayCommand::Open),
        START_ID => Some(TrayCommand::Start),
        STOP_ID => Some(TrayCommand::Stop),
        EXIT_ID => Some(TrayCommand::Exit),
        _ => None,
    }
}

/// Releasing the left button on the icon opens the window; the right button opens the menu,
/// which the tray library does itself.
pub fn command_for_click(button: MouseButton, state: MouseButtonState) -> Option<TrayCommand> {
    (button == MouseButton::Left && state == MouseButtonState::Up).then_some(TrayCommand::Open)
}

/// What a close request does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    Exit,
    Hide,
}

/// The window's close button hides the window to the tray. The app exits when it is quitting
/// (tray Exit, the updater, Windows or an installer), when the window is already hidden (the
/// request came from outside, e.g. `taskkill`), and when there is no tray icon to bring it back.
pub fn close_action(quitting: bool, in_tray: bool, tray_available: bool) -> CloseAction {
    if quitting || in_tray || !tray_available {
        CloseAction::Exit
    } else {
        CloseAction::Hide
    }
}

/// What the tray shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayState {
    /// The green dot, and Stop is enabled.
    pub running: bool,
    pub start_enabled: bool,
    pub tooltip: String,
}

/// The tray for the app's state. `start_blocked` is whether the Start button has a hint;
/// `model` is the loaded model's name.
pub fn tray_state(running: bool, start_blocked: bool, model: Option<&str>) -> TrayState {
    TrayState {
        running,
        start_enabled: !running && !start_blocked,
        tooltip: tooltip(running, model),
    }
}

/// Windows keeps at most 127 UTF-16 units of a tray tooltip.
const TOOLTIP_MAX: usize = 127;

/// "VoiceChanger: Running (<model>)" or "VoiceChanger: Stopped", cut to what Windows shows.
pub fn tooltip(running: bool, model: Option<&str>) -> String {
    let mut text = match (running, model) {
        (false, _) => "VoiceChanger: Stopped".to_string(),
        (true, Some(name)) => format!("VoiceChanger: Running ({name})"),
        (true, None) => "VoiceChanger: Running".to_string(),
    };
    if text.encode_utf16().count() > TOOLTIP_MAX {
        while text.encode_utf16().count() > TOOLTIP_MAX - 1 {
            text.pop();
        }
        text.push('…');
    }
    text
}

/// Width and height of the tray icon in pixels.
pub const ICON_SIZE: u32 = 32;
/// The running dot.
pub const DOT_COLOUR: Rgba<u8> = Rgba([34, 197, 94, 255]);
const DOT_CENTRE: i32 = 25;
const DOT_RADIUS: i32 = 6;

/// Whether pixel (x, y) of the icon is inside the running dot, at the bottom right.
fn in_dot(x: u32, y: u32) -> bool {
    let (dx, dy) = (x as i32 - DOT_CENTRE, y as i32 - DOT_CENTRE);
    dx * dx + dy * dy <= DOT_RADIUS * DOT_RADIUS
}

/// The tray icon: the logo scaled to 32 px, with a green dot while running.
pub fn icon_rgba(running: bool) -> RgbaImage {
    let logo = image::load_from_memory_with_format(crate::ICON_PNG, ImageFormat::Png)
        .expect("assets/icon-256.png is a valid PNG")
        .into_rgba8();
    let mut icon = imageops::resize(&logo, ICON_SIZE, ICON_SIZE, FilterType::Lanczos3);
    if running {
        for (x, y, pixel) in icon.enumerate_pixels_mut() {
            if in_dot(x, y) {
                *pixel = DOT_COLOUR;
            }
        }
    }
    icon
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_action_every_combination() {
        for quitting in [false, true] {
            for in_tray in [false, true] {
                for tray_available in [false, true] {
                    let expected = if !quitting && !in_tray && tray_available {
                        CloseAction::Hide
                    } else {
                        CloseAction::Exit
                    };
                    assert_eq!(
                        close_action(quitting, in_tray, tray_available),
                        expected,
                        "quitting {quitting}, in_tray {in_tray}, tray {tray_available}"
                    );
                }
            }
        }
    }

    #[test]
    fn tray_state_stopped_running_blocked() {
        let stopped = tray_state(false, false, Some("Ana"));
        assert!(!stopped.running && stopped.start_enabled);
        assert_eq!(stopped.tooltip, "VoiceChanger: Stopped");
        let running = tray_state(true, false, Some("Ana"));
        assert!(running.running && !running.start_enabled);
        assert_eq!(running.tooltip, "VoiceChanger: Running (Ana)");
        let blocked = tray_state(false, true, None);
        assert!(!blocked.running && !blocked.start_enabled);
        assert_eq!(blocked.tooltip, "VoiceChanger: Stopped");
        // A running session's Start stays greyed whatever the hint says.
        assert!(!tray_state(true, true, Some("Ana")).start_enabled);
    }

    #[test]
    fn tooltip_text() {
        assert_eq!(tooltip(false, None), "VoiceChanger: Stopped");
        assert_eq!(tooltip(false, Some("Ana")), "VoiceChanger: Stopped");
        assert_eq!(tooltip(true, Some("Ana")), "VoiceChanger: Running (Ana)");
        assert_eq!(tooltip(true, None), "VoiceChanger: Running");
        let long = tooltip(true, Some(&"é".repeat(300)));
        assert_eq!(long.encode_utf16().count(), 127);
        assert!(long.starts_with("VoiceChanger: Running (é") && long.ends_with('…'));
    }

    #[test]
    fn menu_ids_map_to_commands() {
        assert_eq!(command_for_menu(OPEN_ID), Some(TrayCommand::Open));
        assert_eq!(command_for_menu(START_ID), Some(TrayCommand::Start));
        assert_eq!(command_for_menu(STOP_ID), Some(TrayCommand::Stop));
        assert_eq!(command_for_menu(EXIT_ID), Some(TrayCommand::Exit));
        assert_eq!(command_for_menu("1001"), None);
    }

    #[test]
    fn left_click_release_opens() {
        use MouseButton::{Left, Middle, Right};
        use MouseButtonState::{Down, Up};
        assert_eq!(command_for_click(Left, Up), Some(TrayCommand::Open));
        for (button, state) in [(Left, Down), (Right, Up), (Right, Down), (Middle, Up)] {
            assert_eq!(command_for_click(button, state), None);
        }
    }

    fn scaled_logo() -> RgbaImage {
        let logo = image::load_from_memory_with_format(crate::ICON_PNG, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        imageops::resize(&logo, ICON_SIZE, ICON_SIZE, FilterType::Lanczos3)
    }

    #[test]
    fn icons_are_32_px() {
        assert_eq!(ICON_SIZE, 32);
        for running in [false, true] {
            assert_eq!(icon_rgba(running).dimensions(), (32, 32));
        }
    }

    #[test]
    fn stopped_icon_is_the_scaled_logo() {
        let icon = icon_rgba(false);
        assert!(icon == scaled_logo());
        assert!(icon.pixels().any(|p| p[3] > 0), "the icon is not empty");
    }

    #[test]
    fn running_icon_adds_only_a_green_dot() {
        let (stopped, running) = (icon_rgba(false), icon_rgba(true));
        let mut dot_pixels = 0;
        for (x, y, pixel) in running.enumerate_pixels() {
            if in_dot(x, y) {
                dot_pixels += 1;
                assert_eq!(*pixel, DOT_COLOUR, "({x}, {y}) is in the dot");
            } else {
                assert_eq!(pixel, stopped.get_pixel(x, y), "({x}, {y}) is outside the dot");
            }
        }
        assert!(dot_pixels > 50, "the dot covers {dot_pixels} pixels");
        let [r, g, b, a] = DOT_COLOUR.0;
        assert!(g > r && g > b && a == 255, "the dot is opaque green");
        assert!(running != stopped);
    }
}
```

### R5. `crates/app/src/tray.rs`, the icon (step 5)
Remove the `#![cfg_attr(not(test), expect(dead_code, ...))]` attribute. Add the imports `std::sync::mpsc::{self, Receiver}`, `tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem}`, `tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent}` and `crate::window::Waker`, then:
```rust
/// The tray icon and its menu. It lives on the UI thread, whose message loop delivers the clicks.
pub struct Tray {
    icon: TrayIcon,
    start: MenuItem,
    stop: MenuItem,
    stopped_icon: Icon,
    running_icon: Icon,
    rx: Receiver<TrayCommand>,
    /// What the tray shows now, so `sync` only calls Windows when it changes.
    shown: TrayState,
}

fn to_icon(image: RgbaImage) -> Result<Icon, String> {
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).map_err(|e| e.to_string())
}

impl Tray {
    /// Adds the icon to the system tray. A click queues a command and wakes the app.
    pub fn new(waker: Waker) -> Result<Self, String> {
        let shown = tray_state(false, true, None);
        let item = |id: &str, text: &str, enabled: bool| {
            MenuItem::with_id(MenuId::new(id), text, enabled, None)
        };
        let open = item(OPEN_ID, "Open VoiceChanger", true);
        let start = item(START_ID, "Start", shown.start_enabled);
        let stop = item(STOP_ID, "Stop", shown.running);
        let exit = item(EXIT_ID, "Exit", true);
        let menu = Menu::with_items(&[&open, &start, &stop, &exit]).map_err(|e| e.to_string())?;
        let stopped_icon = to_icon(icon_rgba(false))?;
        let running_icon = to_icon(icon_rgba(true))?;

        let (tx, rx) = mpsc::channel();
        let (menu_tx, menu_waker) = (tx.clone(), waker.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if let Some(command) = command_for_menu(&e.id.0) {
                let _ = menu_tx.send(command);
                menu_waker.wake();
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = e
                && let Some(command) = command_for_click(button, button_state)
            {
                let _ = tx.send(command);
                waker.wake();
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip(&shown.tooltip)
            .with_icon(stopped_icon.clone())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            icon,
            start,
            stop,
            stopped_icon,
            running_icon,
            rx,
            shown,
        })
    }

    /// The commands clicked since the last call.
    pub fn poll(&self) -> Vec<TrayCommand> {
        self.rx.try_iter().collect()
    }

    /// Shows `state`: the greyed items, the icon and the tooltip. Does nothing when it is
    /// what the tray already shows.
    pub fn sync(&mut self, state: TrayState) {
        if state == self.shown {
            return;
        }
        self.start.set_enabled(state.start_enabled);
        self.stop.set_enabled(state.running);
        if state.running != self.shown.running {
            let icon = if state.running {
                &self.running_icon
            } else {
                &self.stopped_icon
            };
            if let Err(e) = self.icon.set_icon(Some(icon.clone())) {
                tracing::warn!("cannot change the tray icon: {e}");
            }
        }
        if state.tooltip != self.shown.tooltip
            && let Err(e) = self.icon.set_tooltip(Some(&state.tooltip))
        {
            tracing::warn!("cannot change the tray tooltip: {e}");
        }
        self.shown = state;
    }
}
```

### R6. `crates/app/src/app.rs` by step
**Step 1.** `Startup` gains `pub instance: Option<Primary>`; `VcApp` gains `/// The single-instance guard; it hears a second launch.` `instance: Option<Primary>`. In `VcApp::new`, before the struct literal:
```rust
        let mut instance = startup.instance;
        if let Some(primary) = &mut instance {
            let ctx = cc.egui_ctx.clone();
            if let Err(e) = primary.listen(move || ctx.request_repaint()) {
                tracing::warn!("cannot listen for a second launch: {e}");
            }
        }
```
New method, and its call in `update` right after `self.poll_updates(ctx);`:
```rust
    /// Brings the window to the front.
    fn show_window(&mut self, ctx: &egui::Context) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
```
```rust
        if self.instance.as_ref().is_some_and(Primary::take_show) {
            self.show_window(ctx);
        }
```
**Step 2.** In `VcApp::new`: `let waker = Waker::new(&cc.egui_ctx, window::hwnd_of(cc));` then `Hotkeys::new(waker.clone())`, the listener `{ let waker = waker.clone(); primary.listen(move || waker.wake()) }` and the updater callback `Box::new(move || waker.wake())` (the `let ctx = cc.egui_ctx.clone();` lines go).

**Step 3.** `VcApp` gains `/// Wakes the hidden window and notices Windows ending the session.` `window: MainWindow`. In `new`: `let window = MainWindow::new(&cc.egui_ctx, window::hwnd_of(cc)); let waker = window.waker();`. In `update`, after the show check:
```rust
        if self.window.take_end_session() {
            // Windows or an installer is closing the app: exit as if the window was closed.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.window.waker().wake();
        }
```
In `on_exit`, last: `self.window.stop();`.

**Step 5.** New fields:
```rust
    /// The tray icon and its menu; `None` when it could not be made, and after exit.
    tray: Option<Tray>,
    /// The window is hidden and only the tray icon shows.
    in_tray: bool,
    /// The app is exiting: the next close request is not turned into "hide".
    quitting: bool,
```
In `new` (a local named `window` shadows the module from here, so call `window::hwnd_of` before it):
```rust
        let hwnd = window::hwnd_of(cc);
        let window = MainWindow::new(&cc.egui_ctx, hwnd);
        let waker = window.waker();
        // The window is never hidden without a tray icon and a ticker to bring it back.
        let tray = if window.can_hide() {
            Tray::new(waker.clone())
                .inspect_err(|e| tracing::error!("no tray icon: {e}"))
                .ok()
        } else {
            tracing::warn!("no window handle: closing the window exits the app");
            None
        };
```
Methods:
```rust
    /// Why Start cannot be pressed now, as on the Voice tab; `None` when it can.
    fn start_blocked(&self) -> Option<&'static str> {
        start_hint(
            self.audio.is_alive(),
            start_blocker(&self.state.devices, &self.state.selection),
            self.model.is_ready(),
        )
    }

    /// Brings the window back from the tray, or to the front.
    fn show_window(&mut self, ctx: &egui::Context) {
        self.in_tray = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    /// Hides the window; the tray icon is the way back.
    fn hide_to_tray(&mut self, ctx: &egui::Context) {
        self.in_tray = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        // A hidden frame right after hiding puts eframe's event loop back to waiting.
        self.window.waker().post_paint();
    }

    /// Really exits: the next close request is not turned into "hide to the tray".
    fn quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        // The close request is seen on the next frame; a hidden window must be woken for it.
        self.window.waker().wake();
    }

    /// Start from the tray menu. If it is blocked or fails, the banner says why and the
    /// window opens on the Voice tab.
    fn start_from_tray(&mut self, ctx: &egui::Context) {
        if self.is_running() {
            return;
        }
        match self.start_blocked() {
            Some(hint) => self.banner = Some(format!("Tray: cannot start. {hint}")),
            None => self.start(),
        }
        if !self.is_running() {
            self.tab = Tab::Voice;
            self.show_window(ctx);
        }
    }

    /// Carries out the tray's commands, a second launch's "show" and Windows ending the session.
    fn poll_tray(&mut self, ctx: &egui::Context) {
        let commands = self.tray.as_ref().map(Tray::poll).unwrap_or_default();
        for command in commands {
            match command {
                TrayCommand::Open => self.show_window(ctx),
                TrayCommand::Start => self.start_from_tray(ctx),
                TrayCommand::Stop => self.stop(),
                TrayCommand::Exit => self.quit(ctx),
            }
        }
        if self.instance.as_ref().is_some_and(Primary::take_show) {
            self.show_window(ctx);
        }
        if self.window.take_end_session() {
            self.quit(ctx);
        }
    }

    /// The close button hides the window to the tray; a real exit lets eframe close it.
    fn handle_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        match close_action(self.quitting, self.in_tray, self.tray.is_some()) {
            CloseAction::Exit => {}
            CloseAction::Hide => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_to_tray(ctx);
            }
        }
    }

    /// Shows the app's state on the tray icon.
    fn sync_tray(&mut self) {
        let state = tray_state(
            self.is_running(),
            self.start_blocked().is_some(),
            self.model.info().map(|info| info.name.as_str()),
        );
        if let Some(tray) = &mut self.tray {
            tray.sync(state);
        }
    }

    /// Asks for the next frame. Not in the tray: Windows never paints a hidden window, so
    /// eframe would poll for the frame in a busy loop. The ticker runs the frames there.
    fn repaint_after(&self, ctx: &egui::Context, delay: Duration) {
        if !self.in_tray {
            ctx.request_repaint_after(delay);
        }
    }

    /// The tab bar, the status bar, the open tab and the update window.
    fn draw(&mut self, ctx: &egui::Context) {
        // the two panels, the CentralPanel and `update::ui::window(ctx, self);`, moved
        // unchanged from `update`
    }
```
`update` after step 5, in this order:
```rust
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // unchanged: the dead-audio banner, poll_status, poll_model, poll_switch, handle_drops,
        // poll_import, poll_convert, poll_updates
        self.poll_tray(ctx);
        // unchanged: monitor gain, the hotkey, push_voice, the meters
        let before = self.state.selection.clone();
        let provider_before = self.state.perf.provider();
        // Nothing is drawn in the tray, so no widget asks for a repaint there.
        if !self.in_tray {
            self.draw(ctx);
        }
        // unchanged: the restart and provider-change blocks
        self.handle_close(ctx);
        // the three repaint requests and the two in poll_updates now go through
        // self.repaint_after(ctx, ..); the save block is otherwise unchanged
        self.sync_tray();
    }
```
`toggle_from_hotkey` uses `self.start_blocked()`. `install_update`'s `Ok(())` arm becomes `self.updates.phase = Phase::Installing; self.quit(ctx);`. `on_exit` gains, before `self.window.stop();`:
```rust
        // eframe ends the process without dropping the app, so the icon is removed here.
        self.tray = None;
```
**Step 6.** New field `/// "Start in the tray": hide the window once its first frame has minimised it.` `start_in_tray: bool`, set to `startup.config.start_minimised && tray.is_some()` (read it before `startup.config` is moved). First lines of `update`:
```rust
        if self.start_in_tray {
            // eframe shows the window after its first frame, and the minimise command from
            // `new` follows it. Hide on the next frame, so nothing appears on screen.
            if ctx.cumulative_pass_nr() == 0 {
                ctx.request_repaint();
            } else {
                self.start_in_tray = false;
                self.hide_to_tray(ctx);
            }
        }
```

## Steps

### 1. Single instance: a second launch shows the running copy
Files: create `crates/app/src/instance.rs`; modify `crates/app/src/main.rs`, `crates/app/src/app.rs`, `crates/app/Cargo.toml`.

1. `Cargo.toml`: add `"Win32_Security"` and `"Win32_System_Threading"` to the `windows-sys` features.
2. `main.rs`: add `mod instance;` (alphabetical). Create `instance.rs` with only the `tests` module of R1 plus `use super::*;`. Run `cargo test -p voicechanger instance::`. Expected: it does not compile (`acquire`, `Instance` not found).
3. Add the rest of R1. Run again. Expected: 3 tests pass (`first_acquire_is_primary`, `second_acquire_is_secondary_and_signals_the_primary`, `primary_again_after_drop`).
4. `main.rs`, after the `--self-test` block and before `let dirs = ...` (so `--self-test` is never blocked):
```rust
    let instance = match instance::acquire(instance::APP_NAME) {
        Ok(instance::Instance::Primary(primary)) => Some(primary),
        Ok(instance::Instance::Secondary) => {
            tracing::info!("VoiceChanger is already running: asked it to show its window");
            return Ok(());
        }
        Err(e) => {
            tracing::warn!("single-instance check failed: {e}");
            None
        }
    };
```
Pass `instance` in `app::Startup`.
5. `app.rs`: R6 step 1 (`Startup.instance`, the `instance` field, `listen`, `show_window`, the check in `update`). Import `crate::instance::Primary`.
6. Run `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test -p voicechanger`. All pass.
7. Commit: `feat(app): run a single instance; a second launch shows the running window (VC-121)`.

Acceptance: a failed check never stops the app from starting; the second process returns `Ok(())` (exit code 0); the "VoiceChanger starting" log line still comes before the check (the installer test waits for it).

### 2. Waker: frames for a hidden window, used by the hotkey and the background threads
Files: create `crates/app/src/window.rs`; modify `crates/app/src/main.rs`, `crates/app/src/hotkey.rs`, `crates/app/src/app.rs`, `crates/app/Cargo.toml`.

1. `Cargo.toml`: add `raw-window-handle = "0.6"` to `[dependencies]`, `"Win32_UI_WindowsAndMessaging"` to the `windows-sys` features, and a new section:
```toml
[dev-dependencies]
winit = { version = "0.30", default-features = false, features = ["rwh_06"] }
```
2. `main.rs`: add `mod window;`. Create `window.rs` with the `tests` module of R2 only. Run `cargo test -p voicechanger window::`. Expected: it does not compile (`Waker`, `hwnd_of` not found).
3. Add the rest of R2. Run again. Expected: 4 tests pass (`wake_posts_paint_to_a_hidden_window`, `wake_does_not_post_to_a_visible_window`, `wake_without_a_window_does_nothing`, `hidden_winit_window_redraws_on_wake`). If pumping does not deliver the event, drive the same probe with `run_app_on_demand` and `event_loop.exit()` instead; the assertion stays.
4. `hotkey.rs`: `pub fn new(waker: Waker) -> Self`; the handler calls `waker.wake()` instead of `ctx.request_repaint()`; doc: "A press sets `pressed` and wakes the app, so it is seen while minimised or hidden in the tray." Import `crate::window::Waker`.
5. `app.rs`: R6 step 2.
6. Run fmt, clippy and `cargo test -p voicechanger`. All pass (the hotkey tests are unchanged).
7. Commit: `feat(app): wake the window with a posted paint while it is hidden (VC-121)`.

### 3. MainWindow: the `vc-wake` ticker and the end-session watch
Files: modify `crates/app/src/window.rs`, `crates/app/src/app.rs`, `crates/app/Cargo.toml`.

1. `Cargo.toml`: add `"Win32_UI_Shell"` to the `windows-sys` features.
2. Add the three tests of R3 to the `tests` module. Run `cargo test -p voicechanger window::`. Expected: it does not compile (`ends_session`, `MainWindow`, `Ticker` not found).
3. Add the code of R3 (without `can_hide`, which step 5 adds). Run again. Expected: 7 tests pass.
4. `app.rs`: R6 step 3 (the `window` field, `take_end_session` in `update`, `self.window.stop()` in `on_exit`).
5. Run fmt, clippy and `cargo test -p voicechanger`. All pass.
6. Commit: `feat(app): tick the hidden window and exit when Windows ends the session (VC-121)`.

Acceptance: `MainWindow::new` is called on the UI thread; with `hwnd == None` it starts no ticker and no watch; the subclass releases its reference on `WM_NCDESTROY`.

### 4. Tray rules and icons
Files: create `crates/app/src/tray.rs`; modify `crates/app/src/main.rs`, `crates/app/Cargo.toml`, `Cargo.lock`.

1. `Cargo.toml`: add `tray-icon = { version = "0.26", default-features = false }` (the default feature is Linux-only).
2. `main.rs`: add `mod tray;`; add `/// The 256 px app icon, generated by cargo xtask gen-icon.` `const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon-256.png");` and use it in `app_icon()`.
3. Create `tray.rs` with the module doc, the `cfg_attr` line and the `tests` module of R4. Run `cargo test -p voicechanger tray::`. Expected: it does not compile.
4. Add the rest of R4. Run again. Expected: 8 tests pass.
5. Run fmt, clippy (the `expect(dead_code)` holds in the non-test build; every item is used by a test in the test build), `cargo test -p voicechanger` and `cargo deny check`. Expected: deny passes; a second `keyboard-types` version is only a warning.
6. Commit: `feat(app): tray rules and icons (VC-121)`.

### 5. Tray icon, its menu and close-to-tray
Files: modify `crates/app/src/tray.rs`, `crates/app/src/window.rs`, `crates/app/src/app.rs`.

1. `tray.rs`: R5 (remove the `cfg_attr` line, add `Tray`).
2. `window.rs`: add `MainWindow::can_hide` (R3).
3. `app.rs`: R6 step 5: the fields, `Tray` creation, the methods, the new order of `update`, `draw`, `toggle_from_hotkey`, `install_update`, `on_exit`.
4. Run fmt, clippy and `cargo test -p voicechanger`. All pass. `git grep -n request_repaint_after crates/app/src` shows one hit, inside `repaint_after`.
5. Commit: `feat(app): system tray icon with Open, Start, Stop and Exit; closing hides to the tray (VC-121)`.

Acceptance:
- Menu, in order: Open VoiceChanger, Start, Stop, Exit; no left-click menu; left-click release opens the window.
- The close button, Alt+F4 and the taskbar's Close hide the window; tray Exit, the updater's install, the end-session flag, a close request while hidden and a missing tray exit.
- Without a tray (`Tray::new` failed or no window handle) the app behaves as before this task.
- `handle_close` runs after `draw`, so the last visible frame is a full one.

### 6. "Start in the tray"
Files: modify `crates/app/src/app.rs`, `crates/app/src/tabs/settings.rs`.

1. `settings.rs`: the checkbox becomes `ui.checkbox(&mut app.prefs.start_minimised, "Start in the tray");` and the doc of `general` says "Start in the tray".
2. `app.rs`: R6 step 6. The `Minimized(true)` command in `new` and its comment stay; add to the comment: "With a tray icon the window is hidden on the next frame."
3. Run fmt, clippy and `cargo test -p voicechanger`. All pass (`to_config_round_trip` proves the key is still `start_minimised`). `git grep -n 'Start minimised' crates` shows nothing.
4. Commit: `feat(app): "Start in the tray" replaces "Start minimised" (VC-121)`.

Acceptance: with the setting on and a tray icon, the app starts with only the tray icon; without a tray icon it starts minimised to the taskbar, as today; `settings.toml` and `CONFIG_VERSION` are unchanged.

### 7. Changelog, README and the full quality gate
Files: modify `CHANGELOG.md`, `README.md`.

1. `CHANGELOG.md`, under `## [Unreleased]` → `### Added`, a new last line:
`- System tray icon: closing the window now hides VoiceChanger to the system tray and the voice keeps running. Click the icon to open the window; right-click it for **Open VoiceChanger**, **Start**, **Stop** and **Exit**. The icon shows a green dot while running, and only Exit closes the app. Launching VoiceChanger again shows the running copy's window, and "Start minimised" is now "Start in the tray".`
2. `README.md`, in "Model library and settings", a new paragraph after the hotkey paragraph:
`Closing the window hides VoiceChanger to the system tray, where the voice keeps running. Click the tray icon to open the window, or right-click it for Open VoiceChanger, Start, Stop and Exit; only Exit closes the app. Settings → General → **Start in the tray** launches it with only the tray icon.`
3. Run the commands of "Final verification". All pass.
4. Commit: `docs: system tray icon in the changelog and README (VC-121)`.

## Final verification
From the repository root, on Windows:
```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo xtask fetch-models
cargo test --workspace          (CI sets VC_REQUIRE_ASSETS=1)
cargo deny check
cargo build --release
```
CI then builds the installer and runs `installer/test-install.ps1`, which still passes: the relaunched app logs "VoiceChanger starting" before the single-instance check, the test stops that copy, and `--self-test` runs before the check.

Requirement coverage: close-to-tray and the close rule (steps 4, 5); menu, dot, tooltip (4, 5); single instance (1); "Start in the tray" (6); wake-up, ticker, end-session (2, 3); hotkey while hidden (2); tray failure fallback, tray Start failure opens the window, icon removed in `on_exit` (5); docs (7). Explorer restarts are handled by `tray-icon` itself.

Not automated, as the design says: the real icon and menu in a live Explorer tray, because the full app needs a GPU window that CI runners lack.
