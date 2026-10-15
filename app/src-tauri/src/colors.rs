//! The UI's colours: the accent the user picked, or the palette of the wallpaper on
//! screen, recomputed whenever the wallpaper changes, also from outside Nimbus.

use crate::{AppState, thumbs};
use nimbus_system::palette::{self, Palette};
use nimbus_system::{wallpaper, wallpaper_engine};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct Colors {
    /// The desktop wallpaper file and its change time, as last seen.
    seen: Mutex<Option<(String, Option<SystemTime>)>>,
    wallpaper: Mutex<Option<Palette>>,
}

impl Colors {
    /// The user's accent if one is set, else the wallpaper's colours (silver until known).
    fn pick(&self, accent: Option<&str>) -> Palette {
        accent
            .and_then(palette::from_accent)
            .or_else(|| self.wallpaper.lock().clone())
            .unwrap_or_else(|| palette::from_pixels(std::iter::empty()))
    }

    /// Recompute the wallpaper's colours; true when they changed.
    fn update(&self, cache: &Path) -> bool {
        let Some(p) = source().and_then(|src| thumbs::palette_for(cache, &src)) else { return false };
        let mut current = self.wallpaper.lock();
        if current.as_ref() == Some(&p) {
            return false;
        }
        *current = Some(p);
        true
    }

    /// Cheap look at the desktop wallpaper setting; true when it changed since last time.
    fn wallpaper_moved(&self) -> bool {
        let Ok(path) = wallpaper::get() else { return false };
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let now = Some((path, stamp));
        let mut seen = self.seen.lock();
        if *seen == now {
            return false;
        }
        *seen = now;
        true
    }
}

/// The picture to take colours from: the running Wallpaper Engine wallpaper's preview
/// (the same image the picker shows, so a preview and the result match), else the
/// desktop wallpaper.
fn source() -> Option<PathBuf> {
    let engine = wallpaper_engine::status();
    if engine.running {
        let preview = engine.current.as_deref().and_then(wallpaper_engine::item).and_then(|i| i.preview);
        if let Some(p) = preview {
            return Some(PathBuf::from(p));
        }
    }
    wallpaper::get().ok().filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// The palette to show now, working it out first if the watcher has not yet.
pub fn current(app: &AppHandle) -> Palette {
    let state = app.state::<AppState>();
    if state.colors.wallpaper.lock().is_none() {
        if let Ok(cache) = app.path().app_cache_dir() {
            state.colors.update(&cache);
        }
    }
    let accent = state.settings.lock().accent.clone();
    state.colors.pick(accent.as_deref())
}

/// Send the UI the palette it should show, and return it.
pub fn publish(app: &AppHandle) -> Palette {
    let p = current(app);
    let _ = app.emit("palette", &p);
    p
}

/// Recompute and publish soon, after Nimbus itself changed the wallpaper.
pub fn refresh_soon(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        let Ok(cache) = app.path().app_cache_dir() else { return };
        if app.state::<AppState>().colors.update(&cache) {
            publish(&app);
        }
    });
}

/// Follow wallpaper changes. The desktop wallpaper setting is cheap to read, so it is
/// checked every 1.5 s; Wallpaper Engine's own state only every 9 s.
pub fn spawn_watcher(app: AppHandle) {
    std::thread::spawn(move || {
        let Ok(cache) = app.path().app_cache_dir() else { return };
        for tick in 0u64.. {
            let colors = &app.state::<AppState>().colors;
            if (colors.wallpaper_moved() || tick % 6 == 0) && colors.update(&cache) {
                publish(&app);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
}
