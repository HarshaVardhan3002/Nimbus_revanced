//! Resource monitor: CPU, memory, GPU, disk, network, battery and the busiest apps.
//!
//! `Monitor` keeps the previous sample so rates (CPU %, network bytes/s, GPU %)
//! are measured over the interval between two `sample()` calls.

use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;
use sysinfo::{Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System, UpdateKind};

#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub cpu_percent: f32,
    pub cpu_per_core: Vec<f32>,
    pub cpu_name: String,
    /// Current CPU speed in MHz. On Windows this is the base clock scaled by the live
    /// performance counter, which is what Task Manager shows as "Speed".
    pub cpu_mhz: Option<u32>,
    pub processes: u32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// The disk holding the system (C:\ or /).
    pub disk_used: u64,
    pub disk_total: u64,
    pub net_rx_per_s: u64,
    pub net_tx_per_s: u64,
    /// GPU utilisation, where the OS exposes it (Windows today).
    pub gpu_percent: Option<f32>,
    /// GPU memory in use (dedicated + shared), where available.
    pub gpu_mem_used: Option<u64>,
    pub gpu_name: Option<String>,
    pub battery: Option<crate::power::Battery>,
    pub uptime_s: u64,
    /// The apps using the most CPU and the most memory.
    pub top_cpu: Vec<Proc>,
    pub top_mem: Vec<Proc>,
}

/// A drive and how full it is.
#[derive(Debug, Clone, Serialize)]
pub struct Drive {
    /// Where it is mounted ("C:\\", "/", "/media/usb").
    pub mount: String,
    /// Its label, if it has one.
    pub name: String,
    pub used: u64,
    pub total: u64,
    pub removable: bool,
}

/// One app: every running process with the same program name, added up.
#[derive(Debug, Clone, Serialize)]
pub struct Proc {
    pub name: String,
    pub count: u32,
    pub cpu_percent: f32,
    pub mem: u64,
}

pub struct Monitor {
    sys: System,
    nets: Networks,
    disks: Disks,
    last: Instant,
    counters: counters::Counters,
    gpu_name: Option<String>,
}

impl Default for Monitor {
    fn default() -> Self {
        Self::new()
    }
}

