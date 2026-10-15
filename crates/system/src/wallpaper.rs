//! Desktop wallpaper.

use crate::{Error, Result};
use std::path::Path;

/// Path of the current wallpaper image.
pub fn get() -> Result<String> {
    wallpaper::get().map_err(|e| Error::Failed(e.to_string()))
}

/// Set the wallpaper from a local image and return the path the system now reports.
pub fn set(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Err(Error::Failed(format!("no such image: {}", path.display())));
    }
    let p = path.to_str().ok_or_else(|| Error::Failed("path is not valid UTF-8".into()))?;
    wallpaper::set_from_path(p).map_err(|e| Error::Failed(e.to_string()))?;
    get()
}

/// Image files in a folder, sorted by name, for the wallpaper picker.
pub fn list_images(dir: &Path) -> Vec<String> {
    const EXT: [&str; 6] = ["jpg", "jpeg", "png", "bmp", "webp", "heic"];
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<String> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| EXT.contains(&x.to_ascii_lowercase().as_str()))
        })
        .filter_map(|p| p.to_str().map(String::from))
        .collect();
    out.sort();
    out
}
