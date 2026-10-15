//! The local brain: a model on this computer's own graphics chip, served by llama.cpp's
//! server, set up and started by Nimbus so the chat works with nothing to install.
//!
//! On start Nimbus looks at the system and its graphics chips (runtime.rs), picks the
//! llama.cpp build for them and the best model that fits their memory (models.rs), and
//! downloads whichever of the two is missing. A GGUF model put in Nimbus's models folder
//! is found on its own and can be chosen in Settings. The server runs hidden on its own
//! port and, like Lemonade, keeps running when Nimbus quits, so the next start is instant.
//!
//! Unless Settings say to keep it loaded, the model is on the chip only while it is used:
//! the first message loads it (`wake`), and after `brain_idle_minutes` with nothing asked
//! it is unloaded again, giving the chip's memory back (`spawn_idle`).
//!
//! Tuned for speed per watt: the whole model on the graphics chip (far less energy per
//! word than the processor), flash attention, one conversation whose prompt stays cached
//! between turns, no thinking out loud, and server threads that sleep instead of spinning
//! while the chip works (`--poll 0`: on this AMD chip 22.9 words a second instead of 20.0,
//! with less processor time, because a spinning processor takes power from the chip it
//! shares with).

use crate::models::{self, Found};
use crate::runtime::{self, Backend, Choice};
use crate::settings::Settings;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// The brain's own port, out of the way of the usual servers.
pub const URL: &str = "http://localhost:8091/v1";
const PORT: &str = "8091";

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Brain {
    /// Not needed: a chat address is set, another server has a model, or it is switched off.
    Off,
    /// No server program or no model file, and none is being downloaded.
    Missing { message: String },
    /// Downloading what it needs.
    Installing { what: String, done: u64, total: u64 },
    Starting { model: String },
    Ready { model: String },
    /// Unloaded to free the chip; the next message loads it again.
    Asleep { model: String },
    Failed { message: String },
}

pub type Status = parking_lot::Mutex<Brain>;

pub fn new() -> Status {
    parking_lot::Mutex::new(Brain::Off)
}

/// One preparation at a time; downloads stop when this is set.
static BUSY: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

/// Runs using the model now, and when the last one ended (seconds since 1970).
static IN_USE: AtomicUsize = AtomicUsize::new(0);
static LAST_USE: AtomicU64 = AtomicU64::new(0);

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Held while a run uses the model: it is not unloaded under it, and the idle time
/// counts from when the last one ends.
pub struct InUse;

pub fn in_use() -> InUse {
    IN_USE.fetch_add(1, Ordering::AcqRel);
    LAST_USE.store(now_secs(), Ordering::Release);
    InUse
}

/// Whether a request is being worked on now.
pub fn working() -> bool {
    IN_USE.load(Ordering::Acquire) > 0
}

impl Drop for InUse {
    fn drop(&mut self) {
        LAST_USE.store(now_secs(), Ordering::Release);
        IN_USE.fetch_sub(1, Ordering::AcqRel);
    }
}

fn set(app: &AppHandle, now: Brain) {
    *app.state::<crate::AppState>().brain.lock() = now.clone();
    let _ = app.emit("brain", now);
}

fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

fn ram() -> u64 {
    use sysinfo::{MemoryRefreshKind, RefreshKind, System};
    System::new_with_specifics(RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram())).total_memory()
}

/// Where and on what the brain runs here.
pub fn choice(s: &Settings) -> Choice {
    runtime::choose(&nimbus_system::gpu::adapters(), ram(), s.brain_backend)
}

/// The llama.cpp server program: the one set in the settings, Nimbus's own build for this
/// chip, a PrismML build made by hand in ~/prism-llama, one on the PATH, or Nimbus's
/// build of another kind.
fn find_server(app: &AppHandle, s: &Settings, b: Backend) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
    let mut places: Vec<PathBuf> = s.brain_server.iter().map(PathBuf::from).collect();
    places.extend(runtime::installed(app, b));
    if let Some(h) = home() {
        places.push(h.join("prism-llama/src/build-vk/bin").join(exe));
        places.push(h.join("prism-llama/src/build/bin").join(exe));
    }
    if let Some(path) = std::env::var_os("PATH") {
        places.extend(std::env::split_paths(&path).map(|d| d.join(exe)));
    }
    for other in [Backend::Vulkan, Backend::Cpu] {
        places.extend(runtime::installed(app, other));
    }
    places.into_iter().find(|p| p.is_file())
}

