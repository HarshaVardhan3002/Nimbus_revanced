//! System controls for the widget, with one native backend per OS.
//!
//! Every setter reads the value back and returns what the system actually reports,
//! so callers (the UI and the AI tools) never have to assume an action worked.

pub mod apps;
pub mod brightness;
pub mod clipboard;
pub mod gpu;
pub mod media;
pub mod meter;
pub mod palette;
pub mod power;
pub mod radio;
pub mod stats;
pub mod theme;
pub mod volume;
pub mod wallpaper;
pub mod wallpaper_engine;

#[cfg(not(windows))]
mod cmd;
#[cfg(windows)]
mod winutil;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not supported on this system: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        Error::Failed(e.message().to_string())
    }
}

/// Clamp a percentage given by a person or a model to 0..=100.
pub fn clamp_percent(v: i64) -> u8 {
    v.clamp(0, 100) as u8
}
