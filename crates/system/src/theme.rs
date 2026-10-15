//! Light or dark mode for the whole system.

use crate::Result;

/// True when the system uses dark mode.
pub fn dark() -> Result<bool> {
    imp::dark()
}

/// Switch dark mode on or off and return what the system reports afterwards.
pub fn set_dark(on: bool) -> Result<bool> {
    imp::set_dark(on)?;
    dark()
}

#[cfg(windows)]
mod imp {
    use crate::{Error, Result};
    use windows::Win32::Foundation::{ERROR_SUCCESS, LPARAM, WPARAM};
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD, RegGetValueW, RegSetKeyValueW};
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
    use windows::core::{PCWSTR, w};

    const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

    pub fn dark() -> Result<bool> {
        let mut v = 1u32;
        let mut len = 4u32;
        let err = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                KEY,
                w!("AppsUseLightTheme"),
                RRF_RT_REG_DWORD,
                None,
                Some(&mut v as *mut u32 as *mut _),
                Some(&mut len),
            )
        };
        // No value yet means Windows' default, which is light for apps.
        Ok(err == ERROR_SUCCESS && v == 0)
    }

    pub fn set_dark(on: bool) -> Result<()> {
        let light: u32 = if on { 0 } else { 1 };
        for name in [w!("AppsUseLightTheme"), w!("SystemUsesLightTheme")] {
            let err = unsafe {
                RegSetKeyValueW(HKEY_CURRENT_USER, KEY, name, REG_DWORD.0, Some(&light as *const u32 as *const _), 4)
            };
            if err != ERROR_SUCCESS {
                return Err(Error::Failed(format!("could not save the theme setting ({})", err.0)));
            }
        }
        // Tell running apps and the shell to repaint, the way the Settings app does.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(0),
                LPARAM(w!("ImmersiveColorSet").as_ptr() as isize),
                SMTO_ABORTIFHUNG,
                200,
                None,
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::Result;
    use crate::cmd::osascript;

    pub fn dark() -> Result<bool> {
        Ok(osascript("tell application \"System Events\" to tell appearance preferences to get dark mode")? == "true")
    }

    pub fn set_dark(on: bool) -> Result<()> {
        osascript(&format!("tell application \"System Events\" to tell appearance preferences to set dark mode to {on}")).map(|_| ())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use crate::Result;
    use crate::cmd::run;

    // GNOME and the desktops that follow its color-scheme key.
    pub fn dark() -> Result<bool> {
        Ok(run("gsettings", &["get", "org.gnome.desktop.interface", "color-scheme"])?.contains("dark"))
    }

    pub fn set_dark(on: bool) -> Result<()> {
        let v = if on { "prefer-dark" } else { "default" };
        run("gsettings", &["set", "org.gnome.desktop.interface", "color-scheme", v]).map(|_| ())
    }
}
