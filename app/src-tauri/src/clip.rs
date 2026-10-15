//! Clipboard history: copied text, links and images, kept on this PC only.
//!
//! Safeguards: content the copying app marks as private (password managers do this)
//! is never read, only counted; the history can be paused and cleared; pinned entries
//! survive a clear; nothing here is sent anywhere.

use crate::island;
use crate::store::write_atomic;
use arboard::{Clipboard, ImageData};
use nimbus_system::clipboard as sys;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// Unpinned entries kept; older ones fall off the end.
const MAX_ITEMS: usize = 200;
/// Unpinned images kept, since one screenshot can take megabytes.
const MAX_IMAGES: usize = 50;
/// Longer text is left out of the history.
const MAX_TEXT: usize = 256 * 1024;
/// Characters of text the list receives; copy and paste always use the full text.
const PREVIEW_CHARS: usize = 2000;
const THUMB_PX: u32 = 360;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Link,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    /// Hash of the content, so copying the same thing twice keeps one entry.
    pub id: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Length of the full text in characters.
    #[serde(default)]
    pub chars: usize,
    /// Image width and height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<[u32; 2]>,
    /// The app it was copied from, where the system tells.
    pub app: Option<String>,
    /// When it was last copied, in milliseconds since 1970.
    pub at: u64,
    #[serde(default)]
    pub pinned: bool,
}

/// One entry as the UI lists it.
#[derive(Serialize)]
pub struct Entry {
    #[serde(flatten)]
    item: Item,
    image: Option<String>,
    thumb: Option<String>,
}

#[derive(Serialize)]
pub struct View {
    pub enabled: bool,
    /// This system marks private copies, so they can be left out (Windows today).
    pub guards_secrets: bool,
    /// Private copies left out since the last clear.
    pub skipped: u32,
    pub items: Vec<Entry>,
}

/// Sent to the UI on every change; `fresh` is a new copy made outside Nimbus.
#[derive(Clone, Serialize)]
struct Change {
    fresh: bool,
    skipped: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    items: Vec<Item>,
    skipped: u32,
}

