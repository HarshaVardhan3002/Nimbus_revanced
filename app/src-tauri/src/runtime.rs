//! The llama.cpp build that runs the local brain, chosen for this system and its
//! graphics chip: CUDA for NVIDIA, Vulkan for AMD and Intel, Metal on Apple silicon,
//! and the processor-only build when no graphics chip can hold a model.
//!
//! The builds come from PrismML's llama.cpp releases: the ternary Bonsai models need its
//! kernels, and it runs every ordinary GGUF model as well. Its list of known problems
//! (KNOWN_ISSUES.md of Ternary-Bonsai-2-27B-gguf) decides the defaults: Windows uses the
//! CUDA 12.4 build because 13.3 crashes on some systems; AMD uses Vulkan because HIP
//! breaks Bonsai on Strix Halo and RDNA2; Intel uses Vulkan because the SYCL build
//! cannot load the ternary types. ROCm and the others stay available as a setting.

use nimbus_system::gpu::{Adapter, Vendor};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const RELEASES: &str = "https://api.github.com/repos/PrismML-Eng/llama.cpp/releases?per_page=8";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Cuda,
    Vulkan,
    Rocm,
    Metal,
    Cpu,
}

impl Backend {
    pub fn key(self) -> &'static str {
        match self {
            Backend::Cuda => "cuda",
            Backend::Vulkan => "vulkan",
            Backend::Rocm => "rocm",
            Backend::Metal => "metal",
            Backend::Cpu => "cpu",
        }
    }

    pub fn on_gpu(self) -> bool {
        self != Backend::Cpu
    }
}

/// The release files a build needs on `os`/`arch`: the build itself (named by its
/// ending) and, for CUDA on Windows, NVIDIA's runtime libraries (named in full).
pub fn assets_for(b: Backend, os: &str, arch: &str) -> Option<&'static [&'static str]> {
    use Backend::*;
    Some(match (os, arch, b) {
        ("windows", "x86_64", Cuda) => &["-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip"],
        ("windows", "aarch64", Cuda) => &["-bin-win-cuda-13.4-arm64.zip", "cudart-llama-bin-win-cuda-13.4-arm64.zip"],
        ("windows", "x86_64", Vulkan) => &["-bin-win-vulkan-x64.zip"],
        ("windows", "x86_64", Rocm) => &["-bin-win-hip-radeon-x64.zip"],
        ("windows", "x86_64", Cpu) => &["-bin-win-cpu-x64.zip"],
        ("windows", "aarch64", Cpu) => &["-bin-win-cpu-arm64.zip"],
        ("linux", "x86_64", Cuda) => &["-bin-linux-cuda-12.8-x64.tar.gz"],
        ("linux", "x86_64", Vulkan) => &["-bin-ubuntu-vulkan-x64.tar.gz"],
        ("linux", "x86_64", Rocm) => &["-bin-ubuntu-rocm-7.2-x64.tar.gz"],
        ("linux", "x86_64", Cpu) => &["-bin-ubuntu-x64.tar.gz"],
        ("linux", "aarch64", Vulkan) => &["-bin-ubuntu-vulkan-arm64.tar.gz"],
        ("linux", "aarch64", Cpu) => &["-bin-ubuntu-arm64.tar.gz"],
        ("macos", "aarch64", Metal) => &["-bin-macos-arm64.tar.gz"],
        ("macos", "x86_64", Cpu) => &["-bin-macos-x64.tar.gz"],
        _ => return None,
    })
}

fn matches(name: &str, want: &str) -> bool {
    if want.starts_with("cudart") { name == want } else { name.ends_with(want) && !name.starts_with("cudart") }
}

const GB: u64 = 1_000_000_000;

/// What runs the brain on this computer and how much memory a model may use there.
#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub backend: Backend,
    pub chip: Option<Adapter>,
    /// Bytes a model, its context and the server may take.
    pub usable: u64,
}

