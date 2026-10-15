//! What is on the screen, for models that can see.
//!
//! While seeing the screen is on (Settings) and Nimbus is working on a request, the main
//! screen is captured once a second into a queue of the last ten frames: when a new one
//! comes in the oldest is dropped. Between requests nothing is captured (each capture
//! is a screenshot scaled and compressed: once a second all day, a laptop's fans notice);
//! a tool that looks takes a frame there and then. Frames are small JPEGs held in memory only, never written to
//! disk, and the queue empties as soon as watching stops.
//!
//! Nimbus's own windows are hidden from capture, so they never show up in a frame.

use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// Frames kept, and how often one is taken.
pub const KEEP: usize = 10;
const EVERY: Duration = Duration::from_secs(1);
/// Frames are scaled to this width: sharp enough to read, light on tokens.
const WIDTH: u32 = 1280;
const QUALITY: u8 = 72;

#[derive(Clone)]
pub struct Frame {
    pub at: SystemTime,
    pub jpeg: Arc<Vec<u8>>,
    /// The frame's size in pixels.
    pub w: u32,
    pub h: u32,
    /// Screen pixels per frame pixel, to turn a point in the frame into a point on screen.
    pub scale: f64,
    /// The captured area's size in physical pixels (the screen, or one window).
    pub screen: (u32, u32),
    /// Where the captured area's top-left corner is on the desktop.
    pub origin: (i32, i32),
}

impl Frame {
    pub fn data_url(&self) -> String {
        format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(self.jpeg.as_slice()))
    }

    /// A point in this frame, on the screen.
    pub fn to_screen(&self, x: f64, y: f64) -> (i32, i32) {
        let sx = (x * self.scale).round().clamp(0.0, self.screen.0 as f64 - 1.0);
        let sy = (y * self.scale).round().clamp(0.0, self.screen.1 as f64 - 1.0);
        (self.origin.0 + sx as i32, self.origin.1 + sy as i32)
    }
}

#[derive(Default)]
pub struct Screen {
    frames: Mutex<VecDeque<Frame>>,
    /// Seeing the screen is on in Settings. Nothing is captured while it is off.
    pub watching: AtomicBool,
    /// Serialises captures, so a fresh one asked for by a tool and the timer's never race.
    taking: Mutex<()>,
}

fn primary() -> Result<xcap::Monitor, String> {
    let all = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let mut first = None;
    for m in all {
        if m.is_primary().unwrap_or(false) {
            return Ok(m);
        }
        first.get_or_insert(m);
    }
    first.ok_or_else(|| "no screen found".into())
}

/// Capture the main screen now.
pub fn take() -> Result<Frame, String> {
    let m = primary()?;
    let shot = m.capture_image().map_err(|e| format!("the screen could not be captured: {e}"))?;
    frame(shot, (m.x().unwrap_or(0), m.y().unwrap_or(0)))
}

/// Capture one window now, as it is drawn even where other windows cover it. Seeing the
/// screen must be on: this is a look at the screen like any other.
pub fn take_window(id: u32) -> Result<Frame, String> {
    let w = xcap::Window::all()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|w| w.id().ok() == Some(id))
        .ok_or("the window was closed")?;
    let shot = w.capture_image().map_err(|e| format!("the window could not be captured: {e}"))?;
    frame(shot, (w.x().unwrap_or(0), w.y().unwrap_or(0)))
}

fn frame(shot: image::RgbaImage, origin: (i32, i32)) -> Result<Frame, String> {
    let (sw, sh) = (shot.width(), shot.height());
    let img = image::DynamicImage::ImageRgba8(shot);
    let img = if sw > WIDTH { img.resize(WIDTH, u32::MAX, FilterType::Triangle) } else { img };
    let rgb = img.to_rgb8();
    let mut jpeg = Vec::with_capacity(160_000);
    JpegEncoder::new_with_quality(&mut jpeg, QUALITY).encode_image(&rgb).map_err(|e| e.to_string())?;
    Ok(Frame {
        at: SystemTime::now(),
        jpeg: Arc::new(jpeg),
        w: rgb.width(),
        h: rgb.height(),
        scale: sw as f64 / rgb.width() as f64,
        screen: (sw, sh),
        origin,
    })
}

