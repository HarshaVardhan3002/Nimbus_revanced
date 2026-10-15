//! Wallpaper Engine (Steam app 431960, Windows only).
//!
//! Wallpapers are switched with Wallpaper Engine's own command line
//! (`wallpaper64.exe -control openWallpaper -file <project.json> -monitor N`).
//! It saves the wallpaper shown on each monitor in its `config.json`, so after a
//! switch we read that file back and only report success once it names the new one.

use crate::{Error, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// The wallpaper's folder; pass it to [`open`].
    pub id: String,
    pub title: String,
    /// "scene", "video", "web" or "application".
    pub kind: String,
    /// Preview image (jpg, png or gif) inside the folder, if there is one.
    pub preview: Option<String>,
    /// "workshop", "mine" or "default".
    pub source: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub installed: bool,
    /// Wallpaper Engine is running, so it covers the normal desktop wallpaper.
    pub running: bool,
    /// Folder of the wallpaper on monitor 0.
    pub current: Option<String>,
}

struct Install {
    root: PathBuf,
    workshop: PathBuf,
}

impl Install {
    fn exe(&self) -> PathBuf {
        self.root.join("wallpaper64.exe")
    }
    fn config(&self) -> PathBuf {
        self.root.join("config.json")
    }
}

#[cfg(windows)]
fn steam_path() -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::w;
    let mut buf = [0u16; 520];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamPath"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if ok.is_err() {
        return None;
    }
    let n = (len as usize / 2).saturating_sub(1);
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

#[cfg(not(windows))]
fn steam_path() -> Option<PathBuf> {
    None
}

/// Every Steam library folder, from `libraryfolders.vdf`.
fn libraries(steam: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam.to_path_buf()];
    let vdf = steam.join("steamapps").join("libraryfolders.vdf");
    if let Ok(text) = std::fs::read_to_string(vdf) {
        for line in text.lines() {
            let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
            if parts.next() == Some("path") {
                if let Some(p) = parts.next() {
                    out.push(PathBuf::from(p.replace("\\\\", "\\")));
                }
            }
        }
    }
    out
}

fn find() -> Option<Install> {
    let steam = steam_path()?;
    libraries(&steam).into_iter().find_map(|lib| {
        let root = lib.join("steamapps").join("common").join("wallpaper_engine");
        root.join("wallpaper64.exe").is_file().then(|| Install {
            workshop: lib.join("steamapps").join("workshop").join("content").join("431960"),
            root,
        })
    })
}

fn running() -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    sys.processes().values().any(|p| {
        let n = p.name().to_string_lossy().to_ascii_lowercase();
        n == "wallpaper64.exe" || n == "wallpaper32.exe"
    })
}

/// Wallpaper ids are folder paths in one spelling: forward slashes, lower case.
/// Windows paths ignore case, and Steam's registry path and Wallpaper Engine's
/// config spell the same folder differently.
fn id_of(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_ascii_lowercase()
}

/// Folder of the wallpaper Wallpaper Engine shows on `monitor`, from its config.json.
fn current_in(inst: &Install, monitor: u32) -> Option<String> {
    let text = std::fs::read_to_string(inst.config()).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&text).ok()?;
    // Settings sit under the Windows user name; "?installdirectory" and such are not users.
    let key = format!("Monitor{monitor}");
    cfg.as_object()?.iter().filter(|(k, _)| !k.starts_with('?')).find_map(|(_, user)| {
        let file = user
            .pointer("/general/wallpaperconfig/selectedwallpapers")?
            .get(&key)?
            .get("file")?
            .as_str()?;
        Path::new(file).parent().map(id_of)
    })
}

pub fn status() -> Status {
    match find() {
        Some(inst) => Status { installed: true, running: running(), current: current_in(&inst, 0) },
        None => Status { installed: false, running: false, current: None },
    }
}

fn read_item(dir: &Path, source: &'static str) -> Option<Item> {
    let text = std::fs::read_to_string(dir.join("project.json")).ok()?;
    let j: serde_json::Value = serde_json::from_str(&text).ok()?;
    let title = j.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled").trim().to_string();
    let kind = j.get("type").and_then(|v| v.as_str()).unwrap_or("").to_ascii_lowercase();
    let preview = j
        .get("preview")
        .and_then(|v| v.as_str())
        .map(|p| dir.join(p))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().replace('\\', "/"));
    Some(Item { id: id_of(dir), title, kind, preview, source })
}

/// One wallpaper by its id (folder), without listing all of them.
pub fn item(id: &str) -> Option<Item> {
    let source = if id.contains("/myprojects/") {
        "mine"
    } else if id.contains("/defaultprojects/") {
        "default"
    } else {
        "workshop"
    };
    read_item(Path::new(id), source)
}

/// Every installed wallpaper: Workshop subscriptions, your own projects, then the defaults.
pub fn list() -> Vec<Item> {
    let Some(inst) = find() else { return Vec::new() };
    let projects = inst.root.join("projects");
    let sources: [(PathBuf, &'static str); 3] = [
        (inst.workshop.clone(), "workshop"),
        (projects.join("myprojects"), "mine"),
        (projects.join("defaultprojects"), "default"),
    ];
    let mut out = Vec::new();
    for (dir, source) in sources {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut items: Vec<Item> =
            entries.filter_map(|e| e.ok()).filter_map(|e| read_item(&e.path(), source)).collect();
        items.sort_by_key(|i| i.title.to_lowercase());
        out.extend(items);
    }
    out
}

/// Show the wallpaper in folder `id` on `monitor` and return the folder Wallpaper
/// Engine now reports. Starts Wallpaper Engine if it is not running.
pub fn open(id: &str, monitor: u32) -> Result<String> {
    let inst = find().ok_or(Error::Unsupported("Wallpaper Engine is not installed"))?;
    let dir = PathBuf::from(id);
    let project = dir.join("project.json");
    if !project.is_file() {
        return Err(Error::Failed(format!("not a Wallpaper Engine wallpaper: {id}")));
    }
    let status = std::process::Command::new(inst.exe())
        .args(["-control", "openWallpaper", "-file"])
        .arg(&project)
        .args(["-monitor", &monitor.to_string()])
        .status()
        .map_err(|e| Error::Failed(format!("could not run Wallpaper Engine: {e}")))?;
    if !status.success() {
        return Err(Error::Failed(format!("Wallpaper Engine refused the change ({status})")));
    }
    // It writes config.json about 200 ms after the command returns; give it 4 s.
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(cur) = current_in(&inst, monitor) {
            if cur == id_of(&dir) {
                return Ok(cur);
            }
        }
        if Instant::now() > deadline {
            return Err(Error::Failed("Wallpaper Engine did not confirm the new wallpaper".into()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
