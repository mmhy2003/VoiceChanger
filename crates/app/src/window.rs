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
