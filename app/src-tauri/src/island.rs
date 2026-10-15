//! The island window: a transparent, always-on-top strip at the top centre of the
//! screen. The window itself is larger than the visible island, so the island can
//! grow and shrink with CSS animations instead of window resizes (which flicker).
//! Everything outside the island's current shape lets clicks through.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Monitor, PhysicalPosition, PhysicalSize, WebviewWindow};

pub const LABEL: &str = "island";

/// How far from the island the pointer still counts as near, for the core's eyes.
const NEAR: f64 = 480.0;

/// The island's visible shape, in CSS pixels relative to the window, sent by the UI
/// whenever it changes size.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Default)]
pub struct HitArea {
    rect: Mutex<Rect>,
    /// Whether the window currently ignores the mouse (the last value we applied).
    ignoring: AtomicBool,
}

impl HitArea {
    pub fn set(&self, r: Rect) {
        *self.rect.lock() = r;
    }
}

/// Pointer position in CSS pixels relative to the window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
struct Pointer {
    x: f64,
    y: f64,
}

pub fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// The window's largest size, in CSS pixels. On smaller or more zoomed screens it
/// takes the screen's size instead and the UI fits its panels inside.
const MAX_W: f64 = 1120.0;
const MAX_H: f64 = 700.0;

/// Put the window at the top centre of the monitor the cursor is on (primary as fallback).
pub fn place(win: &WebviewWindow) {
    let monitor = win
        .cursor_position()
        .ok()
        .and_then(|c| win.monitor_from_point(c.x, c.y).ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        fit(win, &m);
    }
}

/// Where the window belongs on this monitor: as big as it may be (never past the work
/// area), centred at the very top.
fn target(m: &Monitor) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let scale = m.scale_factor();
    let area = m.work_area();
    let w = (MAX_W * scale).min(m.size().width as f64).round() as u32;
    let bottom = (area.position.y + area.size.height as i32 - m.position().y).max(0) as f64;
    let h = (MAX_H * scale).min(bottom).round() as u32;
    let x = m.position().x + (m.size().width as i32 - w as i32) / 2;
    (PhysicalPosition::new(x, m.position().y), PhysicalSize::new(w, h))
}

fn fit(win: &WebviewWindow, m: &Monitor) {
    let (at, size) = target(m);
    let _ = win.set_size(size);
    let _ = win.set_position(at);
}

/// Whether the window is somewhere else than it belongs: a move that did not take when
/// it was first shown, or Windows shifting it after sleep or a display change.
fn misplaced(win: &WebviewWindow, m: &Monitor) -> bool {
    let (at, size) = target(m);
    match (win.outer_position(), win.inner_size()) {
        (Ok(p), Ok(s)) => p != at || s != size,
        _ => false,
    }
}

/// What a monitor change would alter: where it is, how big, how zoomed.
fn shape_of(m: &Monitor) -> (i32, i32, u32, u32, u64, i32) {
    let a = m.work_area();
    (m.position().x, m.position().y, m.size().width, m.size().height, m.scale_factor().to_bits(), a.size.height as i32)
}

/// Keep clicks on the island from stealing focus from the app the user is in, and keep
/// the window out of Alt+Tab and the taskbar.
pub fn make_non_activating(win: &WebviewWindow) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        };
        if let Ok(h) = win.hwnd() {
            let h = HWND(h.0 as *mut _);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize);
        }
    }
    #[cfg(not(windows))]
    let _ = win;
}

/// The window that had the keyboard before the island took it, to hand it back.
#[cfg(windows)]
static PREVIOUS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Let the window take keyboard focus (while the user types in it) or give it back to
/// the app that had it.
pub fn set_activating(win: &WebviewWindow, activating: bool) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{
            GWL_EXSTYLE, GetForegroundWindow, GetWindowLongPtrW, SetForegroundWindow, SetWindowLongPtrW, WS_EX_NOACTIVATE,
        };
        if let Ok(h) = win.hwnd() {
            let h = HWND(h.0 as *mut _);
            let front = GetForegroundWindow();
            if activating && front != h {
                PREVIOUS.store(front.0 as isize, Ordering::Relaxed);
            }
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            let want = if activating { ex & !(WS_EX_NOACTIVATE.0 as isize) } else { ex | WS_EX_NOACTIVATE.0 as isize };
            SetWindowLongPtrW(h, GWL_EXSTYLE, want);
            // Only while the island still has the keyboard: if the user clicked into
            // another app meanwhile, that app keeps it.
            if !activating && front == h {
                let previous = HWND(PREVIOUS.swap(0, Ordering::Relaxed) as *mut _);
                if !previous.is_invalid() {
                    let _ = SetForegroundWindow(previous);
                }
            }
        }
    }
    if activating {
        let _ = win.set_focus();
    }
}

/// Watch the cursor: turn click-through off only while it is over the island, tell
/// the UI when the pointer enters or leaves it ("island-hover"), and where the pointer
/// is while it is near ("cursor", at most 15 times a second, `null` once it leaves).
///
/// 30 checks a second cost a few microseconds each; the window state is only
/// touched when the answer changes.
pub fn spawn_click_through(app: AppHandle, hit: Arc<HitArea>) {
    std::thread::spawn(move || {
        let mut last: Option<Pointer> = None;
        let mut sent = Instant::now();
        let mut screen = None;
        let mut tick = 0u32;
        loop {
            std::thread::sleep(Duration::from_millis(33));
            let Some(win) = window(&app) else { continue };
            // Every two seconds: a new resolution, zoom or taskbar size refits the window,
            // and so does finding it anywhere but the top centre.
            tick = tick.wrapping_add(1);
            if tick % 60 == 0 {
                if let Ok(Some(m)) = win.current_monitor() {
                    let now = Some(shape_of(&m));
                    if (screen.is_some() && screen != now) || misplaced(&win, &m) {
                        fit(&win, &m);
                    }
                    screen = now;
                }
            }
            let (Ok(cursor), Ok(origin), Ok(scale)) = (win.cursor_position(), win.outer_position(), win.scale_factor())
            else {
                continue;
            };
            let r = *hit.rect.lock();
            let x = (cursor.x - origin.x as f64) / scale;
            let y = (cursor.y - origin.y as f64) / scale;
            let inside = x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h;
            let ignore = !inside;
            if hit.ignoring.swap(ignore, Ordering::Relaxed) != ignore {
                let _ = win.set_ignore_cursor_events(ignore);
                let _ = app.emit("island-hover", inside);
            }

            let near = x > r.x - NEAR && x < r.x + r.w + NEAR && y > -NEAR && y < r.y + r.h + NEAR;
            let now = near.then_some(Pointer { x, y });
            let moved = match (now, last) {
                (Some(a), Some(b)) => (a.x - b.x).abs() + (a.y - b.y).abs() > 2.0,
                (a, b) => a != b,
            };
            if moved && sent.elapsed() >= Duration::from_millis(66) {
                let _ = app.emit("cursor", now);
                last = now;
                sent = Instant::now();
            }
        }
    });
}
