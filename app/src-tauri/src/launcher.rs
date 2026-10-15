//! The launcher's app list. Listing apps is quick; drawing their icons is not
//! (about 5 s for 140 apps), so each icon is saved as a PNG and drawn only once.

use nimbus_system::apps;
use parking_lot::Mutex;
use serde::Serialize;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ICON_PX: u32 = 96;
/// Older than this, the next request also refreshes the list in the background.
const FRESH: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: String,
    pub name: String,
    /// The cached icon to show.
    pub icon: Option<String>,
}

#[derive(Default)]
pub struct Cache {
    list: Mutex<Option<(Instant, Vec<Entry>)>>,
    busy: Mutex<()>,
}

impl Cache {
    /// The cached list, refreshed in the background once it gets old. Only the very
    /// first call waits for the apps to be read.
    pub fn get(self: &Arc<Self>, cache_dir: &Path) -> Vec<Entry> {
        if let Some((at, list)) = self.list.lock().as_ref() {
            if at.elapsed() > FRESH {
                let (me, dir) = (self.clone(), cache_dir.to_path_buf());
                std::thread::spawn(move || me.refresh(&dir));
            }
            return list.clone();
        }
        self.refresh(cache_dir)
    }

    /// Read the apps again. A caller that waited for a refresh already running takes its result.
    fn refresh(&self, cache_dir: &Path) -> Vec<Entry> {
        let _one_at_a_time = self.busy.lock();
        if let Some((at, list)) = self.list.lock().as_ref() {
            if at.elapsed() < Duration::from_secs(2) {
                return list.clone();
            }
        }
        let list = load(&cache_dir.join("icons"));
        *self.list.lock() = Some((Instant::now(), list.clone()));
        list
    }
}

fn icon_file(dir: &Path, id: &str) -> PathBuf {
    let mut h = std::hash::DefaultHasher::new();
    id.hash(&mut h);
    dir.join(format!("{:016x}.png", h.finish()))
}

fn load(dir: &Path) -> Vec<Entry> {
    let _ = std::fs::create_dir_all(dir);
    apps::list(ICON_PX, |id| !icon_file(dir, id).exists())
        .into_iter()
        .map(|(app, icon)| {
            let file = icon_file(dir, &app.id);
            if let Some(icon) = icon {
                save_png(&file, icon);
            }
            let icon = file.exists().then(|| file.to_string_lossy().into_owned());
            Entry { id: app.id, name: app.name, icon }
        })
        .collect()
}

fn save_png(file: &Path, icon: apps::Icon) {
    let Some(img) = image::RgbaImage::from_raw(icon.width, icon.height, icon.rgba) else { return };
    let tmp = file.with_extension("tmp");
    if img.save_with_format(&tmp, image::ImageFormat::Png).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}
