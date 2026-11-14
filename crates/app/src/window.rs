//! Win32 glue for the main window: waking it while it is hidden, a ticker that keeps the app
//! polling in the tray, and noticing that Windows is ending the session.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ENDSESSION_CLOSEAPP, IsWindowVisible, PostMessageW, WM_ENDSESSION, WM_NCDESTROY, WM_PAINT,
    WM_QUERYENDSESSION,
};

/// How often the hidden window is woken.
const TICK: Duration = Duration::from_millis(500);
/// Identifies the end-session subclass on the window ("VC").
const SUBCLASS_ID: usize = 0x5643;

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
    let installed = unsafe {
        SetWindowSubclass(
            hwnd as HWND,
            Some(end_session_proc),
            SUBCLASS_ID,
            data as usize,
        )
    };
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
        let thread = thread::Builder::new()
            .name("vc-wake".into())
            .spawn(move || {
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

#[cfg(test)]
mod tests {
    use std::io;
    use std::ptr::{null, null_mut};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, MSG, PM_QS_POSTMESSAGE, PM_REMOVE, PeekMessageW,
        SendMessageW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };

    use super::*;

    /// A 1x1 top-level window of the system "STATIC" class, owned by the test's thread.
    struct TestWindow(HWND);

    impl TestWindow {
        fn new(visible: bool) -> Self {
            let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
            let style = if visible {
                WS_POPUP | WS_VISIBLE
            } else {
                WS_POPUP
            };
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
            assert!(
                !hwnd.is_null(),
                "CreateWindowExW: {}",
                io::Error::last_os_error()
            );
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
                PeekMessageW(
                    &mut msg,
                    self.0,
                    WM_PAINT,
                    WM_PAINT,
                    PM_REMOVE | PM_QS_POSTMESSAGE,
                ) != 0
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

    #[test]
    fn ends_session_cases() {
        assert!(ends_session(WM_ENDSESSION, 1, 0));
        assert!(
            !ends_session(WM_ENDSESSION, 0, 0),
            "the shutdown was cancelled"
        );
        assert!(ends_session(
            WM_QUERYENDSESSION,
            0,
            ENDSESSION_CLOSEAPP as LPARAM
        ));
        assert!(
            !ends_session(WM_QUERYENDSESSION, 0, 0),
            "an ordinary shutdown query"
        );
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