/// The model chosen in Settings, else the best one found that fits.
fn find_model(app: &AppHandle, s: &Settings, c: &Choice) -> Option<Found> {
    let found = models::discover(app);
    if let Some(chosen) = &s.brain_model {
        if let Some(f) = found.iter().find(|f| Path::new(&f.path) == Path::new(chosen)) {
            return Some(f.clone());
        }
        // Chosen from somewhere else than the places Nimbus looks.
        let p = Path::new(chosen);
        let mut one = Vec::new();
        if let (Some(dir), Some(name)) = (p.parent(), p.file_name()) {
            models::find_in(dir, 0, false, &mut one);
            one.retain(|f| Path::new(&f.path).file_name() == Some(name));
        }
        if let Some(f) = one.pop() {
            return Some(f);
        }
    }
    models::pick(&found, c.backend, c.chip.as_ref().map(|a| a.vendor), c.usable).cloned()
}

fn health() -> Option<u16> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(600)))
        .timeout_recv_response(Some(Duration::from_secs(3)))
        .http_status_as_error(false)
        .build()
        .into();
    agent.get(&format!("http://localhost:{PORT}/health")).call().ok().map(|r| r.status().as_u16())
}

/// The name of the model the running server has loaded.
fn served() -> Option<String> {
    let text = ureq::get(&format!("http://localhost:{PORT}/v1/models")).call().ok()?.body_mut().read_to_string().ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["data"][0]["id"].as_str().map(str::to_string)
}

/// The server's arguments. The context is what the settings ask for, cut to what fits
/// beside the model; when the model itself does not fit, llama.cpp's own fitting keeps
/// as many layers on the chip as it can.
fn args(m: &Found, c: &Choice, wanted_context: u32, ram: u64) -> Vec<String> {
    let kv = m.kv.unwrap_or(147_456);
    let ctx = models::context_for(m.size, kv, m.context.unwrap_or(32_768), c.usable, wanted_context);
    let mut a: Vec<String> = vec!["-m".into(), m.path.clone(), "--alias".into(), m.name.clone()];
    // The model's eyes, when its vision file is beside it. Small pictures make it point
    // badly, so each picture gets at least 1,024 tokens.
    if let Some(eyes) = models::vision_for(&m.path) {
        a.extend(["--mmproj".into(), eyes, "--image-min-tokens".into(), "1024".into()]);
    }
    if c.backend.on_gpu() {
        if m.needs(ctx) <= c.usable {
            a.extend(["-ngl".into(), "99".into()]);
        }
        // Threads wait for the chip asleep instead of spinning.
        a.extend(["--poll".into(), "0".into()]);
    }
    // Earlier conversations' prompts kept in memory: up to an eighth of it, 24 GB at most.
    let cache_mib = (ram / 8 / (1 << 20)).min(24_576);
    // Saved points in a conversation, about 150 MB each for Bonsai 2: fewer on small machines.
    let checkpoints = if ram >= 32 << 30 { "32" } else { "16" };
    a.extend(
        [
            "-fa", "on", "-c", &ctx.to_string(), "-np", "1", "--cache-ram", &cache_mib.to_string(),
            // Hybrid models (Bonsai 2) cannot step back in a conversation without a saved
            // point. llama.cpp keeps one at the start of the first question (so a new
            // conversation reuses the tools and instructions) and one at the end of each
            // request; older ones closer than 1,024 tokens are thinned out, so the first
            // one is not pushed out by a long agent run.
            "--checkpoint-min-step", "1024", "--ctx-checkpoints", checkpoints,
            "--port", PORT, "--host", "127.0.0.1", "--jinja", "--no-webui",
            "--chat-template-kwargs", r#"{"enable_thinking":false}"#,
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    a
}

fn launch(app: &AppHandle, exe: &Path, args: &[String]) -> std::io::Result<()> {
    // What the server says goes to a log, for when it does not start.
    let log = app.path().app_log_dir().ok().and_then(|d| {
        std::fs::create_dir_all(&d).ok()?;
        std::fs::File::create(d.join("brain.log")).ok()
    });
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null());
    match log.and_then(|f| Some((f.try_clone().ok()?, f))) {
        Some((out, err)) => cmd.stdout(out).stderr(err),
        None => cmd.stdout(Stdio::null()).stderr(Stdio::null()),
    };
    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }
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

/// The llama-server processes on the brain's port.
fn our_servers(sys: &sysinfo::System) -> impl Iterator<Item = &sysinfo::Process> {
    sys.processes().values().filter(|p| {
        p.name().to_string_lossy().to_lowercase().starts_with("llama-server") && p.cmd().windows(2).any(|w| w[0] == "--port" && w[1] == PORT)
    })
}

fn processes() -> sysinfo::System {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};
    System::new_with_specifics(RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always)))
}

