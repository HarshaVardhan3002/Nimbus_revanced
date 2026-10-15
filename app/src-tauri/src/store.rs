//! Small values the UI keeps between runs (to-dos, starred apps, the weather
//! place), one JSON file per key in the app's data folder.

use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const MAX_BYTES: usize = 512 * 1024;

/// Write then rename, so a crash mid-write never leaves a half file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn path(app: &AppHandle, key: &str) -> Result<PathBuf, String> {
    let valid = !key.is_empty()
        && key.len() <= 40
        && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if !valid {
        return Err(format!("not a valid key: {key:?}"));
    }
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("store").join(format!("{key}.json")))
}

pub fn get(app: &AppHandle, key: &str) -> Result<Option<Value>, String> {
    match std::fs::read_to_string(path(app, key)?) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn set(app: &AppHandle, key: &str, value: &Value) -> Result<(), String> {
    let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if json.len() > MAX_BYTES {
        return Err(format!("{key} is too large to keep ({} KB)", json.len() / 1024));
    }
    write_atomic(&path(app, key)?, &json)
}
