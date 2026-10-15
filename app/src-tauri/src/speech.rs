//! Speech to text through Lemonade, with nothing for the user to do: when Nimbus starts
//! it makes sure Lemonade is running (starting it hidden if not) and that a Whisper
//! model is loaded. The other pages read the result as a status line.
//!
//! Lemonade is left running when Nimbus quits: other apps may be using it too.

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Where Lemonade listens: its current port first, then the one older versions used.
pub const URLS: [&str; 2] = ["http://localhost:13305/api/v1", "http://localhost:8000/api/v1"];

/// Whisper models in the order we prefer them. Small wrote a short request in 1.0 s on
/// this laptop, where Turbo took 3.7 s for the same words (measured 2026-10-14), and is
/// a third of the download.
const PREFERRED: [&str; 3] = ["Whisper-Small", "Whisper-Large-v3-Turbo", "Whisper-Large-v3"];

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Speech {
    /// Looking for Lemonade, or starting it.
    Starting,
    /// Downloading a Whisper model (first run only).
    Downloading { model: String },
    /// Loading the model into memory.
    Loading { model: String },
    Ready { model: String, url: String },
    /// Lemonade is not installed on this computer.
    Missing,
    Failed { message: String },
}

pub type Status = Arc<Mutex<Speech>>;

pub fn new() -> Status {
    Arc::new(Mutex::new(Speech::Starting))
}

fn set(app: &AppHandle, status: &Status, s: Speech) {
    *status.lock() = s.clone();
    let _ = app.emit("speech", s);
}

fn agent(answer_within: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(800)))
        .timeout_recv_response(Some(answer_within))
        .http_status_as_error(false)
        .build()
        .into()
}

fn get(url: &str) -> Option<Value> {
    let mut res = agent(Duration::from_secs(4)).get(url).call().ok()?;
    if res.status().as_u16() >= 400 {
        return None;
    }
    serde_json::from_str(&res.body_mut().read_to_string().ok()?).ok()
}

fn post(url: &str, body: Value, within: Duration) -> Result<Value, String> {
    let mut res = agent(within)
        .post(url)
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status >= 400 || v.get("error").is_some() {
        let said = v.pointer("/error/message").or_else(|| v.get("error")).and_then(Value::as_str);
        return Err(said.map(str::to_string).unwrap_or_else(|| format!("Lemonade answered with error {status}")));
    }
    Ok(v)
}

/// The address of a Lemonade that answers, if one does.
fn running() -> Option<&'static str> {
    URLS.into_iter().find(|u| get(&format!("{u}/health")).is_some_and(|h| h.get("status").and_then(Value::as_str) == Some("ok")))
}

/// Lemonade's server program, where its installers put it.
fn find_server() -> Option<(PathBuf, Vec<&'static str>)> {
    let mut places: Vec<(PathBuf, Vec<&'static str>)> = Vec::new();
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            places.push((PathBuf::from(local).join("lemonade_server/bin/LemonadeServer.exe"), vec![]));
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var) {
                places.push((PathBuf::from(pf).join("Lemonade Server/bin/LemonadeServer.exe"), vec![]));
            }
        }
    }
    #[cfg(not(windows))]
    {
        for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/opt/lemonade/bin"] {
            places.push((PathBuf::from(dir).join("lemonade-server"), vec!["serve"]));
        }
        if let Some(home) = std::env::var_os("HOME") {
            places.push((PathBuf::from(home).join(".local/bin/lemonade-server"), vec!["serve"]));
        }
    }
    if let Some(found) = places.into_iter().find(|(p, _)| p.is_file()) {
        return Some(found);
    }
    // Anywhere on PATH.
    let name = if cfg!(windows) { "LemonadeServer.exe" } else { "lemonade-server" };
    let args = if cfg!(windows) { vec![] } else { vec!["serve"] };
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .map(|p| (p, args))
}

/// Start Lemonade with no window, on its own: it keeps running when Nimbus quits.
fn launch(exe: &PathBuf, args: &[&str]) -> std::io::Result<()> {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map(|_| ())
}

/// The transcription model loaded right now, if any.
fn loaded(url: &str) -> Option<String> {
    let health = get(&format!("{url}/health"))?;
    health
        .get("all_models_loaded")?
        .as_array()?
        .iter()
        .find(|m| m.get("type").and_then(Value::as_str) == Some("transcription"))
        .and_then(|m| m.get("model_name").and_then(Value::as_str))
        .map(str::to_string)
}

/// Whisper models already on this computer.
fn downloaded(url: &str) -> Vec<String> {
    let Some(list) = get(&format!("{url}/models")) else { return vec![] };
    list.get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter(|m| {
                    let labels = m.get("labels").and_then(Value::as_array);
                    labels.is_some_and(|l| l.iter().any(|l| l.as_str() == Some("transcription")))
                        && m.get("downloaded").and_then(Value::as_bool) != Some(false)
                })
                .filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Make speech to text ready. Runs once, on its own thread, when Nimbus starts; `chosen`
/// is the Whisper model picked in Settings, if any.
pub fn prepare(app: &AppHandle, status: &Status, chosen: Option<String>) {
    set(app, status, Speech::Starting);

    let url = match running() {
        Some(u) => u,
        None => {
            let Some((exe, args)) = find_server() else {
                set(app, status, Speech::Missing);
                return;
            };
            if let Err(e) = launch(&exe, &args) {
                set(app, status, Speech::Failed { message: format!("Lemonade did not start: {e}") });
                return;
            }
            // A cold start loads its backends first: allow it a minute and a half.
            let began = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(1000));
                if let Some(u) = running() {
                    break u;
                }
                if began.elapsed() > Duration::from_secs(90) {
                    set(app, status, Speech::Failed { message: "Lemonade started but did not answer".into() });
                    return;
                }
            }
        }
    };

    let have = downloaded(url);
    // A Whisper model already loaded is kept when it is the one Settings asks for, or,
    // with no choice made, when no model we prefer more is on this computer.
    let current = loaded(url);
    if let Some(m) = &current {
        let better = PREFERRED.iter().take_while(|p| **p != m.as_str()).any(|p| have.iter().any(|h| h == *p));
        if chosen.as_deref().map_or(!better, |c| c == m) {
            set(app, status, Speech::Ready { model: m.clone(), url: url.into() });
            return;
        }
    }

    let model = chosen
        .filter(|c| have.contains(c))
        .or_else(|| PREFERRED.iter().find(|p| have.iter().any(|h| h == *p)).map(|p| p.to_string()))
        .or_else(|| have.first().cloned());
    let model = match model {
        Some(m) => m,
        None => {
            // First run with no Whisper at all: download the fast one.
            let m = PREFERRED[0].to_string();
            set(app, status, Speech::Downloading { model: m.clone() });
            if let Err(e) = post(&format!("{url}/pull"), json!({ "model_name": m }), Duration::from_secs(3 * 3600)) {
                set(app, status, Speech::Failed { message: format!("{m} did not download: {e}") });
                return;
            }
            m
        }
    };

    set(app, status, Speech::Loading { model: model.clone() });
    match post(&format!("{url}/load"), json!({ "model_name": model }), Duration::from_secs(300)) {
        Ok(_) => set(app, status, Speech::Ready { model, url: url.into() }),
        Err(e) => set(app, status, Speech::Failed { message: format!("{model} did not load: {e}") }),
    }
}