impl Monitor {
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(RefreshKind::everything().without_processes());
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_kind());
        Self {
            sys,
            nets: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last: Instant::now(),
            counters: counters::Counters::new(),
            gpu_name: counters::gpu_name(),
        }
    }

    /// Take a sample. Call it on a steady interval (about once a second).
    pub fn sample(&mut self, top_n: usize) -> Snapshot {
        let dt = self.last.elapsed().as_secs_f64().max(0.001);
        self.last = Instant::now();

        self.sys.refresh_cpu_usage();
        // Windows reports the base clock here; the live speed comes from the counters below.
        #[cfg(not(windows))]
        self.sys.refresh_cpu_frequency();
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_kind());
        self.nets.refresh(true);
        self.disks.refresh(true);

        let (rx, tx) = self
            .nets
            .iter()
            .fold((0u64, 0u64), |(r, t), (_, n)| (r + n.received(), t + n.transmitted()));

        let system_disk = self
            .disks
            .iter()
            .find(|d| {
                let m = d.mount_point().to_string_lossy();
                m == "/" || m.eq_ignore_ascii_case("C:\\")
            })
            .or_else(|| self.disks.iter().next());

        let (top_cpu, top_mem) = self.top_apps(top_n);
        let reading = self.counters.read();
        let cpu = self.sys.cpus().first();
        let cpu_mhz = cpu.map(|c| c.frequency()).filter(|&f| f > 0).map(|base| match reading.cpu_perf {
            Some(perf) => (base as f64 * perf / 100.0).round() as u32,
            None => base as u32,
        });

        Snapshot {
            cpu_percent: self.sys.global_cpu_usage(),
            cpu_per_core: self.sys.cpus().iter().map(|c| c.cpu_usage()).collect(),
            cpu_name: cpu.map(|c| c.brand().trim().to_string()).unwrap_or_default(),
            cpu_mhz,
            processes: self.sys.processes().len() as u32,
            mem_used: self.sys.used_memory(),
            mem_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            disk_used: system_disk.map(|d| d.total_space() - d.available_space()).unwrap_or(0),
            disk_total: system_disk.map(|d| d.total_space()).unwrap_or(0),
            net_rx_per_s: (rx as f64 / dt) as u64,
            net_tx_per_s: (tx as f64 / dt) as u64,
            gpu_percent: reading.gpu_percent,
            gpu_mem_used: reading.gpu_mem,
            gpu_name: self.gpu_name.clone(),
            battery: crate::power::battery(),
            uptime_s: System::uptime(),
            top_cpu,
            top_mem,
        }
    }

    /// Every drive with a size, the system's first. Snap and loop mounts are left out.
    pub fn drives(&mut self) -> Vec<Drive> {
        self.disks.refresh(true);
        let mut out: Vec<Drive> = self
            .disks
            .iter()
            .filter(|d| d.total_space() > 0)
            .map(|d| Drive {
                mount: d.mount_point().to_string_lossy().to_string(),
                name: d.name().to_string_lossy().to_string(),
                used: d.total_space().saturating_sub(d.available_space()),
                total: d.total_space(),
                removable: d.is_removable(),
            })
            .filter(|d| !d.mount.starts_with("/snap") && !d.mount.starts_with("/boot") && !d.mount.starts_with("/run"))
            .collect();
        out.sort_by_key(|d| (!(d.mount == "/" || d.mount.eq_ignore_ascii_case("C:\\")), d.mount.clone()));
        out.dedup_by(|a, b| a.mount == b.mount);
        out
    }

    /// Processes grouped by program name, so a browser counts as one app, not forty.
    fn top_apps(&self, top_n: usize) -> (Vec<Proc>, Vec<Proc>) {
        let cores = self.sys.cpus().len().max(1) as f32;
        let mut apps: HashMap<String, (Proc, Option<&Path>)> = HashMap::new();
        for p in self.sys.processes().values().filter(|p| p.pid().as_u32() != 0) {
            let name = p.name().to_string_lossy();
            let (app, exe) = apps.entry(name.to_lowercase()).or_insert_with(|| {
                (Proc { name: strip_exe(&name).to_string(), count: 0, cpu_percent: 0.0, mem: 0 }, None)
            });
            app.count += 1;
            // sysinfo reports per-core percent; normalise to the whole machine.
            app.cpu_percent += p.cpu_usage() / cores;
            app.mem += p.memory();
            if exe.is_none() {
                *exe = p.exe();
            }
        }
        let mut apps: Vec<_> = apps.into_values().collect();
        let mut top = |order: fn(&Proc, &Proc) -> std::cmp::Ordering| {
            apps.sort_by(|a, b| order(&a.0, &b.0));
            apps.iter()
                .take(top_n)
                .map(|(p, exe)| Proc { name: friendly_name(&p.name, *exe), ..p.clone() })
                .collect::<Vec<_>>()
        };
        let by_cpu = top(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        let by_mem = top(|a, b| b.mem.cmp(&a.mem));
        (by_cpu, by_mem)
    }
}

/// CPU and memory for every process; the program path only once per process.
fn process_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet)
}

fn strip_exe(name: &str) -> &str {
    match name.len().checked_sub(4) {
        Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".exe") => &name[..i],
        _ => name,
    }
}

/// The name an app gives itself ("Google Chrome" rather than "chrome").
fn friendly_name(name: &str, exe: Option<&Path>) -> String {
    #[cfg(windows)]
    if let Some(exe) = exe.and_then(|e| e.to_str()) {
        return crate::winutil::app_name(exe);
    }
    let _ = exe;
    name.to_string()
}

#[cfg(windows)]
mod counters {
    //! The Windows performance counters Task Manager uses: GPU load and memory,
    //! and the CPU's live speed relative to its base clock.
    use windows::Win32::System::Performance::*;
    use windows::core::w;