/// The best place for a model: the graphics chip with the most memory (a card, or one
/// built into the processor that shares the system's memory), else the processor.
/// `forced` is the build chosen in the settings.
pub fn choose(adapters: &[Adapter], ram: u64, forced: Option<Backend>) -> Choice {
    choose_on(std::env::consts::OS, std::env::consts::ARCH, adapters, ram, forced)
}

fn choose_on(os: &str, arch: &str, adapters: &[Adapter], ram: u64, forced: Option<Backend>) -> Choice {
    let has = |b: Backend| assets_for(b, os, arch).is_some();
    // Leave the processor's share of memory to the system and the apps.
    let cpu = Choice { backend: Backend::Cpu, chip: None, usable: ram.saturating_sub(4 * GB) / 2 };
    if os == "macos" && arch == "aarch64" {
        let chip = adapters.first().cloned();
        let usable = chip.as_ref().map(|c| c.budget()).unwrap_or(ram / 4 * 3) * 9 / 10;
        return Choice { backend: forced.filter(|b| has(*b)).unwrap_or(Backend::Metal), chip, usable };
    }
    // Under 3 GB a graphics chip holds only the smallest model with little context: the
    // processor does as well.
    let Some(chip) = adapters.iter().find(|a| a.budget() >= 3 * GB) else { return cpu };
    let mut backend = match chip.vendor {
        Vendor::Nvidia if has(Backend::Cuda) => Backend::Cuda,
        _ if has(Backend::Vulkan) => Backend::Vulkan,
        _ => return cpu,
    };
    if let Some(f) = forced.filter(|b| has(*b)) {
        if f == Backend::Cpu {
            return cpu;
        }
        backend = f;
    }
    // A tenth stays free for the desktop and other apps drawing on the same chip.
    Choice { backend, chip: Some(chip.clone()), usable: chip.budget() * 9 / 10 }
}

/// Where Nimbus keeps its builds: one folder per kind.
pub fn root(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager;
    app.path().app_local_data_dir().ok().map(|d| d.join("runtime"))
}

fn server_name() -> &'static str {
    if cfg!(windows) { "llama-server.exe" } else { "llama-server" }
}

/// The server program somewhere under `dir` (the archives put it at the top or in a
/// build/bin folder).
pub fn server_in(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join(server_name());
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().is_dir()).find_map(|e| server_in(&e.path()))
}

/// The installed build of this kind, if there is one.
pub fn installed(app: &tauri::AppHandle, b: Backend) -> Option<PathBuf> {
    server_in(&root(app)?.join(b.key()))
}

/// Which release the installed build came from.
pub fn version(app: &tauri::AppHandle, b: Backend) -> Option<String> {
    std::fs::read_to_string(root(app)?.join(b.key()).join("release.txt")).ok().map(|s| s.trim().to_string())
}

fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        zip::ZipArchive::new(f).and_then(|mut z| z.extract(into)).map_err(|e| e.to_string())
    } else {
        tar::Archive::new(flate2::read::GzDecoder::new(f)).unpack(into).map_err(|e| e.to_string())
    }
}

