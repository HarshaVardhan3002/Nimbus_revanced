//! What the clipboard history needs from the system: a cheap change counter, the
//! "keep this out of history" markers that password managers set, which app copied,
//! and a way to paste into the app in front.

use crate::Result;

/// Changes every time anything is copied, without opening the clipboard.
/// `None` where the system has no such counter; then compare contents instead.
pub fn sequence() -> Option<u32> {
    imp::sequence()
}

/// The app that copied asked clipboard tools to skip this content. Password
/// managers (KeePass, 1Password, Bitwarden…) mark passwords this way.
pub fn is_secret() -> bool {
    imp::is_secret()
}

/// Friendly name of the app that put the current content on the clipboard.
pub fn owner_app() -> Option<String> {
    imp::owner_app()
}

/// Press the paste shortcut in the app in front (Ctrl+V, or Cmd+V on macOS).
pub fn send_paste() -> Result<()> {
    imp::send_paste()
}

#[cfg(windows)]
mod imp {
    use crate::Result;
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_V,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    use windows::core::w;

    pub fn sequence() -> Option<u32> {
        Some(unsafe { GetClipboardSequenceNumber() })
    }

    pub fn is_secret() -> bool {
        unsafe {
            let skip = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
            let ignore = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
            if IsClipboardFormatAvailable(skip).is_ok() || IsClipboardFormatAvailable(ignore).is_ok() {
                return true;
            }
            // A DWORD where 0 means "keep out of clipboard history" (Windows' own Win+V honours it).
            let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
            if IsClipboardFormatAvailable(history).is_err() {
                return false;
            }
            if OpenClipboard(None).is_err() {
                return true; // cannot read the flag: stay on the safe side
            }
            let mut keep_out = true;
            if let Ok(h) = GetClipboardData(history) {
                let mem = HGLOBAL(h.0);
                let p = GlobalLock(mem) as *const u32;
                if !p.is_null() {
                    keep_out = *p == 0;
                    let _ = GlobalUnlock(mem);
                }
            }
            let _ = CloseClipboard();
            keep_out
        }
    }

    pub fn owner_app() -> Option<String> {
        unsafe {
            let hwnd = GetClipboardOwner().ok()?;
            if hwnd.is_invalid() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let exe = crate::winutil::exe_path(pid)?;
            Some(crate::winutil::app_name(&exe))
        }
    }

    pub fn send_paste() -> Result<()> {
        let key = |vk: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() } },
        };
        let down = KEYBD_EVENT_FLAGS(0);
        let inputs = [key(VK_CONTROL, down), key(VK_V, down), key(VK_V, KEYEVENTF_KEYUP), key(VK_CONTROL, KEYEVENTF_KEYUP)];
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize == inputs.len() { Ok(()) } else { Err(crate::Error::Failed("Windows blocked the paste keystroke".into())) }
    }
}

#[cfg(not(windows))]
mod imp {
    use crate::Result;
    use crate::cmd::run;

    pub fn sequence() -> Option<u32> {
        None
    }

    // macOS pasteboards and X11/Wayland selections carry no marker this crate can
    // read yet, so the history stays off by default on these systems.
    pub fn is_secret() -> bool {
        false
    }

    pub fn owner_app() -> Option<String> {
        None
    }

    pub fn send_paste() -> Result<()> {
        #[cfg(target_os = "macos")]
        return crate::cmd::osascript("tell application \"System Events\" to keystroke \"v\" using command down").map(|_| ());
        #[cfg(not(target_os = "macos"))]
        run("xdotool", &["key", "--clearmodifiers", "ctrl+v"])
            .or_else(|_| run("wtype", &["-M", "ctrl", "v", "-m", "ctrl"]))
            .map(|_| ())
    }
}