    #[derive(Default)]
    pub struct Reading {
        pub gpu_percent: Option<f32>,
        pub gpu_mem: Option<u64>,
        /// "% Processor Performance": 100 at the base clock, more while boosting.
        pub cpu_perf: Option<f64>,
    }

    pub struct Counters {
        query: isize,
        engines: isize,
        dedicated: isize,
        shared: isize,
        cpu_perf: isize,
    }

    // The PDH handles are plain kernel handles, used from one thread at a time behind a Mutex.
    unsafe impl Send for Counters {}

    impl Counters {
        pub fn new() -> Self {
            unsafe {
                let mut q = PDH_HQUERY::default();
                let mut c = Counters { query: 0, engines: 0, dedicated: 0, shared: 0, cpu_perf: 0 };
                if PdhOpenQueryW(None, 0, &mut q) != 0 {
                    return c;
                }
                let add = |path| {
                    let mut h = PDH_HCOUNTER::default();
                    if PdhAddEnglishCounterW(q, path, 0, &mut h) == 0 { h.0 as isize } else { 0 }
                };
                c.engines = add(w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"));
                c.dedicated = add(w!("\\GPU Adapter Memory(*)\\Dedicated Usage"));
                c.shared = add(w!("\\GPU Adapter Memory(*)\\Shared Usage"));
                c.cpu_perf = add(w!("\\Processor Information(_Total)\\% Processor Performance"));
                PdhCollectQueryData(q);
                c.query = q.0 as isize;
                c
            }
        }

        fn sum(counter: isize) -> Option<f64> {
            if counter == 0 {
                return None;
            }
            let c = PDH_HCOUNTER(counter as *mut _);
            unsafe {
                let (mut size, mut count) = (0u32, 0u32);
                PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None);
                if size == 0 {
                    return None;
                }
                // The buffer holds the item array followed by the instance name strings.
                let mut buf = vec![0u8; size as usize];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
                    return None;
                }
                let items = std::slice::from_raw_parts(items, count as usize);
                Some(items.iter().map(|i| i.FmtValue.Anonymous.doubleValue).sum())
            }
        }

        pub fn read(&mut self) -> Reading {
            if self.query == 0 {
                return Reading::default();
            }
            unsafe { PdhCollectQueryData(PDH_HQUERY(self.query as *mut _)) };
            let gpu_mem = match (Self::sum(self.dedicated), Self::sum(self.shared)) {
                (None, None) => None,
                (d, s) => Some((d.unwrap_or(0.0) + s.unwrap_or(0.0)) as u64),
            };
            Reading {
                gpu_percent: Self::sum(self.engines).map(|v| v.min(100.0) as f32),
                gpu_mem,
                cpu_perf: Self::sum(self.cpu_perf).filter(|p| *p > 0.0),
            }
        }
    }

    impl Drop for Counters {
        fn drop(&mut self) {
            if self.query != 0 {
                unsafe { PdhCloseQuery(PDH_HQUERY(self.query as *mut _)) };
            }
        }
    }

    /// Name of the first hardware graphics adapter.
    pub fn gpu_name() -> Option<String> {
        use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1};
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            (0..8).find_map(|i| {
                let desc = factory.EnumAdapters1(i).ok()?.GetDesc1().ok()?;
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                    return None;
                }
                let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
                Some(String::from_utf16_lossy(&desc.Description[..end]).trim().to_string())
            })
        }
    }
}

#[cfg(not(windows))]
mod counters {
    #[derive(Default)]
    pub struct Reading {
        pub gpu_percent: Option<f32>,
        pub gpu_mem: Option<u64>,
        pub cpu_perf: Option<f64>,
    }

    pub struct Counters;

    impl Counters {
        pub fn new() -> Self {
            Counters
        }

        pub fn read(&mut self) -> Reading {
            Reading::default()
        }
    }

    pub fn gpu_name() -> Option<String> {
        None
    }
}