/// The arguments the running brain server was started with (without the program).
fn running_args() -> Option<Vec<String>> {
    let sys = processes();
    let p = our_servers(&sys).next()?;
    Some(p.cmd().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect())
}

/// End the brain's server, to load another model or start it with other settings.
fn stop_server() {
    let sys = processes();
    for p in our_servers(&sys) {
        p.kill();
    }
    let began = Instant::now();
    while health().is_some() && began.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Download the server build and the recommended model, whichever is missing.
fn install(app: &AppHandle, c: &Choice, server: bool, model: bool) -> Result<(), String> {
    STOP.store(false, Ordering::Relaxed);
    if server {
        let what = format!("the model server for {}", label(c.backend));
        runtime::install(app, c.backend, &STOP, |_, done, total| set(app, Brain::Installing { what: what.clone(), done, total }))?;
    }
    if model {
        let e = models::recommend(c.backend, c.chip.as_ref().map(|a| a.vendor), c.usable);
        let (url, sha, _) = models::source(e, c.backend)?;
        let dest = models::folder(app).ok_or("Nimbus's data folder is not available")?.join(e.file(c.backend).0);
        crate::fetch::download(&url, &dest, sha.as_deref(), &STOP, |done, total| set(app, Brain::Installing { what: e.name.into(), done, total }))?;
    }
    Ok(())
}

pub fn label(b: Backend) -> &'static str {
    match b {
        Backend::Cuda => "NVIDIA CUDA",
        Backend::Vulkan => "Vulkan",
        Backend::Rocm => "AMD ROCm",
        Backend::Metal => "Apple Metal",
        Backend::Cpu => "the processor",
    }
}

/// Make the brain ready if the chat needs it: on Nimbus's start (downloading what is
/// missing when the settings allow it), after a model is chosen, or when asked to set up.
/// With `load` false, everything is made ready but the model is left off the chip until a
/// message needs it.
pub fn prepare(app: &AppHandle, setup: bool, load: bool) {
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    run(app, setup, load);
    BUSY.store(false, Ordering::Release);
}

/// How long the model may sit unused before it is unloaded; `None` keeps it loaded.
fn idle_limit(s: &Settings) -> Option<Duration> {
    (s.brain_idle_minutes > 0).then(|| Duration::from_secs(s.brain_idle_minutes as u64 * 60))
}

/// Loads the model onto the chip if a message needs it and it is not there (asleep, or
/// its server gone), and waits until it answers or says why not.
pub fn wake(app: &AppHandle) -> Result<(), String> {
    let s = app.state::<crate::AppState>().settings.lock().clone();
    if !s.local_brain || s.chat_url.is_some() {
        return Ok(());
    }
    let state = app.state::<crate::AppState>().brain.lock().clone();
    let ours = matches!(state, Brain::Asleep { .. } | Brain::Ready { .. } | Brain::Starting { .. });
    if !ours || health() == Some(200) {
        return Ok(());
    }
    // Another start may be under way (Nimbus starting, Settings): wait for it, then look again.
    let began = Instant::now();
    while BUSY.load(Ordering::Acquire) && began.elapsed() < Duration::from_secs(300) {
        std::thread::sleep(Duration::from_millis(300));
    }
    if health() != Some(200) {
        prepare(app, false, true);
    }
    match app.state::<crate::AppState>().brain.lock().clone() {
        Brain::Failed { message } | Brain::Missing { message } => Err(format!("the local brain did not start: {message}")),
        _ => Ok(()),
    }
}

