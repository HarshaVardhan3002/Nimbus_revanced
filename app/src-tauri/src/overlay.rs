//! The agent's presence on screen. While the agent acts on the desktop (opens an app or
//! a page, starts a mail draft) a see-through overlay covers the main screen: a soft
//! light runs along the screen's edges, a chip above the taskbar says what it is doing,
//! and the agent's own cursor shows where it points. Escape stops every run at once.
//!
//! The overlay lets every click through, never takes focus, and is always hidden from
//! capture, so it never shows up in a screenshot, a share, or the agent's own frames.

use parking_lot::Mutex;
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

pub const LABEL: &str = "overlay";
/// After the last step, the light stays this long before it fades, so quick steps in a
/// row read as one stretch of work instead of a flicker.
const LINGER: Duration = Duration::from_millis(1400);

#[derive(Default)]
pub struct Overlay {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// Steps under way that act on the screen.
    holders: usize,
    /// Bumped on every change, so a delayed hide only lands if nothing came after it.
    epoch: u64,
}

/// What the overlay page is told. Positions are physical pixels from the screen's
/// top-left corner.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Show {
    /// The light comes up (or stays) with this line in the chip. `bottom` is how much
    /// of the screen's foot the taskbar takes, so the chip sits above it.
    On { label: String, bottom: u32 },
    Off,
    /// The cursor glides to a spot; `click` makes it press there.
    Cursor { x: i32, y: i32, click: bool, label: Option<String> },
}

/// Steps that do something the person can see happen on the desktop.
pub fn acts_on_screen(tool: &str) -> bool {
    matches!(tool, "open_app" | "open_url" | "draft_mail" | "act" | "press_keys")
}

pub fn escape() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

/// Made once at start and shown only while the agent acts.
pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("overlay.html".into()))
        .title("Nimbus at work")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    let _ = win.set_ignore_cursor_events(true);
    cover(&win);
    Ok(())
}

/// Cover the main screen. One pixel short of full height, so Windows does not treat it
/// as a full-screen app (which would push the taskbar back and mute notifications).
/// Returns how tall the taskbar is at the screen's foot.
fn cover(win: &WebviewWindow) -> u32 {
    let Ok(Some(m)) = win.primary_monitor() else { return 0 };
    let (pos, size) = (*m.position(), *m.size());
    let _ = win.set_position(PhysicalPosition::new(pos.x, pos.y));
    let _ = win.set_size(PhysicalSize::new(size.width, size.height.saturating_sub(1)));
    let work = m.work_area();
    let foot = (pos.y + size.height as i32) - (work.position.y + work.size.height as i32);
    foot.max(0) as u32
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// A step that acts on the screen begins: the light comes up with `label` in the chip.
pub fn hold(app: &AppHandle, label: &str) {
    let state = app.state::<crate::AppState>();
    let first = {
        let mut i = state.overlay.inner.lock();
        i.holders += 1;
        i.epoch += 1;
        i.holders == 1
    };
    let mut bottom = 0;
    if let Some(win) = window(app) {
        bottom = cover(&win);
        if first {
            let _ = win.show();
            let _ = win.set_ignore_cursor_events(true);
            crate::island::make_non_activating(&win);
            crate::privacy::apply(&win, true);
        }
    }
    let _ = app.emit_to(LABEL, "overlay", Show::On { label: label.into(), bottom });
    if first {
        let _ = app.emit("agent-on-screen", true);
        if let Err(e) = app.global_shortcut().register(escape()) {
            eprintln!("[nimbus] Escape could not be taken to stop the agent: {e}");
        }
    }
}

/// The step is over. The light fades once no step holds it.
pub fn release(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let epoch = {
        let mut i = state.overlay.inner.lock();
        if i.holders == 0 {
            return;
        }
        i.holders -= 1;
        if i.holders > 0 {
            return;
        }
        i.epoch += 1;
        i.epoch
    };
    fade(app, epoch, LINGER);
}

/// Escape: everything stops, and the light goes at once.
pub fn cancel(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    state.agent.stop_all();
    let epoch = {
        let mut i = state.overlay.inner.lock();
        i.holders = 0;
        i.epoch += 1;
        i.epoch
    };
    fade(app, epoch, Duration::ZERO);
}

/// True while the light is up.
pub fn showing(app: &AppHandle) -> bool {
    app.state::<crate::AppState>().overlay.inner.lock().holders > 0
}

fn fade(app: &AppHandle, epoch: u64, after: Duration) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(after);
        let state = app.state::<crate::AppState>();
        if state.overlay.inner.lock().epoch != epoch {
            return;
        }
        let _ = app.global_shortcut().unregister(escape());
        let _ = app.emit_to(LABEL, "overlay", Show::Off);
        let _ = app.emit("agent-on-screen", false);
        // Hide once the page has faded out.
        std::thread::sleep(Duration::from_millis(520));
        if state.overlay.inner.lock().epoch == epoch {
            if let Some(win) = window(&app) {
                let _ = win.hide();
            }
        }
    });
}

/// Point the agent's cursor at a spot on the main screen, in physical pixels.
pub fn point(app: &AppHandle, x: i32, y: i32, click: bool, label: Option<String>) {
    let _ = app.emit_to(LABEL, "overlay", Show::Cursor { x, y, click, label });
}

/// Settings' "Preview": the light, the chip and the cursor, for a few seconds, without
/// anything being done. Escape ends it early.
pub fn preview(app: &AppHandle) {
    if showing(app) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        hold(&app, "This is how it looks while Nimbus works");
        let (w, h) = window(&app)
            .and_then(|w| w.primary_monitor().ok().flatten())
            .map(|m| (m.size().width as f64, m.size().height as f64))
            .unwrap_or((1920.0, 1080.0));
        let tour = [
            (0.0, 0.0, false, None, 900),
            (0.64, 0.36, false, Some("Its own cursor"), 1500),
            (0.34, 0.56, true, Some("Shows each click"), 1600),
            (0.52, 0.7, false, Some("Esc stops it"), 1700),
        ];
        for (fx, fy, click, label, wait) in tour {
            if !showing(&app) {
                return;
            }
            if fx > 0.0 {
                point(&app, (w * fx) as i32, (h * fy) as i32, click, label.map(String::from));
            }
            std::thread::sleep(Duration::from_millis(wait));
        }
        release(&app);
    });
}
