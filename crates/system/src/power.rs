//! Battery state, and the power actions: lock, sleep, restart, shut down.

use crate::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct Battery {
    pub percent: u8,
    pub charging: bool,
    pub plugged_in: bool,
}

/// `None` on machines without a battery.
pub fn battery() -> Option<Battery> {
    imp::battery()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Lock,
    Sleep,
    Restart,
    ShutDown,
}

/// Run a power action. Restart and shut down hand over to the system's own
/// sequence, which still lets apps ask to save their work.
pub fn run(action: Action) -> Result<()> {
    imp::run(action)
}

#[cfg(windows)]
mod imp {
    use super::{Action, Battery};
    use crate::{Error, Result};
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS, SetSuspendState};
    use windows::Win32::System::Shutdown::LockWorkStation;

    pub fn run(action: Action) -> Result<()> {
        // shutdown.exe asks running apps to close, exactly like the Start menu does.
        let shutdown = |flag: &str| {
            const NO_WINDOW: u32 = 0x0800_0000;
            let ok = std::process::Command::new("shutdown")
                .args([flag, "/t", "0"])
                .creation_flags(NO_WINDOW)
                .status()
                .map_err(|e| Error::Failed(format!("shutdown: {e}")))?
                .success();
            if ok { Ok(()) } else { Err(Error::Failed("Windows refused to start the shutdown".into())) }
        };
        match action {
            Action::Lock => unsafe { LockWorkStation().map_err(Error::from) },
            // Returns after the computer wakes up again.
            Action::Sleep => {
                if unsafe { SetSuspendState(false, false, false) } { Ok(()) } else { Err(Error::Failed("Windows refused to sleep".into())) }
            }
            Action::Restart => shutdown("/r"),
            Action::ShutDown => shutdown("/s"),
        }
    }

    pub fn battery() -> Option<Battery> {
        let mut s = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut s).ok()? };
        // BatteryFlag 128 = no system battery, 255 = unknown; percent 255 = unknown.
        if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent == 255 {
            return None;
        }
        Some(Battery {
            percent: s.BatteryLifePercent.min(100),
            charging: s.BatteryFlag & 8 != 0,
            plugged_in: s.ACLineStatus == 1,
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Action, Battery};
    use crate::cmd::{osascript, run as sh};
    use crate::Result;

    pub fn run(action: Action) -> Result<()> {
        match action {
            // Sleeping the display locks the Mac when "require password" is on, the default.
            Action::Lock => sh("pmset", &["displaysleepnow"]).map(|_| ()),
            Action::Sleep => sh("pmset", &["sleepnow"]).map(|_| ()),
            Action::Restart => osascript("tell application \"System Events\" to restart").map(|_| ()),
            Action::ShutDown => osascript("tell application \"System Events\" to shut down").map(|_| ()),
        }
    }

    /// `pmset -g batt` → "... 'AC Power' ... 87%; charging; ..."
    pub fn battery() -> Option<Battery> {
        let out = sh("pmset", &["-g", "batt"]).ok()?;
        let pct_end = out.find('%')?;
        let start = out[..pct_end].rfind(|c: char| !c.is_ascii_digit())? + 1;
        let percent = out[start..pct_end].parse().ok()?;
        Some(Battery {
            percent,
            charging: out.contains("; charging"),
            plugged_in: out.contains("AC Power"),
        })
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{Action, Battery};
    use crate::Result;
    use crate::cmd::run as sh;
    use std::fs;

    pub fn run(action: Action) -> Result<()> {
        let (prog, args): (&str, &[&str]) = match action {
            Action::Lock => ("loginctl", &["lock-session"]),
            Action::Sleep => ("systemctl", &["suspend"]),
            Action::Restart => ("systemctl", &["reboot"]),
            Action::ShutDown => ("systemctl", &["poweroff"]),
        };
        sh(prog, args).map(|_| ())
    }

    pub fn battery() -> Option<Battery> {
        let dir = fs::read_dir("/sys/class/power_supply").ok()?;
        for e in dir.flatten() {
            let p = e.path();
            if fs::read_to_string(p.join("type")).ok()?.trim() != "Battery" {
                continue;
            }
            let percent = fs::read_to_string(p.join("capacity")).ok()?.trim().parse().ok()?;
            let status = fs::read_to_string(p.join("status")).unwrap_or_default();
            let status = status.trim();
            return Some(Battery {
                percent,
                charging: status == "Charging",
                plugged_in: status != "Discharging",
            });
        }
        None
    }
}
