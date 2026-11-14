# VC-121: System tray icon

## Goal
Closing the VoiceChanger window no longer exits the app: the window hides and the app keeps running (and converting) from a system tray icon. The tray icon has a menu with **Open VoiceChanger, Start, Stop, Exit**, shows a green dot while running, and only Exit really closes the app.

This design matches mockup **v2**.

## Decisions
Your answers:

| Question | Your choice |
|---|---|
| Tray menu | **A**: Open VoiceChanger, Start, Stop, Exit. Start and Stop are both listed; the one that doesn't apply is greyed out. |
| Launching the app while it is already running | **Show the running copy's window.** Only one copy ever runs. |
| "Start minimised" checkbox | **Renamed "Start in the tray"**: the app launches with only the tray icon. The saved setting is reused. |
| How to build it | **`tray-icon` library, truly hidden window, small wake-up.** |
| Running indicator | **Green dot on the tray icon while running**, plus the tooltip. |

Rejected alternatives:
- **Minimised window without a taskbar button** instead of a hidden one: no wake-up needed, but the app would still appear in Alt-Tab.
- **Our own Win32 tray code**: no new crates, but about 300 lines of unsafe code to own.
- **A setting to turn close-to-tray off**: the task says "always", so there is none.

Behaviour I fixed without asking, all visible in the mockup:
- Left-click on the tray icon opens the window; right-click opens the menu.
- The minimise button still minimises to the taskbar.
- If Start from the tray cannot start or fails, the window opens so the reason is visible.

**Why a wake-up is needed.** eframe calls `VcApp::update` only when Windows sends the window a paint message (`eframe` `run.rs`, `RedrawRequested`). Windows sends none to a hidden window, so `ctx.request_repaint()` does nothing while hidden. Without a fix, tray commands, the hotkey, device-loss handling and even Exit would stall until the window is shown again.

The fix keeps all app logic where it is. While hidden, a helper posts a paint message to the window itself; winit 0.30.13 turns it into `RedrawRequested` (read in its source) and eframe runs a normal frame. eframe already paints a hidden window at start-up, so this is a path it uses.

## Architecture
New modules in `crates/app/src/`, in the style of `hotkey.rs` (pure logic separated from the OS calls):

| Module | Purpose |
|---|---|
| `tray.rs` | Owns the `tray-icon` icon and its menu. Turns clicks into `TrayCommand { Open, Start, Stop, Exit }` on a channel and wakes the app. `sync(TrayState)` updates the greyed items, the icon and the tooltip, only when the state changes. Also builds the two 32×32 icons from `assets/icon-256.png`: plain, and with a green dot. |
| `window.rs` | The Win32 glue for the main window, about 40 lines of unsafe. It has three parts: the wake-up, a ticker and an end-session watch (below). |
| `instance.rs` | Single instance: a named mutex plus a named "show" event, both per Windows session. |

`window.rs` parts:
- **`Waker::wake()`** calls `ctx.request_repaint()` and, while the window is hidden, also posts the paint message.
- **Ticker thread `vc-wake`** wakes the hidden window about twice a second. This keeps device-loss handling, model switches, update checks and settings saves running while hidden.
- **End-session watch** is a window subclass that sets a flag when Windows asks the app to end: `WM_ENDSESSION`, or `WM_QUERYENDSESSION` with `ENDSESSION_CLOSEAPP`. The second is what Restart Manager is documented to send, which is how `CloseApplications=yes` in `installer/setup.iss` closes the app.

Changes to existing files:

| File | Change |
|---|---|
| `main.rs` | After the `--self-test` branch, run the single-instance check. A second launch signals the running copy and exits with code 0. `--self-test` is never blocked. |
| `app.rs` | `VcApp` gains `tray: Option<Tray>`, the window helper, `in_tray` and `quitting`. `update` handles tray commands, the "show" signal, the end-session flag and close requests. `install_update` sets `quitting` before closing. `on_exit` removes the tray icon and stops the ticker. |
| `hotkey.rs` | The press handler calls `Waker::wake()` instead of `ctx.request_repaint()`, so the hotkey works while hidden. |
| `tabs/settings.rs` | Checkbox label becomes "Start in the tray". |
| `crates/app/Cargo.toml` | Add `tray-icon` 0.26 and `raw-window-handle` 0.6 (to read the window handle). Add `winit` 0.30 as a dev-dependency for one test. Add `windows-sys` features for messaging, subclassing and mutex/event. |
| `CHANGELOG.md`, `README.md` | Entry under Unreleased → Added; one line in the README. |

