//! The graphics chips in this computer and how much memory each can give a model, for
//! choosing how to run a local language model.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Qualcomm,
    Other,
}

impl Vendor {
    pub fn from_pci(id: u32) -> Self {
        match id {
            0x10DE => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            0x106B => Vendor::Apple,
            0x4D4F_4351 | 0x5143 => Vendor::Qualcomm,
            _ => Vendor::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Adapter {
    pub name: String,
    pub vendor: Vendor,
    /// Memory of its own (or set aside for it in the firmware), in bytes.
    pub dedicated: u64,
    /// System memory it may borrow, in bytes.
    pub shared: u64,
    /// Built into the processor, sharing the system's memory.
    pub integrated: bool,
}

impl Adapter {
    /// What a model may count on: its own memory, or for a chip with little of its own,
    /// what it may borrow from the system.
    pub fn budget(&self) -> u64 {
        if self.dedicated >= 2 << 30 { self.dedicated } else { self.shared.max(self.dedicated) }
    }
}

/// The real graphics chips (software renderers left out), the one with the most memory
/// for a model first.
pub fn adapters() -> Vec<Adapter> {
    let mut all = imp::adapters();
    all.sort_by(|a, b| b.budget().cmp(&a.budget()).then((a.integrated as u8).cmp(&(b.integrated as u8))));
    all
}

#[cfg(windows)]
mod imp {
    use super::{Adapter, Vendor};
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};

    pub fn adapters() -> Vec<Adapter> {
        let mut out: Vec<Adapter> = Vec::new();
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return out };
            for i in 0..16 {
                let Ok(adapter) = factory.EnumAdapters1(i) else { break };
                let Ok(d) = adapter.GetDesc1() else { continue };
                if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.VendorId == 0x1414 {
                    continue; // Microsoft's own software renderers
                }
                let end = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
                let name = String::from_utf16_lossy(&d.Description[..end]).trim().to_string();
                // The same chip can be listed once per output: keep it once.
                if out.iter().any(|a| a.name == name && a.dedicated == d.DedicatedVideoMemory as u64) {
                    continue;
                }
                let vendor = Vendor::from_pci(d.VendorId);
                let dedicated = d.DedicatedVideoMemory as u64;
                out.push(Adapter {
                    integrated: super::looks_integrated(vendor, &name, dedicated),
                    name,
                    vendor,
                    dedicated,
                    shared: d.SharedSystemMemory as u64,
                })
            }
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{Adapter, Vendor};
    use std::fs;

    fn read(p: &std::path::Path) -> Option<String> {
        fs::read_to_string(p).ok().map(|s| s.trim().to_string())
    }

    pub fn adapters() -> Vec<Adapter> {
        let ram = sysinfo::System::new_with_specifics(sysinfo::RefreshKind::nothing().with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram())).total_memory();
        let mut out = Vec::new();
        let Ok(cards) = fs::read_dir("/sys/class/drm") else { return out };
        for card in cards.flatten() {
            let name = card.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            let dev = card.path().join("device");
            let Some(vendor) = read(&dev.join("vendor")).and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok()) else { continue };
            let vendor = Vendor::from_pci(vendor);
            let vram = read(&dev.join("mem_info_vram_total")).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            let gtt = read(&dev.join("mem_info_gtt_total")).and_then(|v| v.parse::<u64>().ok());
            let label = read(&dev.join("product_name")).unwrap_or_else(|| format!("{vendor:?} graphics"));
            out.push(Adapter { integrated: super::looks_integrated(vendor, &label, vram), name: label, vendor, dedicated: vram, shared: gtt.unwrap_or(ram / 2) });
        }
        // NVIDIA's driver tells its memory through its own tool, not sysfs.
        if let Ok(o) = std::process::Command::new("nvidia-smi").args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]).output() {
            let text = String::from_utf8_lossy(&o.stdout);
            let mut nv = text.lines().filter_map(|l| {
                let (name, mib) = l.rsplit_once(',')?;
                Some((name.trim().to_string(), mib.trim().parse::<u64>().ok()? << 20))
            });
            for a in out.iter_mut().filter(|a| a.vendor == Vendor::Nvidia) {
                if let Some((name, mem)) = nv.next() {
                    a.name = name;
                    a.dedicated = mem;
                    a.integrated = false;
                }
            }
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Adapter, Vendor};

    /// Apple silicon shares all memory with its graphics; Metal lets a model use about
    /// three quarters of it.
    pub fn adapters() -> Vec<Adapter> {
        if !cfg!(target_arch = "aarch64") {
            return Vec::new();
        }
        let ram = sysinfo::System::new_with_specifics(sysinfo::RefreshKind::nothing().with_memory(sysinfo::MemoryRefreshKind::nothing().with_ram())).total_memory();
        vec![Adapter { name: "Apple GPU".into(), vendor: Vendor::Apple, dedicated: 0, shared: ram / 4 * 3, integrated: true }]
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    pub fn adapters() -> Vec<super::Adapter> {
        Vec::new()
    }
}

/// Built into the processor: Intel's and AMD's graphics without a card of their own,
/// told apart by name and by how little memory they have.
#[allow(dead_code)]
fn looks_integrated(vendor: Vendor, name: &str, dedicated: u64) -> bool {
    let n = name.to_lowercase().replace("(tm)", "").replace("(r)", "");
    match vendor {
        Vendor::Intel => !n.contains("arc a") && !n.contains("arc b") && !n.contains("arc pro"),
        Vendor::Amd => !n.contains(" rx ") && !n.contains("radeon pro") && !n.contains("instinct") && (n.ends_with("graphics") || dedicated < 2 << 30),
        Vendor::Apple | Vendor::Qualcomm => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integrated_or_not() {
        assert!(looks_integrated(Vendor::Amd, "AMD Radeon(TM) 8060S Graphics", 96 << 30));
        assert!(!looks_integrated(Vendor::Amd, "AMD Radeon RX 7900 XTX", 24 << 30));
        assert!(looks_integrated(Vendor::Intel, "Intel(R) UHD Graphics 770", 128 << 20));
        assert!(!looks_integrated(Vendor::Intel, "Intel(R) Arc(TM) A770 Graphics", 16 << 30));
        assert!(!looks_integrated(Vendor::Nvidia, "NVIDIA GeForce RTX 4070", 12 << 30));
    }

    #[test]
    fn budget() {
        let card = Adapter { name: "x".into(), vendor: Vendor::Nvidia, dedicated: 12 << 30, shared: 16 << 30, integrated: false };
        assert_eq!(card.budget(), 12 << 30);
        let igpu = Adapter { name: "x".into(), vendor: Vendor::Intel, dedicated: 128 << 20, shared: 16 << 30, integrated: true };
        assert_eq!(igpu.budget(), 16 << 30);
    }

    /// What this computer has.
    #[test]
    #[ignore = "depends on the machine"]
    fn this_computer() {
        for a in adapters() {
            println!("{a:?} budget {} GB", a.budget() >> 30);
        }
    }
}