/// Unloads the model when nothing has used it for the time set in Settings.
pub fn spawn_idle(app: AppHandle) {
    LAST_USE.store(now_secs(), Ordering::Release);
    std::thread::Builder::new()
        .name("brain-idle".into())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(30));
            let s = app.state::<crate::AppState>().settings.lock().clone();
            let Some(limit) = idle_limit(&s) else { continue };
            let idle = now_secs().saturating_sub(LAST_USE.load(Ordering::Acquire));
            if IN_USE.load(Ordering::Acquire) > 0 || idle < limit.as_secs() {
                continue;
            }
            let Brain::Ready { model } = app.state::<crate::AppState>().brain.lock().clone() else { continue };
            if BUSY.swap(true, Ordering::AcqRel) {
                continue;
            }
            // Only a server whose arguments Nimbus can read is one it started.
            if IN_USE.load(Ordering::Acquire) == 0 && running_args().is_some() {
                stop_server();
                set(&app, Brain::Asleep { model });
            }
            BUSY.store(false, Ordering::Release);
        })
        .ok();
}

/// On quit: a model loaded only while used is not left on the chip.
pub fn on_exit(app: &AppHandle) {
    let s = app.state::<crate::AppState>().settings.lock().clone();
    if idle_limit(&s).is_some() && s.local_brain && s.chat_url.is_none() {
        let sys = processes();
        for p in our_servers(&sys) {
            p.kill();
        }
    }
}

/// The local brain as the chat server while it is asleep: the next message wakes it.
pub fn asleep_status(app: &AppHandle) -> Option<crate::chat::Status> {
    let Brain::Asleep { model } = app.state::<crate::AppState>().brain.lock().clone() else { return None };
    Some(crate::chat::Status { url: Some(URL.into()), server: "The local brain".into(), models: vec![model.clone()], model: Some(model), custom: false, error: None })
}

fn run(app: &AppHandle, setup: bool, load: bool) {
    let s = app.state::<crate::AppState>().settings.lock().clone();
    if !s.local_brain || s.chat_url.is_some() {
        return set(app, Brain::Off);
    }
    let c = choice(&s);
    let mut model = find_model(app, &s, &c);
    if health() == Some(200) {
        let running = served();
        let want = model.as_ref().map(|m| args(m, &c, s.context_window, ram()));
        // A llama-server on the brain's port is restarted when its model or settings are
        // not what Nimbus would start now. One whose arguments cannot be read (another
        // user's) is left alone unless a different model was chosen in Settings.
        let keep = match (running_args(), &want) {
            (Some(now), Some(want)) => &now == want,
            (None, _) => s.brain_model.is_none() || running.as_deref() == model.as_ref().map(|m| m.name.as_str()),
            (Some(_), None) => true,
        };
        if keep {
            return set(app, Brain::Ready { model: running.unwrap_or_else(|| "local model".into()) });
        }
        stop_server();
    } else if s.brain_model.is_none() {
        // Another server on this computer already offers a chat model: use that instead.
        let other = crate::chat::status(None, "", None);
        if other.model.is_some() && other.url.as_deref() != Some(URL) {
            return set(app, Brain::Off);
        }
    }
    let mut exe = find_server(app, &s, c.backend);
    if exe.is_none() || model.is_none() {
        let e = models::recommend(c.backend, c.chip.as_ref().map(|a| a.vendor), c.usable);
        if !setup {
            let what = match (&exe, &model) {
                (None, None) => format!("the model server and {} ({})", e.name, gb(e.file(c.backend).1)),
                (None, Some(_)) => format!("the model server for {}", label(c.backend)),
                _ => format!("{} ({})", e.name, gb(e.file(c.backend).1)),
            };
            return set(app, Brain::Missing { message: format!("nothing to run yet; Nimbus can download {what} in Settings") });
        }
        if let Err(e) = install(app, &c, exe.is_none(), model.is_none()) {
            let message = if e == "stopped" { "the download was stopped; it continues from where it was".into() } else { format!("the download did not finish: {e}") };
            return set(app, Brain::Missing { message });
        }
        exe = find_server(app, &s, c.backend);
        model = find_model(app, &s, &c);
    }
    let (Some(exe), Some(model)) = (exe, model) else {
        return set(app, Brain::Failed { message: "the download finished, but its files were not found".into() });
    };
    if !load {
        return set(app, Brain::Asleep { model: model.name });
    }
    set(app, Brain::Starting { model: model.name.clone() });
    if let Err(e) = launch(app, &exe, &args(&model, &c, s.context_window, ram())) {
        return set(app, Brain::Failed { message: format!("the model server did not start: {e}") });
    }
    // Loading several gigabytes onto the GPU takes a few seconds, more from a cold disk.
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(240) {
        std::thread::sleep(Duration::from_millis(800));
        if health() == Some(200) {
            return set(app, Brain::Ready { model: model.name });
        }
    }
    set(app, Brain::Failed { message: "the model server started but did not finish loading; see brain.log".into() })
}