**Close rule**, a pure function `close_action(quitting, in_tray, tray_available)`:
- Exit when `quitting` is set. Tray Exit, the updater's install and the end-session flag set it.
- Exit when the window is already hidden: the request cannot be the ✕ button, so it comes from outside (for example `taskkill`).
- Exit when there is no tray icon.
- Otherwise cancel the close (`ViewportCommand::CancelClose`) and hide the window.

**Tray state**, a pure function of the app state:
- Start is enabled when the app is stopped and `start_hint(...)` is `None`, the same rule as the Start button.
- Stop is enabled while running.
- Tooltip is `VoiceChanger: Running (<model name>)` or `VoiceChanger: Stopped`.

**Start in the tray.** eframe always shows the window after its first frame, so the app keeps today's minimise command for the first frame and hides the window on the next. Nothing flashes on screen.

## Data and interfaces
- `settings.toml`: no change. The key stays `start_minimised`, and `CONFIG_VERSION` stays 1.
- New crates: `tray-icon` and `muda`, both MIT OR Apache-2.0, which `deny.toml` already allows. `cargo deny check` must pass; `muda` brings a second version of `keyboard-types`, which is only a warning under the current `multiple-versions = "warn"`.
- No installer change.

## Error handling and edge cases
- **Tray icon cannot be created:** logged, and the app behaves as today: ✕ exits and "Start in the tray" starts minimised to the taskbar. The window is never hidden without a tray icon to bring it back.
- **Explorer restarts:** `tray-icon` re-adds the icon itself.
- **Start from the tray is blocked or fails:** the banner is set as today and the window opens.
- **A device is lost while hidden:** the session stops on the next tick, the dot goes away, the tooltip says Stopped, and the banner is there when the window opens. There is no pop-up notification.
- **Installer or Windows sign-out while the app is open or in the tray:** the end-session flag makes the app exit properly, with settings saved and audio stopped.
- **Second launch while the first copy is exiting:** the launch does nothing; launching again works.
- **Exit:** the tray icon is removed explicitly in `on_exit`, because eframe ends the process without dropping the app.

## Testing
All automated, run by `cargo test --workspace` in CI. No manual checklist.

- **`tray.rs`:**
  - `close_action` for every combination.
  - Tray state for stopped, running and blocked.
  - Tooltip text.
  - Click and menu-id mapping to `TrayCommand`.
  - Icons: both are 32×32, the stopped icon equals the scaled logo, and the running icon differs only in the dot area, which is green.
- **`window.rs`**, against a plain hidden Win32 window made by the test:
  - `wake()` posts the paint message when hidden and not when visible.
  - The end-session watch sets its flag for `WM_ENDSESSION` and for `WM_QUERYENDSESSION` with `ENDSESSION_CLOSEAPP`, and not for an ordinary shutdown query.
- **winit guard test:** a hidden winit window gets `wake()` and must deliver `RedrawRequested` within a timeout. This is the test that fails if a winit or eframe upgrade breaks the wake-up.
- **`instance.rs`**, with a test-only name:
  - The first acquire is primary.
  - The second is secondary, and its signal reaches the primary's waiter.
  - After the primary is dropped, the next acquire is primary again.
- **Existing suites** (config, hotkey, installer test) keep passing. `--self-test` still runs while another copy is open.

Not covered automatically: the real icon and menu in a live Explorer tray. The full app needs a GPU window, which the CI runners don't have, so the logic and the Win32 glue are tested separately as above.

## Out of scope
- A pop-up notification ("still running in the tray") on first close or on device loss.
- A setting to turn close-to-tray off.
- Choosing a model, pitch or other controls from the tray menu.
- Starting VoiceChanger with Windows.
- Installer changes.
