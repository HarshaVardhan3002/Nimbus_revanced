//! Master output volume of the default playback device.

use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Volume {
    /// 0..=100
    pub percent: u8,
    pub muted: bool,
    /// The output device's name, e.g. "Speakers (Realtek(R) Audio)", where the system says.
    pub device: Option<String>,
}

pub fn get() -> Result<Volume> {
    imp::get()
}

/// Set the volume and return what the device reports afterwards.
pub fn set(percent: u8) -> Result<Volume> {
    imp::set(percent.min(100))?;
    get()
}

pub fn set_muted(muted: bool) -> Result<Volume> {
    imp::set_muted(muted)?;
    get()
}

#[cfg(windows)]
pub(crate) mod imp {
    use super::Volume;
    use crate::Result;
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender};
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, STGM_READ};

    /// The default playback device.
    pub(crate) fn device() -> Result<IMMDevice> {
        unsafe {
            // Already initialised on this thread (in either mode) is fine.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            Ok(devices.GetDefaultAudioEndpoint(eRender, eConsole)?)
        }
    }

    fn endpoint() -> Result<IAudioEndpointVolume> {
        unsafe { Ok(device()?.Activate(CLSCTX_ALL, None)?) }
    }

    fn name(device: &IMMDevice) -> Option<String> {
        unsafe {
            let value = device.OpenPropertyStore(STGM_READ).ok()?.GetValue(&PKEY_Device_FriendlyName).ok()?;
            let text = PropVariantToStringAlloc(&value).ok()?;
            let out = text.to_string().ok();
            CoTaskMemFree(Some(text.0 as *const _));
            out
        }
    }

    pub fn get() -> Result<Volume> {
        let dev = device()?;
        unsafe {
            let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
            let level = ep.GetMasterVolumeLevelScalar()?;
            let muted = ep.GetMute()?.as_bool();
            Ok(Volume { percent: (level * 100.0).round() as u8, muted, device: name(&dev) })
        }
    }

    pub fn set(percent: u8) -> Result<()> {
        unsafe { Ok(endpoint()?.SetMasterVolumeLevelScalar(percent as f32 / 100.0, std::ptr::null())?) }
    }

    pub fn set_muted(muted: bool) -> Result<()> {
        unsafe { Ok(endpoint()?.SetMute(muted, std::ptr::null())?) }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Volume;
    use crate::cmd::osascript;
    use crate::{Error, Result};

    pub fn get() -> Result<Volume> {
        let out = osascript("set s to get volume settings\nreturn (output volume of s as text) & \",\" & (output muted of s as text)")?;
        let (v, m) = out.split_once(',').ok_or_else(|| Error::Failed(format!("unexpected: {out}")))?;
        let percent = v.trim().parse::<u8>().map_err(|_| Error::Failed(format!("unexpected: {out}")))?;
        Ok(Volume { percent, muted: m.trim() == "true", device: None })
    }

    pub fn set(percent: u8) -> Result<()> {
        osascript(&format!("set volume output volume {percent}")).map(|_| ())
    }

    pub fn set_muted(muted: bool) -> Result<()> {
        osascript(&format!("set volume output muted {muted}")).map(|_| ())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::Volume;
    use crate::cmd::run;
    use crate::{Error, Result};

    const SINK: &str = "@DEFAULT_AUDIO_SINK@";

    /// PipeWire's `wpctl` first, PulseAudio's `pactl` as the fallback.
    pub fn get() -> Result<Volume> {
        if let Ok(out) = run("wpctl", &["get-volume", SINK]) {
            // "Volume: 0.40" or "Volume: 0.40 [MUTED]"
            let level = out
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<f32>().ok())
                .ok_or_else(|| Error::Failed(format!("unexpected wpctl output: {out}")))?;
            return Ok(Volume { percent: (level * 100.0).round().min(100.0) as u8, muted: out.contains("MUTED"), device: None });
        }
        let vol = run("pactl", &["get-sink-volume", "@DEFAULT_SINK@"])?;
        let percent = vol
            .split('/')
            .nth(1)
            .and_then(|s| s.trim().trim_end_matches('%').parse::<u8>().ok())
            .ok_or_else(|| Error::Failed(format!("unexpected pactl output: {vol}")))?;
        let muted = run("pactl", &["get-sink-mute", "@DEFAULT_SINK@"])?.ends_with("yes");
        Ok(Volume { percent, muted, device: None })
    }

    pub fn set(percent: u8) -> Result<()> {
        let level = format!("{:.2}", percent as f32 / 100.0);
        if run("wpctl", &["set-volume", SINK, &level]).is_ok() {
            return Ok(());
        }
        run("pactl", &["set-sink-volume", "@DEFAULT_SINK@", &format!("{percent}%")]).map(|_| ())
    }

    pub fn set_muted(muted: bool) -> Result<()> {
        let flag = if muted { "1" } else { "0" };
        if run("wpctl", &["set-mute", SINK, flag]).is_ok() {
            return Ok(());
        }
        run("pactl", &["set-sink-mute", "@DEFAULT_SINK@", flag]).map(|_| ())
    }
}