/// Stop a download that is under way.
pub fn stop_setup() {
    STOP.store(true, Ordering::Relaxed);
}

/// What Settings shows: the chip, the build, the models found and which runs.
#[derive(Debug, Serialize)]
pub struct Plan {
    pub chip: Option<String>,
    pub backend: Backend,
    pub backend_label: &'static str,
    pub usable: u64,
    /// The server program in use, and its release when Nimbus installed it.
    pub server: Option<String>,
    pub release: Option<String>,
    pub recommended: Recommended,
    pub models: Vec<Listed>,
    pub chosen: Option<String>,
    pub folder: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Recommended {
    pub name: &'static str,
    pub size: u64,
    pub present: bool,
}

#[derive(Debug, Serialize)]
pub struct Listed {
    pub path: String,
    pub name: String,
    pub size: u64,
    /// Fits wholly on the chip with a workable context.
    pub fits: bool,
    /// The one that runs when none is chosen.
    pub automatic: bool,
}

pub fn plan(app: &AppHandle) -> Plan {
    let s = app.state::<crate::AppState>().settings.lock().clone();
    let c = choice(&s);
    let vendor = c.chip.as_ref().map(|a| a.vendor);
    let found = models::discover(app);
    let auto = models::pick(&found, c.backend, vendor, c.usable).map(|f| f.path.clone());
    let e = models::recommend(c.backend, vendor, c.usable);
    let server = find_server(app, &s, c.backend);
    Plan {
        chip: c.chip.as_ref().map(|a| a.name.clone()),
        backend: c.backend,
        backend_label: label(c.backend),
        usable: c.usable,
        release: server.as_ref().filter(|p| runtime::root(app).is_some_and(|r| p.starts_with(r))).and_then(|_| runtime::version(app, c.backend)),
        server: server.map(|p| p.to_string_lossy().into_owned()),
        recommended: Recommended { name: e.name, size: e.file(c.backend).1, present: found.iter().any(|f| f.known == Some(e.id)) },
        models: found
            .iter()
            .map(|f| Listed { path: f.path.clone(), name: f.name.clone(), size: f.size, fits: f.needs(models::MIN_CONTEXT) <= c.usable, automatic: auto.as_deref() == Some(&f.path) })
            .collect(),
        chosen: s.brain_model.clone(),
        folder: models::folder(app).map(|p| p.to_string_lossy().into_owned()),
    }
}

/// Watch the models folder: a model put there shows in Settings ("brain-models"), and
/// starts the brain if it had nothing to run. Waits until a copy has finished.
pub fn watch(app: AppHandle) {
    use notify::{RecursiveMode, Watcher};
    let Some(dir) = models::folder(&app) else { return };
    let _ = std::fs::create_dir_all(&dir);
    std::thread::Builder::new()
        .name("models-watch".into())
        .spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let Ok(mut w) = notify::recommended_watcher(tx) else { return };
            if w.watch(&dir, RecursiveMode::Recursive).is_err() {
                return;
            }
            let sizes = |dir: &Path| {
                let mut v = Vec::new();
                for e in walk(dir) {
                    v.push((e.clone(), std::fs::metadata(&e).map(|m| m.len()).unwrap_or(0)));
                }
                v
            };
            while let Ok(ev) = rx.recv() {
                let Ok(ev) = ev else { continue };
                if !ev.paths.iter().any(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gguf"))) {
                    continue;
                }
                // Until nothing has changed for three seconds: a copy of several gigabytes
                // is still growing.
                let mut last = sizes(&dir);
                loop {
                    std::thread::sleep(Duration::from_secs(3));
                    while rx.try_recv().is_ok() {}
                    let now = sizes(&dir);
                    if now == last {
                        break;
                    }
                    last = now;
                }
                let _ = app.emit("brain-models", ());
                let state = app.state::<crate::AppState>().brain.lock().clone();
                if matches!(state, Brain::Missing { .. } | Brain::Failed { .. }) {
                    let app = app.clone();
                    std::thread::spawn(move || prepare(&app, false, false));
                }
            }
        })
        .ok();
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gguf")) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nimbus_system::gpu::{Adapter, Vendor};

    fn found(path: &str, size: u64) -> Found {
        Found { path: path.into(), name: "Ternary-Bonsai-2-27B-PQ2_0".into(), size, known: Some("bonsai2-27b"), context: Some(262_144), kv: Some(65_536) }
    }

    #[test]
    fn arguments() {
        let chip = Adapter { name: "Radeon".into(), vendor: Vendor::Amd, dedicated: 64 << 30, shared: 0, integrated: true };
        let c = Choice { backend: Backend::Vulkan, chip: Some(chip), usable: 57_000_000_000 };
        let a = args(&found("C:/m/Ternary-Bonsai-2-27B-PQ2_0.gguf", 7_206_168_928), &c, 125_000, 64 << 30);
        let after = |a: &[String], flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
        assert_eq!(after(&a, "--alias").as_deref(), Some("Ternary-Bonsai-2-27B-PQ2_0"));
        assert_eq!(after(&a, "-c").as_deref(), Some("124928"));
        assert_eq!(after(&a, "-ngl").as_deref(), Some("99"));
        assert_eq!(after(&a, "--poll").as_deref(), Some("0"));
        assert_eq!(after(&a, "--cache-ram").as_deref(), Some("8192"));
        assert_eq!(after(&a, "--checkpoint-min-step").as_deref(), Some("1024"));
        assert_eq!(after(&a, "--port").as_deref(), Some(PORT));
        assert!(a.iter().any(|x| x == "--jinja"));
        assert_eq!(after(&a, "--mmproj"), None);
        // A vision file beside the model, or one folder down, is loaded with it.
        let dir = std::env::temp_dir().join(format!("nimbus-eyes-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Bonsai-2-vision")).unwrap();
        let model = dir.join("Ternary-Bonsai-2-27B-PQ2_0.gguf");
        std::fs::write(&model, b"").unwrap();
        std::fs::write(dir.join("Other-7B-mmproj-Q8_0.gguf"), b"").unwrap();
        let eyes = dir.join("Bonsai-2-vision").join("Ternary-Bonsai-2-27B-mmproj-Q8_0.gguf");
        std::fs::write(&eyes, b"").unwrap();
        let a = args(&found(model.to_str().unwrap(), 7_206_168_928), &c, 125_000, 64 << 30);
        assert_eq!(after(&a, "--mmproj").as_deref(), eyes.to_str());
        assert_eq!(after(&a, "--image-min-tokens").as_deref(), Some("1024"));
        let _ = std::fs::remove_dir_all(&dir);
        // Too big for the chip: no layer count, llama.cpp fits what it can.
        let small = Choice { usable: 6_000_000_000, ..c.clone() };
        let a = args(&found("m.gguf", 7_206_168_928), &small, 125_000, 16 << 30);
        assert_eq!(after(&a, "-ngl"), None);
        assert_eq!(after(&a, "-c").as_deref(), Some("8192"));
        // On the processor: no GPU flags.
        let cpu = Choice { backend: Backend::Cpu, chip: None, usable: 14_000_000_000 };
        let a = args(&found("m.gguf", 2_180_000_000), &cpu, 125_000, 32 << 30);
        assert_eq!(after(&a, "-ngl"), None);
        assert_eq!(after(&a, "--poll"), None);
    }
}
