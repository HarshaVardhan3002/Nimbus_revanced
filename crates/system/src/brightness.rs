//! Display brightness (built-in panels; external monitors where DDC/CI allows it).

use crate::Result;

/// Brightness of the first controllable display, 0..=100.
pub fn get() -> Result<u8> {
    imp::get()
}

/// Set every controllable display and return the first one's reported value.
pub fn set(percent: u8) -> Result<u8> {
    imp::set(percent.min(100))?;
    get()
}

#[cfg(windows)]
mod imp {
    use crate::{Error, Result};
    use brightness::blocking::{Brightness, brightness_devices};

    pub fn get() -> Result<u8> {
        for dev in brightness_devices() {
            if let Ok(v) = dev.and_then(|d| d.get()) {
                return Ok(v.min(100) as u8);
            }
        }
        Err(Error::Unsupported("no display with adjustable brightness"))
    }

    pub fn set(percent: u8) -> Result<()> {
        let mut any = false;
        let mut last_err = None;
        for dev in brightness_devices() {
            match dev.and_then(|d| d.set(percent as u32)) {
                Ok(()) => any = true,
                Err(e) => last_err = Some(e.to_string()),
            }
        }
        if any { Ok(()) } else { Err(Error::Failed(last_err.unwrap_or_else(|| "no display found".into()))) }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::{Error, Result};

    // macOS has no public CLI for this; the app layer will use the brightness keys instead.
    pub fn get() -> Result<u8> {
        Err(Error::Unsupported("brightness on macOS"))
    }

    pub fn set(_percent: u8) -> Result<()> {
        Err(Error::Unsupported("brightness on macOS"))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use crate::cmd::run;
    use crate::{Error, Result};

    /// `brightnessctl -m` prints "device,class,current,percent%,max".
    pub fn get() -> Result<u8> {
        let out = run("brightnessctl", &["-m", "-c", "backlight"])?;
        out.split(',')
            .nth(3)
            .and_then(|p| p.trim_end_matches('%').parse::<u8>().ok())
            .ok_or_else(|| Error::Failed(format!("unexpected brightnessctl output: {out}")))
    }

    pub fn set(percent: u8) -> Result<()> {
        run("brightnessctl", &["-c", "backlight", "set", &format!("{percent}%")]).map(|_| ())
    }
}