impl Screen {
    fn active(&self) -> bool {
        self.watching.load(Ordering::Relaxed)
    }

    fn push(&self, f: Frame) {
        let mut q = self.frames.lock();
        q.push_back(f);
        while q.len() > KEEP {
            q.pop_front();
        }
    }

    pub fn clear(&self) {
        self.frames.lock().clear();
    }

    /// The newest frame, taken now if the queue has nothing from the last second.
    pub fn latest(&self) -> Result<Frame, String> {
        let _one = self.taking.lock();
        if let Some(f) = self.frames.lock().back() {
            if f.at.elapsed().unwrap_or_default() < EVERY + Duration::from_millis(200) {
                return Ok(f.clone());
            }
        }
        let f = take()?;
        self.push(f.clone());
        Ok(f)
    }

    /// The newest frame taken after something just happened on screen.
    pub fn fresh(&self) -> Result<Frame, String> {
        let _one = self.taking.lock();
        let f = take()?;
        self.push(f.clone());
        Ok(f)
    }

    /// Up to `n` frames, oldest first, spread over the queue: the last few seconds as
    /// a short film, for models that can follow change over time.
    pub fn recent(&self, n: usize) -> Result<Vec<Frame>, String> {
        let newest = self.latest()?;
        let q = self.frames.lock();
        let n = n.clamp(1, KEEP);
        if n == 1 || q.len() <= 1 {
            return Ok(vec![newest]);
        }
        let all: Vec<Frame> = q.iter().cloned().collect();
        let step = (all.len() - 1) as f64 / (n.min(all.len()) - 1).max(1) as f64;
        let mut picked: Vec<Frame> = (0..n.min(all.len())).map(|i| all[(i as f64 * step).round() as usize].clone()).collect();
        picked.dedup_by_key(|f| f.at);
        Ok(picked)
    }

    /// Frames kept right now (for Settings).
    pub fn count(&self) -> usize {
        self.frames.lock().len()
    }
}

/// The once-a-second capture while a request is worked on, on its own thread for the
/// life of the app.
pub fn spawn(screen: Arc<Screen>) {
    let _ = std::thread::Builder::new().name("screen".into()).spawn(move || {
        let mut idle_since: Option<Instant> = None;
        loop {
            let began = Instant::now();
            if screen.active() && crate::brain::working() {
                idle_since = None;
                if let Ok(f) = {
                    let _one = screen.taking.lock();
                    take()
                } {
                    screen.push(f);
                }
            } else if !screen.active() {
                // Not watching: what was seen is let go within a few seconds.
                let since = *idle_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_secs(3) && screen.count() > 0 {
                    screen.clear();
                }
            }
            std::thread::sleep(EVERY.saturating_sub(began.elapsed()));
        }
    });
}

/// The open windows, for models that cannot see: what is where, and which has focus.
pub fn windows() -> Result<String, String> {
    let all = xcap::Window::all().map_err(|e| e.to_string())?;
    let mut lines = Vec::new();
    for w in all {
        let title = w.title().unwrap_or_default();
        if title.trim().is_empty() || w.is_minimized().unwrap_or(false) {
            continue;
        }
        let (Ok(x), Ok(y), Ok(wd), Ok(ht)) = (w.x(), w.y(), w.width(), w.height()) else { continue };
        if wd < 80 || ht < 40 {
            continue;
        }
        let app = w.app_name().unwrap_or_default();
        let focus = if w.is_focused().unwrap_or(false) { " (focused)" } else { "" };
        lines.push(format!("- {title} [{app}] at {x},{y} size {wd}x{ht}{focus}"));
        if lines.len() >= 40 {
            break;
        }
    }
    Ok(if lines.is_empty() { "No windows are open.".into() } else { lines.join("\n") })
}