/// Download and unpack the newest release's build of this kind. `on` hears (file name,
/// bytes so far, whole size).
pub fn install(app: &tauri::AppHandle, b: Backend, stop: &AtomicBool, mut on: impl FnMut(&str, u64, u64)) -> Result<PathBuf, String> {
    let wants = assets_for(b, std::env::consts::OS, std::env::consts::ARCH).ok_or("there is no such build for this system")?;
    let root = root(app).ok_or("Nimbus's data folder is not available")?;
    let releases = crate::fetch::json(RELEASES)?;
    // The newest finished release that has every file this build needs.
    let (tag, assets) = releases
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["draft"] != true)
        .find_map(|r| {
            let all = r["assets"].as_array()?;
            let picked: Option<Vec<&serde_json::Value>> =
                wants.iter().map(|w| all.iter().find(|a| a["name"].as_str().is_some_and(|n| matches(n, w)))).collect();
            Some((r["tag_name"].as_str()?.to_string(), picked?))
        })
        .ok_or("no release has this build")?;
    let downloads = root.join("downloads");
    let fresh = root.join(format!("{}.new", b.key()));
    let _ = std::fs::remove_dir_all(&fresh);
    for a in &assets {
        let name = a["name"].as_str().unwrap_or_default();
        let url = a["browser_download_url"].as_str().ok_or("a release file has no address")?;
        let sha = a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:"));
        let file = downloads.join(name);
        crate::fetch::download(url, &file, sha, stop, |done, total| on(name, done, total))?;
        unpack(&file, &fresh)?;
    }
    let exe = server_in(&fresh).ok_or("the download has no llama-server program")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755));
    }
    let _ = exe;
    std::fs::write(fresh.join("release.txt"), &tag).map_err(|e| e.to_string())?;
    let home = root.join(b.key());
    let _ = std::fs::remove_dir_all(&home);
    std::fs::rename(&fresh, &home).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&downloads);
    server_in(&home).ok_or_else(|| "the build was unpacked, but its server program is missing".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(vendor: Vendor, name: &str, dedicated: u64, shared: u64, integrated: bool) -> Adapter {
        Adapter { name: name.into(), vendor, dedicated, shared, integrated }
    }

    #[test]
    fn picks_the_build() {
        let ram = 32 * GB;
        let nv = [chip(Vendor::Nvidia, "RTX 4070", 12 * GB, 16 * GB, false)];
        assert_eq!(choose_on("windows", "x86_64", &nv, ram, None).backend, Backend::Cuda);
        assert_eq!(choose_on("linux", "x86_64", &nv, ram, None).backend, Backend::Cuda);
        assert_eq!(choose_on("windows", "x86_64", &nv, ram, Some(Backend::Vulkan)).backend, Backend::Vulkan);
        let amd = [chip(Vendor::Amd, "Radeon 8060S Graphics", 64 * GB, 48 * GB, true)];
        let c = choose_on("windows", "x86_64", &amd, ram, None);
        assert_eq!(c.backend, Backend::Vulkan);
        assert_eq!(c.usable, 64 * GB * 9 / 10);
        assert_eq!(choose_on("linux", "x86_64", &amd, ram, Some(Backend::Rocm)).backend, Backend::Rocm);
        let intel = [chip(Vendor::Intel, "UHD Graphics", 128 << 20, 16 * GB, true)];
        assert_eq!(choose_on("windows", "x86_64", &intel, ram, None).backend, Backend::Vulkan);
        // Too little memory on the chip, or no chip, or no GPU build for the system.
        let tiny = [chip(Vendor::Intel, "HD Graphics", 128 << 20, 2 * GB, true)];
        assert_eq!(choose_on("windows", "x86_64", &tiny, ram, None).backend, Backend::Cpu);
        assert_eq!(choose_on("linux", "x86_64", &[], ram, None).usable, 14 * GB);
        assert_eq!(choose_on("windows", "aarch64", &intel, ram, None).backend, Backend::Cpu);
        assert_eq!(choose_on("macos", "aarch64", &[], ram, None).backend, Backend::Metal);
    }

    #[test]
    fn release_file_names() {
        let n = "llama-prism-b10776-d00e252-bin-win-cuda-12.4-x64.zip";
        assert!(matches(n, "-bin-win-cuda-12.4-x64.zip"));
        assert!(!matches("cudart-llama-bin-win-cuda-12.4-x64.zip", "-bin-win-cuda-12.4-x64.zip"));
        assert!(matches("cudart-llama-bin-win-cuda-12.4-x64.zip", "cudart-llama-bin-win-cuda-12.4-x64.zip"));
        assert!(!matches("llama-prism-b1-bin-macos-arm64-kleidiai.tar.gz", "-bin-macos-arm64.tar.gz"));
        for os in ["windows", "linux", "macos"] {
            for arch in ["x86_64", "aarch64"] {
                assert!(assets_for(Backend::Cpu, os, arch).is_some() || os == "macos", "{os} {arch}");
            }
        }
    }
}