enum Content {
    Text(String),
    Image(ImageData<'static>),
}

pub struct History {
    dir: PathBuf,
    saved: Mutex<Saved>,
    enabled: AtomicBool,
    /// Kept open: on Linux, copied content only lives as long as its owner.
    clipboard: Mutex<Option<Clipboard>>,
    /// Change counter after Nimbus itself last wrote to the clipboard.
    own_change: Mutex<Option<u32>>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn content_id(tag: u8, bytes: &[u8]) -> String {
    let mut h = std::hash::DefaultHasher::new();
    tag.hash(&mut h);
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

fn is_link(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("https://") || t.starts_with("http://")) && !t.chars().any(char::is_whitespace)
}

fn path_string(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

impl History {
    pub fn open(dir: PathBuf, enabled: bool) -> Self {
        let saved = std::fs::read_to_string(dir.join("history.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let _ = std::fs::create_dir_all(dir.join("img"));
        Self {
            dir,
            saved: Mutex::new(saved),
            enabled: AtomicBool::new(enabled),
            clipboard: Mutex::new(None),
            own_change: Mutex::new(None),
        }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    pub fn view(&self) -> View {
        let saved = self.saved.lock();
        View {
            enabled: self.enabled.load(Ordering::Relaxed),
            guards_secrets: cfg!(windows),
            skipped: saved.skipped,
            items: saved.items.iter().map(|i| self.entry(i)).collect(),
        }
    }

    fn entry(&self, item: &Item) -> Entry {
        let mut item = item.clone();
        if let Some(t) = item.text.as_mut().filter(|_| item.chars > PREVIEW_CHARS) {
            *t = t.chars().take(PREVIEW_CHARS).collect();
        }
        let image = item.kind == Kind::Image;
        Entry {
            image: image.then(|| path_string(self.image_file(&item.id))),
            thumb: image.then(|| path_string(self.thumb_file(&item.id))),
            item,
        }
    }

    fn image_file(&self, id: &str) -> PathBuf {
        self.dir.join("img").join(format!("{id}.png"))
    }

    fn thumb_file(&self, id: &str) -> PathBuf {
        self.dir.join("img").join(format!("{id}.thumb.png"))
    }

    fn save(&self, saved: &Saved) {
        if let Ok(json) = serde_json::to_vec(saved) {
            let _ = write_atomic(&self.dir.join("history.json"), &json);
        }
    }

    fn remove_files(&self, id: &str) {
        let _ = std::fs::remove_file(self.image_file(id));
        let _ = std::fs::remove_file(self.thumb_file(id));
    }

    fn count_skipped(&self) {
        let mut saved = self.saved.lock();
        saved.skipped += 1;
        self.save(&saved);
    }

    /// Record a copy: a new entry, or an old one moved back to the top.
    fn add(&self, content: Content, app: Option<String>) {
        let (id, kind) = match &content {
            Content::Text(t) => (content_id(b't', t.as_bytes()), if is_link(t) { Kind::Link } else { Kind::Text }),
            Content::Image(img) => (content_id(b'i', &img.bytes), Kind::Image),
        };
        let known = self.saved.lock().items.iter().any(|i| i.id == id);
        let (mut text, mut size) = (None, None);
        match content {
            Content::Text(t) => text = Some(t),
            Content::Image(img) => {
                size = Some([img.width as u32, img.height as u32]);
                // Encode outside the lock: a 4K screenshot takes a moment.
                if !known && !self.write_image(&id, img) {
                    return;
                }
            }
        }
        let mut saved = self.saved.lock();
        match saved.items.iter().position(|i| i.id == id) {
            Some(pos) => {
                let mut item = saved.items.remove(pos);
                item.at = now_ms();
                item.app = app.or(item.app);
                saved.items.insert(0, item);
            }
            None => {
                let chars = text.as_deref().map_or(0, |t| t.chars().count());
                saved.items.insert(0, Item { id, kind, text, chars, size, app, at: now_ms(), pinned: false });
                self.trim(&mut saved);
            }
        }
        self.save(&saved);
    }

    fn write_image(&self, id: &str, img: ImageData<'static>) -> bool {
        let (w, h) = (img.width as u32, img.height as u32);
        let Some(full) = image::RgbaImage::from_raw(w, h, img.bytes.into_owned()) else { return false };
        let full = image::DynamicImage::ImageRgba8(full);
        let png = |img: &image::DynamicImage, file: PathBuf| {
            let tmp = file.with_extension("tmp");
            img.save_with_format(&tmp, image::ImageFormat::Png).is_ok() && std::fs::rename(&tmp, &file).is_ok()
        };
        png(&full.thumbnail(THUMB_PX, THUMB_PX), self.thumb_file(id)) && png(&full, self.image_file(id))
    }

    /// Drop the oldest unpinned entries past the limits, with their image files.
    fn trim(&self, saved: &mut Saved) {
        let (mut items, mut images) = (0, 0);
        let mut gone = Vec::new();
        saved.items.retain(|i| {
            if i.pinned {
                return true;
            }
            items += 1;
            images += usize::from(i.kind == Kind::Image);
            let keep = items <= MAX_ITEMS && (i.kind != Kind::Image || images <= MAX_IMAGES);
            if !keep && i.kind == Kind::Image {
                gone.push(i.id.clone());
            }
            keep
        });
        for id in gone {
            self.remove_files(&id);
        }
    }

    /// Put an entry back on the clipboard.
    pub fn copy(&self, id: &str) -> Result<(), String> {
        let item = self.saved.lock().items.iter().find(|i| i.id == id).cloned().ok_or("That entry is no longer in the history.")?;
        let mut slot = self.clipboard.lock();
        if slot.is_none() {
            *slot = Some(Clipboard::new().map_err(|e| e.to_string())?);
        }
        let cb = slot.as_mut().expect("opened above");
        let done = match item.kind {
            Kind::Image => {
                let img = image::open(self.image_file(id)).map_err(|e| e.to_string())?.to_rgba8();
                let (w, h) = img.dimensions();
                cb.set_image(ImageData { width: w as usize, height: h as usize, bytes: Cow::Owned(img.into_raw()) })
            }
            Kind::Text | Kind::Link => cb.set_text(item.text.unwrap_or_default()),
        };
        done.map_err(|e| e.to_string())?;
        *self.own_change.lock() = sys::sequence();
        Ok(())
    }

    pub fn pin(&self, id: &str, pinned: bool) {
        let mut saved = self.saved.lock();
        if let Some(i) = saved.items.iter_mut().find(|i| i.id == id) {
            i.pinned = pinned;
        }
        self.trim(&mut saved);
        self.save(&saved);
    }

    pub fn delete(&self, id: &str) {
        let mut saved = self.saved.lock();
        saved.items.retain(|i| i.id != id);
        self.remove_files(id);
        self.save(&saved);
    }

    /// Remove everything except pinned entries, and reset the skipped count.
    pub fn clear(&self) {
        let mut saved = self.saved.lock();
        let gone: Vec<String> = saved.items.iter().filter(|i| !i.pinned).map(|i| i.id.clone()).collect();
        saved.items.retain(|i| i.pinned);
        saved.skipped = 0;
        for id in gone {
            self.remove_files(&id);
        }
        self.save(&saved);
    }
}

/// Read what is on the clipboard: text first (a spreadsheet copy also carries a
/// picture of the cells), then an image. Retries while another app holds it open.
fn read() -> Option<Content> {
    for attempt in 0..3 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(60));
        }
        let Ok(mut cb) = Clipboard::new() else { continue };
        if let Ok(t) = cb.get_text() {
            if !t.trim().is_empty() {
                return (t.len() <= MAX_TEXT).then_some(Content::Text(t));
            }
        }
        match cb.get_image() {
            Ok(img) => return Some(Content::Image(img)),
            Err(arboard::Error::ContentNotAvailable) => return None,
            Err(_) => continue,
        }
    }
    None
}

/// Watch for copies. Windows keeps a change counter, so the clipboard itself is only
/// opened when something new arrives; elsewhere its content is compared each second.
pub fn spawn_watcher(app: AppHandle, history: Arc<History>) {
    std::thread::spawn(move || {
        let mut last_seq = sys::sequence();
        let mut last_id: Option<String> = None;
        loop {
            std::thread::sleep(Duration::from_millis(if last_seq.is_some() { 450 } else { 1000 }));
            let seq = sys::sequence();
            if seq.is_some() && seq == last_seq {
                continue;
            }
            last_seq = seq;
            if !history.enabled.load(Ordering::Relaxed) {
                continue;
            }
            if sys::is_secret() {
                history.count_skipped();
                let _ = app.emit("clipboard", Change { fresh: false, skipped: true });
                continue;
            }
            let Some(content) = read() else { continue };
            if seq.is_none() {
                let id = match &content {
                    Content::Text(t) => content_id(b't', t.as_bytes()),
                    Content::Image(i) => content_id(b'i', &i.bytes),
                };
                if last_id.as_ref() == Some(&id) {
                    continue;
                }
                last_id = Some(id);
            }
            let own = seq.is_some() && *history.own_change.lock() == seq;
            history.add(content, if own { None } else { sys::owner_app() });
            let _ = app.emit("clipboard", Change { fresh: !own, skipped: false });
        }
    });
}

/// Copy an entry, give the keyboard back to the app that had it, and paste there.
pub fn paste(history: &History, app: &AppHandle, id: &str) -> Result<(), String> {
    history.copy(id)?;
    if let Some(win) = island::window(app) {
        island::set_activating(&win, false);
    }
    // Give the target app a moment to take focus before the keystroke arrives.
    std::thread::sleep(Duration::from_millis(90));
    sys::send_paste().map_err(|e| e.to_string())
}
