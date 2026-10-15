//! "Hide from screen capture": the island never shows up in screenshots,
//! screen recordings or screen shares, and not in the AI's own screenshots either.
//!
//! The approach follows the earlier Nimbus prototype: apply the setting every time a window is shown, then
//! read it back from the OS. A one-shot call at creation can silently fail to stick,
//! and the app must never report itself hidden while a window is still capturable.

use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub enabled: bool,
    /// True only when the OS confirms the window is excluded from capture.
    pub verified: bool,
    /// What the OS reports, for the settings screen ("excluded", "none", "unsupported"…).
    pub mode: String,
}

pub fn apply(win: &WebviewWindow, on: bool) -> Status {
    imp::apply(win, on);
    status(win, on)
}

pub fn status(win: &WebviewWindow, enabled: bool) -> Status {
    let mode = imp::read(win);
    Status { enabled, verified: enabled && mode == "excluded", mode }
}

#[cfg(windows)]
mod imp {
    use tauri::WebviewWindow;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowDisplayAffinity, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WDA_NONE,
    };

    fn hwnd(win: &WebviewWindow) -> Option<HWND> {
        // Tauri links its own `windows` version; go through the raw pointer.
        win.hwnd().ok().map(|h| HWND(h.0 as *mut _))
    }

    pub fn apply(win: &WebviewWindow, on: bool) {
        if let Some(h) = hwnd(win) {
            // WDA_EXCLUDEFROMCAPTURE needs Windows 10 2004+; older builds would show a black box.
            let mode = if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
            unsafe {
                let _ = SetWindowDisplayAffinity(h, mode);
            }
        }
    }

    pub fn read(win: &WebviewWindow) -> String {
        let Some(h) = hwnd(win) else { return "unknown".into() };
        let mut a = 0u32;
        if unsafe { GetWindowDisplayAffinity(h, &mut a) }.is_err() {
            return "unknown".into();
        }
        match a {
            x if x == WDA_EXCLUDEFROMCAPTURE.0 => "excluded",
            x if x == WDA_MONITOR.0 => "black-box",
            x if x == WDA_NONE.0 => "none",
            _ => "unknown",
        }
        .into()
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use tauri::WebviewWindow;

    // NSWindow.sharingType = .none. There is no read-back API worth trusting here, so the
    // state we set is what we report; recent macOS versions may still capture through
    // ScreenCaptureKit, which the settings screen says.
    pub fn apply(win: &WebviewWindow, on: bool) {
        let _ = win.set_content_protected(on);
    }

    pub fn read(_win: &WebviewWindow) -> String {
        "set (unverified)".into()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use tauri::WebviewWindow;

    // X11 and Wayland have no way for a window to opt out of capture.
    pub fn apply(_win: &WebviewWindow, _on: bool) {}

    pub fn read(_win: &WebviewWindow) -> String {
        "unsupported".into()
    }
}
