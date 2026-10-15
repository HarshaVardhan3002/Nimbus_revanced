//! Small cached thumbnails for the wallpaper picker. Decoding a folder of 4K images
//! (or animated GIF previews) in the web view is slow and keeps the GPU busy; here
//! each image is decoded once, in parallel, and the 320 px JPEG is reused until the
//! file changes. Each thumbnail also carries the image's colours, so the picker can
//! preview a wallpaper's palette before it is applied.

use nimbus_system::palette::{self, Palette};
use nimbus_system::wallpaper_engine;
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const MAX_IMAGES: usize = 240;

#[derive(Debug, Clone, Serialize)]
pub struct Thumb {
    /// The original image, to set as wallpaper.
    pub path: String,
    /// The cached thumbnail, to show.
    pub thumb: String,
    pub palette: Option<Palette>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineThumb {
    #[serde(flatten)]
    pub item: wallpaper_engine::Item,
    pub thumb: Option<String>,
    pub palette: Option<Palette>,
}

fn cache_key(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    meta.len().hash(&mut h);
    meta.modified().ok()?.hash(&mut h);
    Some(h.finish())
}

fn make(src: &Path, dst: &Path) -> Option<()> {
    // For a GIF this decodes the first frame only.
    let img = image::ImageReader::open(src).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let small = img.thumbnail(320, 200).to_rgb8();
    let tmp = dst.with_extension("tmp");
    small.save_with_format(&tmp, image::ImageFormat::Jpeg).ok()?;
    std::fs::rename(&tmp, dst).ok()
}

/// Cached thumbnail for one image, made on first use.
fn one(out_dir: &Path, src: &Path) -> Option<String> {
    let dst = out_dir.join(format!("{:016x}.jpg", cache_key(src)?));
    if !dst.exists() {
        make(src, &dst)?;
    }
    Some(dst.to_string_lossy().into_owned())
}

/// Colours of a thumbnail, remembered for the session (thumbnails never change in place).
fn colours(thumb: &str) -> Option<Palette> {
    static SEEN: LazyLock<Mutex<HashMap<String, Palette>>> = LazyLock::new(Default::default);
    if let Some(p) = SEEN.lock().get(thumb) {
        return Some(p.clone());
    }
    let img = image::open(thumb).ok()?.to_rgb8();
    let p = palette::from_pixels(img.pixels().map(|px| px.0));
    SEEN.lock().insert(thumb.to_string(), p.clone());
    Some(p)
}

fn out_dir(cache: &Path) -> PathBuf {
    let dir = cache.join("thumbs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Colours of any image, taken from its thumbnail so they match the picker exactly.
pub fn palette_for(cache: &Path, src: &Path) -> Option<Palette> {
    colours(&one(&out_dir(cache), src)?)
}

pub fn for_folder(cache: &Path, dir: &Path) -> Vec<Thumb> {
    let out = out_dir(cache);
    nimbus_system::wallpaper::list_images(dir)
        .par_iter()
        .take(MAX_IMAGES)
        .filter_map(|p| {
            let thumb = one(&out, Path::new(p))?;
            Some(Thumb { path: p.clone(), palette: colours(&thumb), thumb })
        })
        .collect()
}

pub fn for_engine(cache: &Path) -> Vec<EngineThumb> {
    let out = out_dir(cache);
    wallpaper_engine::list()
        .into_par_iter()
        .map(|item| {
            let thumb = item.preview.as_deref().and_then(|p| one(&out, Path::new(p)));
            let palette = thumb.as_deref().and_then(colours);
            EngineThumb { item, thumb, palette }
        })
        .collect()
}
