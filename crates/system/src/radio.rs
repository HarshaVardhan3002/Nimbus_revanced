//! Wi-Fi and Bluetooth: on or off, and what they are connected to.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Wifi,
    Bluetooth,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Radio {
    pub on: bool,
    /// The Wi-Fi network, or the connected Bluetooth device(s).
    pub detail: Option<String>,
    /// Wi-Fi signal from 0 to 5 bars, while connected.
    pub signal: Option<u8>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Radios {
    /// `None` when the machine has no such radio, or the system does not expose it.
    pub wifi: Option<Radio>,
    pub bluetooth: Option<Radio>,
}

pub fn get() -> Result<Radios> {
    imp::get()
}

/// Switch a radio and return the state the system reports afterwards.
pub fn set(kind: Kind, on: bool) -> Result<Radios> {
    imp::set(kind, on)?;
    // Radios take a moment to change state; read back until it matches, for up to 2 s.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let now = get()?;
        let r = match kind {
            Kind::Wifi => &now.wifi,
            Kind::Bluetooth => &now.bluetooth,
        };
        if r.as_ref().map(|r| r.on) == Some(on) || Instant::now() >= deadline {
            return Ok(now);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(windows)]
mod imp {
    use super::{Kind, Radio, Radios};
    use crate::{Error, Result};
    use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Devices::Radios::{Radio as WinRadio, RadioAccessStatus, RadioKind, RadioState};
    use windows::Networking::Connectivity::{ConnectionProfile, NetworkConnectivityLevel, NetworkInformation};

    fn radios() -> Result<Vec<WinRadio>> {
        Ok(WinRadio::GetRadiosAsync()?.join()?.into_iter().collect())
    }

    fn state(all: &[WinRadio], kind: RadioKind) -> Option<bool> {
        all.iter().find(|r| r.Kind().ok() == Some(kind)).and_then(|r| r.State().ok()).map(|s| s == RadioState::On)
    }

    /// The connected Wi-Fi network: the internet profile when it is Wi-Fi, else any
    /// Wi-Fi profile with a live connection.
    fn wifi_network() -> (Option<String>, Option<u8>) {
        let connected = |p: &ConnectionProfile| {
            p.IsWlanConnectionProfile().unwrap_or(false)
                && p.GetNetworkConnectivityLevel().is_ok_and(|l| l != NetworkConnectivityLevel::None)
        };
        let mut candidates: Vec<ConnectionProfile> = NetworkInformation::GetInternetConnectionProfile().into_iter().collect();
        if let Ok(all) = NetworkInformation::GetConnectionProfiles() {
            candidates.extend(all);
        }
        let Some(p) = candidates.into_iter().find(connected) else { return (None, None) };
        // Windows 11 hides the network name from apps without location access; the
        // profile name is the same text in practice and stays readable.
        let ssid = p
            .WlanConnectionProfileDetails()
            .and_then(|d| d.GetConnectedSsid())
            .map(|s| s.to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| p.ProfileName().ok().map(|s| s.to_string()).filter(|s| !s.is_empty()));
        let bars = p.GetSignalBars().ok().and_then(|r| r.Value().ok());
        (Some(ssid.unwrap_or_else(|| "Connected".into())), bars)
    }

    fn bluetooth_devices() -> Option<String> {
        let mut names: Vec<String> = Vec::new();
        let selectors = [
            BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
            BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected),
        ];
        for sel in selectors.into_iter().flatten() {
            let Ok(found) = DeviceInformation::FindAllAsyncAqsFilter(&sel).and_then(|op| op.join()) else { continue };
            for d in &found {
                if let Ok(n) = d.Name() {
                    let n = n.to_string();
                    if !n.is_empty() && !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
        }
        match names.len() {
            0 => None,
            1 => names.pop(),
            n => Some(format!("{} + {}", names[0], n - 1)),
        }
    }

    pub fn get() -> Result<Radios> {
        let all = radios()?;
        let wifi = state(&all, RadioKind::WiFi).map(|on| {
            let (detail, signal) = if on { wifi_network() } else { (None, None) };
            Radio { on, detail, signal }
        });
        let bluetooth = state(&all, RadioKind::Bluetooth)
            .map(|on| Radio { on, detail: if on { bluetooth_devices() } else { None }, signal: None });
        Ok(Radios { wifi, bluetooth })
    }

    pub fn set(kind: Kind, on: bool) -> Result<()> {
        let want = match kind {
            Kind::Wifi => RadioKind::WiFi,
            Kind::Bluetooth => RadioKind::Bluetooth,
        };
        if WinRadio::RequestAccessAsync()?.join()? != RadioAccessStatus::Allowed {
            return Err(Error::Failed("Windows did not allow this app to switch radios".into()));
        }
        let radio = radios()?
            .into_iter()
            .find(|r| r.Kind().ok() == Some(want))
            .ok_or(Error::Unsupported("this computer has no such radio"))?;
        let status = radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off })?.join()?;
        if status != RadioAccessStatus::Allowed {
            // Airplane mode, or a policy, keeps the radio where it is.
            return Err(Error::Failed("Windows kept the radio as it was (airplane mode or a policy)".into()));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Kind, Radio, Radios};
    use crate::cmd::run;
    use crate::Result;

    /// The Wi-Fi interface name ("en0" on most Macs).
    fn wifi_device() -> Option<String> {
        let out = run("networksetup", &["-listallhardwareports"]).ok()?;
        let mut lines = out.lines();
        while let Some(l) = lines.next() {
            if l.trim() == "Hardware Port: Wi-Fi" {
                return lines.next()?.trim().strip_prefix("Device: ").map(String::from);
            }
        }
        None
    }

    pub fn get() -> Result<Radios> {
        let wifi = wifi_device().and_then(|dev| {
            let on = run("networksetup", &["-getairportpower", &dev]).ok()?.trim_end().ends_with("On");
            let detail = on
                .then(|| run("networksetup", &["-getairportnetwork", &dev]).ok())
                .flatten()
                .and_then(|s| s.split_once(": ").map(|(_, n)| n.trim().to_string()));
            Some(Radio { on, detail, signal: None })
        });
        // macOS has no built-in command for Bluetooth power; blueutil is the common one.
        let bluetooth = run("blueutil", &["-p"]).ok().map(|s| Radio { on: s.trim() == "1", detail: None, signal: None });
        Ok(Radios { wifi, bluetooth })
    }

    pub fn set(kind: Kind, on: bool) -> Result<()> {
        let flag = if on { "on" } else { "off" };
        match kind {
            Kind::Wifi => {
                let dev = wifi_device().ok_or(crate::Error::Unsupported("no Wi-Fi interface"))?;
                run("networksetup", &["-setairportpower", &dev, flag]).map(|_| ())
            }
            Kind::Bluetooth => run("blueutil", &["-p", if on { "1" } else { "0" }]).map(|_| ()),
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{Kind, Radio, Radios};
    use crate::cmd::run;
    use crate::Result;

    pub fn get() -> Result<Radios> {
        // NetworkManager for Wi-Fi, BlueZ for Bluetooth.
        let wifi = run("nmcli", &["radio", "wifi"]).ok().map(|s| {
            let on = s.trim() == "enabled";
            let detail = on
                .then(|| run("nmcli", &["-t", "-f", "active,ssid", "dev", "wifi"]).ok())
                .flatten()
                .and_then(|out| out.lines().find_map(|l| l.strip_prefix("yes:").map(String::from)));
            Radio { on, detail, signal: None }
        });
        let bluetooth = run("bluetoothctl", &["show"]).ok().filter(|s| s.contains("Powered:")).map(|s| {
            let on = s.contains("Powered: yes");
            let detail = on
                .then(|| run("bluetoothctl", &["devices", "Connected"]).ok())
                .flatten()
                .and_then(|out| out.lines().next().and_then(|l| l.splitn(3, ' ').nth(2)).map(String::from));
            Radio { on, detail, signal: None }
        });
        Ok(Radios { wifi, bluetooth })
    }

    pub fn set(kind: Kind, on: bool) -> Result<()> {
        let flag = if on { "on" } else { "off" };
        match kind {
            Kind::Wifi => run("nmcli", &["radio", "wifi", flag]).map(|_| ()),
            Kind::Bluetooth => run("bluetoothctl", &["power", flag]).map(|_| ()),
        }
    }
}
