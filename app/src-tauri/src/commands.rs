//! What the UI can call. System calls run on the blocking pool so the UI thread
//! never waits on COM, WinRT, the disk or a child process.

use crate::island::{self, HitArea, Rect};
use crate::{AppState, chat, clip, colors, launcher, privacy, routines, settings, store, thumbs};
use nimbus_system::palette::{self, Palette};
use nimbus_system::{apps, brightness, media, meter, power, radio, stats, theme, volume, wallpaper, wallpaper_engine};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

type Res<T> = Result<T, String>;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> nimbus_system::Result<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Like [`blocking`], for work that reports errors as plain text.
async fn task<T: Send + 'static>(f: impl FnOnce() -> Res<T> + Send + 'static) -> Res<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn volume_get() -> Res<volume::Volume> {
    blocking(volume::get).await
}

#[tauri::command]
pub async fn volume_set(percent: u8) -> Res<volume::Volume> {
    blocking(move || volume::set(percent)).await
}

#[tauri::command]
pub async fn volume_mute(muted: bool) -> Res<volume::Volume> {
    blocking(move || volume::set_muted(muted)).await
}

#[tauri::command]
pub async fn brightness_get() -> Res<u8> {
    blocking(brightness::get).await
}

#[tauri::command]
pub async fn brightness_set(percent: u8) -> Res<u8> {
    blocking(move || brightness::set(percent)).await
}

#[tauri::command]
pub async fn media_now(artwork: bool) -> Res<Option<media::NowPlaying>> {
    blocking(move || media::now_playing(artwork)).await
}

#[tauri::command]
pub async fn media_control(action: media::Action) -> Res<Option<media::NowPlaying>> {
    blocking(move || media::control(action)).await
}

/// How loud the speakers are right now, 0 to 1, for the music bars.
#[tauri::command]
pub fn audio_peak() -> f32 {
    meter::peak()
}

#[tauri::command]
pub async fn radios_get() -> Res<radio::Radios> {
    blocking(radio::get).await
}

#[tauri::command]
pub async fn radio_set(kind: radio::Kind, on: bool) -> Res<radio::Radios> {
    blocking(move || radio::set(kind, on)).await
}

#[tauri::command]
pub async fn theme_get() -> Res<bool> {
    blocking(theme::dark).await
}

#[tauri::command]
pub async fn theme_set(dark: bool) -> Res<bool> {
    blocking(move || theme::set_dark(dark)).await
}

#[tauri::command]
pub async fn power_run(action: power::Action) -> Res<()> {
    blocking(move || power::run(action)).await
}

#[tauri::command]
pub async fn wallpaper_get() -> Res<String> {
    blocking(wallpaper::get).await
}

#[tauri::command]
pub async fn wallpaper_set(app: AppHandle, path: String) -> Res<String> {
    let set = blocking(move || wallpaper::set(&PathBuf::from(path))).await?;
    colors::refresh_soon(&app);
    Ok(set)
}

/// Thumbnails for the picker's folder: the saved one, or the Pictures folder.
#[tauri::command]
pub async fn wallpaper_thumbs(app: AppHandle, state: State<'_, AppState>) -> Res<WallpaperFolder> {
    let dir = state
        .settings
        .lock()
        .wallpaper_dir
        .clone()
        .map(PathBuf::from)
        .or_else(|| app.path().picture_dir().ok())
        .ok_or("no folder to show")?;
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let d = dir.clone();
    let thumbs = blocking(move || Ok(thumbs::for_folder(&cache, &d))).await?;
    Ok(WallpaperFolder { dir: dir.to_string_lossy().into_owned(), thumbs })
}

#[derive(serde::Serialize)]
pub struct WallpaperFolder {
    dir: String,
    thumbs: Vec<thumbs::Thumb>,
}

/// Ask for a folder with the system's own picker; `None` when cancelled.
#[tauri::command]
pub async fn wallpaper_pick_folder(app: AppHandle, state: State<'_, AppState>) -> Res<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().set_title("Wallpaper folder").blocking_pick_folder();
    let Some(dir) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let dir = dir.to_string_lossy().into_owned();
    let mut s = state.settings.lock();
    s.wallpaper_dir = Some(dir.clone());
    settings::save(&app, &s)?;
    Ok(Some(dir))
}

#[tauri::command]
pub async fn engine_status() -> Res<wallpaper_engine::Status> {
    blocking(|| Ok(wallpaper_engine::status())).await
}

/// Installed Wallpaper Engine wallpapers with cached thumbnails of their previews.
#[tauri::command]
pub async fn engine_list(app: AppHandle) -> Res<Vec<thumbs::EngineThumb>> {
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    blocking(move || Ok(thumbs::for_engine(&cache))).await
}

/// Show a Wallpaper Engine wallpaper on the main monitor; returns the id Wallpaper
/// Engine confirms.
#[tauri::command]
pub async fn engine_open(app: AppHandle, id: String) -> Res<String> {
    let opened = blocking(move || wallpaper_engine::open(&id, 0)).await?;
    colors::refresh_soon(&app);
    Ok(opened)
}

/// The colours the UI should wear: the user's accent, or the wallpaper's.
#[tauri::command]
pub async fn palette_current(app: AppHandle) -> Res<Palette> {
    task(move || Ok(colors::current(&app))).await
}

/// Set the user's own accent colour, or `None` to follow the wallpaper again.
#[tauri::command]
pub async fn accent_set(app: AppHandle, accent: Option<String>) -> Res<Palette> {
    if let Some(a) = &accent {
        palette::from_accent(a).ok_or_else(|| format!("{a} is not a colour like #4F8FFF"))?;
    }
    task(move || {
        {
            let state = app.state::<AppState>();
            let mut s = state.settings.lock();
            s.accent = accent;
            settings::save(&app, &s)?;
        }
        Ok(colors::publish(&app))
    })
    .await
}

#[tauri::command]
pub async fn stats_sample(state: State<'_, AppState>) -> Res<stats::Snapshot> {
    let monitor = state.monitor.clone();
    blocking(move || Ok(monitor.lock().sample(6))).await
}

/// Installed apps with cached icons; only the first call waits for them to be read.
#[tauri::command]
pub async fn apps_list(app: AppHandle, state: State<'_, AppState>) -> Res<Vec<launcher::Entry>> {
    let cache = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let apps = state.apps.clone();
    task(move || Ok(apps.get(&cache))).await
}

#[tauri::command]
pub async fn app_launch(id: String) -> Res<()> {
    blocking(move || apps::launch(&id)).await
}

#[tauri::command]
pub async fn clipboard_list(state: State<'_, AppState>) -> Res<clip::View> {
    let history = state.clip.clone();
    task(move || Ok(history.view())).await
}

#[tauri::command]
pub async fn clipboard_copy(id: String, state: State<'_, AppState>) -> Res<()> {
    let history = state.clip.clone();
    task(move || history.copy(&id)).await
}

/// Paste an entry into the app that was in front before Nimbus.
#[tauri::command]
pub async fn clipboard_paste(app: AppHandle, id: String, state: State<'_, AppState>) -> Res<()> {
    let history = state.clip.clone();
    task(move || clip::paste(&history, &app, &id)).await
}

#[tauri::command]
pub async fn clipboard_pin(id: String, pinned: bool, state: State<'_, AppState>) -> Res<()> {
    let history = state.clip.clone();
    task(move || Ok(history.pin(&id, pinned))).await
}

#[tauri::command]
pub async fn clipboard_delete(id: String, state: State<'_, AppState>) -> Res<()> {
    let history = state.clip.clone();
    task(move || Ok(history.delete(&id))).await
}

/// Remove everything but pinned entries.
#[tauri::command]
pub async fn clipboard_clear(state: State<'_, AppState>) -> Res<()> {
    let history = state.clip.clone();
    task(move || Ok(history.clear())).await
}

/// Pause or resume the history (saved, so it stays that way after a restart).
#[tauri::command]
pub fn clipboard_enable(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    let mut s = state.settings.lock();
    s.clipboard_history = on;
    settings::save(&app, &s)?;
    state.clip.set_enabled(on);
    Ok(())
}

#[tauri::command]
pub async fn store_get(app: AppHandle, key: String) -> Res<Option<serde_json::Value>> {
    task(move || store::get(&app, &key)).await
}

#[tauri::command]
pub async fn store_set(app: AppHandle, key: String, value: serde_json::Value) -> Res<()> {
    task(move || store::set(&app, &key, &value)).await
}

#[tauri::command]
pub fn autostart_get(app: AppHandle) -> Res<bool> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

/// Start Nimbus when the user signs in, or not; returns what the system now reports.
#[tauri::command]
pub fn autostart_set(app: AppHandle, on: bool) -> Res<bool> {
    use tauri_plugin_autostart::ManagerExt;
    let launcher = app.autolaunch();
    if on { launcher.enable() } else { launcher.disable() }.map_err(|e| e.to_string())?;
    launcher.is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_hit_rect(rect: Rect, hit: State<'_, Arc<HitArea>>) {
    hit.set(rect);
}

/// The UI asks for keyboard focus while a text field is in use or the panel was opened
/// from the keyboard, and gives it back after.
#[tauri::command]
pub fn set_keyboard_focus(app: AppHandle, on: bool) {
    if let Some(w) = island::window(&app) {
        island::set_activating(&w, on);
    }
}

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> settings::Settings {
    state.settings.lock().clone()
}

#[tauri::command]
pub fn privacy_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<privacy::Status> {
    let mut s = state.settings.lock();
    s.hide_from_capture = on;
    settings::save(&app, &s)?;
    let win = island::window(&app).ok_or("no island window")?;
    Ok(privacy::apply(&win, on))
}

#[tauri::command]
pub fn privacy_status(app: AppHandle, state: State<'_, AppState>) -> Res<privacy::Status> {
    let win = island::window(&app).ok_or("no island window")?;
    Ok(privacy::status(&win, state.settings.lock().hide_from_capture))
}

// ---- chat ----

/// Where typed chat goes right now, and which models it offers.
#[tauri::command]
pub async fn chat_status(app: AppHandle, state: State<'_, AppState>) -> Res<chat::Status> {
    let s = state.settings.lock().clone();
    task(move || {
        let found = chat::status(s.chat_url.as_deref(), s.chat_key.as_deref().unwrap_or(""), s.chat_model.as_deref());
        // The local brain unloaded to free the chip still takes the chat: a message wakes it.
        Ok(if found.model.is_none() { crate::brain::asleep_status(&app).unwrap_or(found) } else { found })
    })
    .await
}

/// Save where chat goes. Empty strings clear a value.
#[tauri::command]
pub fn chat_configure(app: AppHandle, url: Option<String>, model: Option<String>, key: Option<String>, state: State<'_, AppState>) -> Res<()> {
    let clean = |v: Option<String>| v.map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let mut s = state.settings.lock();
    s.chat_url = clean(url);
    s.chat_model = clean(model);
    s.chat_key = clean(key);
    settings::save(&app, &s)
}

// ---- agent ----

/// Send the person's message to a session ("chat" for the Chat page). The run goes on
/// in the background and reports through "agent" events.
#[tauri::command]
pub fn agent_send(app: AppHandle, session: String, text: String) -> Res<u64> {
    crate::agent::start(&app, &session, &text, None, false, false, |_, _| {})
}

#[tauri::command]
pub fn agent_stop(session: String, state: State<'_, AppState>) {
    state.agent.stop(&session);
}

/// Forget a session's conversation (New chat).
#[tauri::command]
pub fn agent_reset(session: String, state: State<'_, AppState>) {
    state.agent.reset(&session);
}

/// The button the person pressed on a plugin's notice.
#[tauri::command]
pub fn notice_answer(id: String, button: String) {
    crate::api::answer(&id, &button);
}

/// The person's answer to a question the agent asked.
#[tauri::command]
pub fn agent_answer(ask: u64, allowed: bool, window: tauri::Window, state: State<'_, AppState>) -> Res<()> {
    let taken = state.agent.answer(ask, allowed);
    eprintln!(
        "[nimbus] ask {ask} answered {} in the {} window{}",
        if allowed { "yes" } else { "no" },
        window.label(),
        taken.as_ref().err().map(|e| format!(", refused: {e}")).unwrap_or_default()
    );
    taken
}

// ---- to-dos and routines ----

#[tauri::command]
pub fn todos_list(state: State<'_, AppState>) -> Vec<routines::Todo> {
    state.routines.todos()
}

#[tauri::command]
pub fn todo_add(app: AppHandle, text: String, at: Option<i64>, repeat: Option<routines::Repeat>, kind: Option<routines::Kind>, state: State<'_, AppState>) -> Res<routines::Todo> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("a to-do needs text".into());
    }
    Ok(state.routines.add_todo(
        &app,
        routines::Todo { id: String::new(), text, done: false, at, repeat: repeat.unwrap_or_default(), kind: kind.unwrap_or_default(), task: None, fired: false, by_agent: false },
    ))
}

/// Save a changed to-do (text, done, time, repeat).
#[tauri::command]
pub fn todo_edit(app: AppHandle, todo: routines::Todo, state: State<'_, AppState>) -> Res<routines::Todo> {
    state
        .routines
        .edit_todo(&app, &todo.id.clone(), move |t| {
            t.text = todo.text;
            t.done = todo.done;
            t.at = todo.at;
            t.repeat = todo.repeat;
            t.kind = todo.kind;
        })
        .ok_or_else(|| "that to-do is gone".into())
}

#[tauri::command]
pub fn todo_remove(app: AppHandle, id: String, state: State<'_, AppState>) {
    state.routines.remove_todo(&app, &id);
}

/// A reminder that spoke up: again in `minutes`.
#[tauri::command]
pub fn todo_snooze(app: AppHandle, id: String, minutes: i64, state: State<'_, AppState>) -> Res<routines::Todo> {
    state.routines.snooze(&app, &id, minutes.clamp(1, 24 * 60)).ok_or_else(|| "that to-do is gone".into())
}

#[tauri::command]
pub fn todos_clear_done(app: AppHandle, state: State<'_, AppState>) {
    state.routines.clear_done(&app);
}

/// Open a web page in the browser (help links). Only https.
#[tauri::command]
pub fn open_link(url: String) -> Res<()> {
    if !url.starts_with("https://") {
        return Err("only https links open from here".into());
    }
    crate::open_outside(&url).map_err(|e| e.to_string())
}

/// Listening to the computer's sound: whether it is on, and the last twenty seconds.
#[tauri::command]
pub fn hearing_get(state: State<'_, AppState>) -> crate::hearing::Snapshot {
    crate::hearing::Snapshot::of(&state.hearing)
}

#[tauri::command]
pub fn hearing_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    {
        let mut s = state.settings.lock();
        s.listen_audio = on;
        settings::save(&app, &s)?;
    }
    crate::hearing::set(&app, on);
    Ok(())
}

/// The local brain: off, starting, ready, or why not.
#[tauri::command]
pub fn brain_get(state: State<'_, AppState>) -> crate::brain::Brain {
    state.brain.lock().clone()
}

/// This computer's graphics chip, the llama.cpp build for it, and the models found.
#[tauri::command]
pub async fn brain_plan(app: AppHandle) -> Res<crate::brain::Plan> {
    blocking(move || Ok(crate::brain::plan(&app))).await
}

/// Run this model file (`None`: the best one found), loading it in place of the running one.
#[tauri::command]
pub fn brain_use(app: AppHandle, state: State<'_, AppState>, model: Option<String>) -> Res<()> {
    {
        let mut s = state.settings.lock();
        s.brain_model = model;
        settings::save(&app, &s)?;
    }
    let load = state.settings.lock().brain_idle_minutes == 0;
    std::thread::spawn(move || crate::brain::prepare(&app, false, load));
    Ok(())
}

/// Minutes without a message before the local model is unloaded from the chip; 0 keeps it
/// loaded (and loads it now if it was unloaded).
#[tauri::command]
pub fn brain_idle_set(app: AppHandle, state: State<'_, AppState>, minutes: u32) -> Res<()> {
    {
        let mut s = state.settings.lock();
        s.brain_idle_minutes = minutes;
        settings::save(&app, &s)?;
    }
    if minutes == 0 {
        std::thread::spawn(move || crate::brain::wake(&app));
    }
    Ok(())
}

/// The language spoken to Nimbus ("en", "de"); None lets speech to text guess.
#[tauri::command]
pub fn speech_language_set(app: AppHandle, state: State<'_, AppState>, language: Option<String>) -> Res<()> {
    let mut s = state.settings.lock();
    s.speech_language = language.filter(|l| !l.trim().is_empty());
    settings::save(&app, &s)?;
    Ok(())
}

/// Where web searches go: a SearXNG address, a Brave Search key, or neither (DuckDuckGo).
#[tauri::command]
pub fn search_set(app: AppHandle, state: State<'_, AppState>, searxng: Option<String>, brave_key: Option<String>) -> Res<()> {
    let searxng = searxng.map(|s| s.trim().trim_end_matches('/').to_string()).filter(|s| !s.is_empty());
    if let Some(url) = &searxng {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err("type the address with http:// or https://".into());
        }
    }
    let mut s = state.settings.lock();
    s.search_searxng = searxng;
    s.search_brave_key = brave_key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    settings::save(&app, &s)?;
    Ok(())
}

/// Download what the brain is missing (the server build, a model) and start it.
#[tauri::command]
pub fn brain_setup(app: AppHandle, state: State<'_, AppState>) {
    let load = state.settings.lock().brain_idle_minutes == 0;
    std::thread::spawn(move || crate::brain::prepare(&app, true, load));
}

#[tauri::command]
pub fn brain_stop_setup() {
    crate::brain::stop_setup();
}

/// Open the models folder, where a GGUF file from Hugging Face can be put.
#[tauri::command]
pub fn brain_open_folder(app: AppHandle) -> Res<()> {
    let dir = crate::models::folder(&app).ok_or("Nimbus's data folder is not available")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    crate::open_outside(&dir).map_err(|e| e.to_string())
}

/// "Hey Nimbus": whether the microphone is listened to, and whether a request is awaited.
#[tauri::command]
pub fn voice_get(state: State<'_, AppState>) -> crate::voice::Snapshot {
    crate::voice::Snapshot::of(&state.voice)
}

#[tauri::command]
pub fn voice_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    {
        let mut s = state.settings.lock();
        s.voice = on;
        settings::save(&app, &s)?;
    }
    crate::voice::set(&app, on);
    Ok(())
}

/// The chat's microphone button: listen for one request.
#[tauri::command]
pub fn talk_start(app: AppHandle) -> Res<()> {
    crate::voice::talk_start(&app).map_err(Into::into)
}

/// The microphone button again: `keep` writes down what was said so far.
#[tauri::command]
pub fn talk_stop(app: AppHandle, keep: bool) {
    crate::voice::talk_stop(&app, keep);
}

/// The page is reading an answer aloud (or has stopped), so the microphone does not hear it.
#[tauri::command]
pub fn voice_speaking(app: AppHandle, on: bool) {
    crate::voice::speaking(&app, on);
}

#[tauri::command]
pub fn speak_replies_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    let mut s = state.settings.lock();
    s.speak_replies = on;
    settings::save(&app, &s)?;
    Ok(())
}

/// Seeing the screen: whether it is on, and how many pictures are held right now.
#[derive(serde::Serialize)]
pub struct ScreenState {
    on: bool,
    pictures: usize,
}

#[tauri::command]
pub fn screen_get(state: State<'_, AppState>) -> ScreenState {
    ScreenState { on: state.screen.watching.load(std::sync::atomic::Ordering::Relaxed), pictures: state.screen.count() }
}

#[tauri::command]
pub fn screen_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    {
        let mut s = state.settings.lock();
        s.see_screen = on;
        settings::save(&app, &s)?;
    }
    state.screen.watching.store(on, std::sync::atomic::Ordering::Relaxed);
    if !on {
        state.screen.clear();
    }
    Ok(())
}

/// Let the agent click and type in apps.
#[tauri::command]
pub fn control_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    let mut s = state.settings.lock();
    s.control = on;
    settings::save(&app, &s)
}

// ---- file search ----

#[tauri::command]
pub async fn files_search(query: String, limit: Option<usize>, state: State<'_, AppState>) -> Res<Vec<crate::files::Hit>> {
    let files = state.files.clone();
    task(move || Ok(files.search(&query, limit.unwrap_or(40)))).await
}

#[tauri::command]
pub fn files_status(state: State<'_, AppState>) -> crate::files::Status {
    state.files.status()
}

#[tauri::command]
pub fn files_set(app: AppHandle, on: bool, state: State<'_, AppState>) -> Res<()> {
    {
        let mut s = state.settings.lock();
        s.file_search = on;
        settings::save(&app, &s)?;
    }
    crate::files::set(&app, on);
    Ok(())
}

/// Open a file or folder with its usual app.
#[tauri::command]
pub async fn file_open(path: String) -> Res<()> {
    task(move || {
        let p = PathBuf::from(&path);
        if !p.exists() {
            return Err("That file is no longer there.".into());
        }
        crate::open_outside(&p).map_err(|e| format!("Could not open it: {e}"))
    })
    .await
}

/// Show a file in its folder, selected where the system allows it.
#[tauri::command]
pub async fn file_reveal(path: String) -> Res<()> {
    task(move || {
        let p = PathBuf::from(&path);
        if !p.exists() {
            return Err("That file is no longer there.".into());
        }
        crate::files::reveal(&p)
    })
    .await
}

/// Show the agent's on-screen overlay for a few seconds, with nothing being done.
#[tauri::command]
pub fn overlay_preview(app: AppHandle) {
    crate::overlay::preview(&app);
}

// ---- mail (read-only) ----

#[tauri::command]
pub fn mail_accounts(state: State<'_, AppState>) -> Vec<crate::mail::Account> {
    state.mail.accounts()
}

/// Where an address's mail lives and what to sign in with.
#[tauri::command]
pub fn mail_provider(email: String) -> crate::mail::Provider {
    crate::mail::provider(&email)
}

/// Sign in (checked against the server before anything is kept). Returns the unread count.
#[tauri::command]
pub async fn mail_add(app: AppHandle, email: String, password: String, user: Option<String>, host: Option<String>, port: Option<u16>) -> Res<usize> {
    task(move || {
        let state = app.state::<AppState>();
        state.mail.add(&app, &email, &password, user, host, port).map(|(_, n)| n)
    })
    .await
}

/// Which browser sign-ins are set up (an app ID is known for them).
#[derive(serde::Serialize)]
pub struct BrowserSignIn {
    google: bool,
    microsoft: bool,
}

#[tauri::command]
pub fn mail_browser_ready(state: State<'_, AppState>) -> BrowserSignIn {
    let s = state.settings.lock();
    BrowserSignIn { google: crate::oauth::issuer(crate::oauth::Kind::Google, &s).is_some(), microsoft: crate::oauth::issuer(crate::oauth::Kind::Microsoft, &s).is_some() }
}

/// Keep the person's own sign-in app for Google or Microsoft.
#[tauri::command]
pub fn mail_browser_setup(app: AppHandle, kind: crate::oauth::Kind, client_id: String, client_secret: Option<String>, state: State<'_, AppState>) -> Res<()> {
    let id = client_id.trim().to_string();
    let secret = client_secret.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    match kind {
        crate::oauth::Kind::Google if !id.ends_with(".apps.googleusercontent.com") => {
            return Err("that is not a Google client ID; it ends in .apps.googleusercontent.com".into());
        }
        crate::oauth::Kind::Google if secret.is_none() => return Err("Google also needs the client secret shown next to the ID".into()),
        crate::oauth::Kind::Microsoft if !(id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')) => {
            return Err("that is not an Application (client) ID; it looks like 1a2b3c4d-1a2b-1a2b-1a2b-1a2b3c4d5e6f".into());
        }
        _ => {}
    }
    let mut s = state.settings.lock();
    match kind {
        crate::oauth::Kind::Google => {
            s.google_client_id = Some(id);
            s.google_client_secret = secret;
        }
        crate::oauth::Kind::Microsoft => s.microsoft_client_id = Some(id),
    }
    settings::save(&app, &s)
}

#[derive(serde::Serialize)]
pub struct MailAdded {
    email: String,
    unread: usize,
}

/// Sign in through Google or Microsoft in the browser; waits until the person is done.
#[tauri::command]
pub async fn mail_browser_sign_in(app: AppHandle, kind: crate::oauth::Kind, hint: Option<String>) -> Res<MailAdded> {
    task(move || {
        let state = app.state::<AppState>();
        state.mail.add_browser(&app, kind, hint).map(|(a, unread)| MailAdded { email: a.email, unread })
    })
    .await
}

#[tauri::command]
pub fn mail_browser_cancel() {
    crate::oauth::cancel();
}

#[tauri::command]
pub fn mail_remove(app: AppHandle, id: String, state: State<'_, AppState>) {
    state.mail.remove(&app, &id);
}

#[tauri::command]
pub fn routines_list(state: State<'_, AppState>) -> Vec<routines::Routine> {
    state.routines.routines()
}

#[tauri::command]
pub fn routine_save(app: AppHandle, routine: routines::Routine, state: State<'_, AppState>) -> Res<routines::Routine> {
    if routine.name.trim().is_empty() || routine.steps.trim().is_empty() {
        return Err("a routine needs a name and steps".into());
    }
    Ok(state.routines.put_routine(&app, routine))
}

#[tauri::command]
pub fn routine_remove(app: AppHandle, id: String, state: State<'_, AppState>) {
    state.agent.stop(&format!("routine:{id}"));
    state.routines.remove_routine(&app, &id);
}

/// Run a routine now, by hand.
#[tauri::command]
pub fn routine_run(app: AppHandle, id: String, state: State<'_, AppState>) -> Res<()> {
    let r = state.routines.routine(&id).ok_or("that routine is gone")?;
    if state.agent.busy(&format!("routine:{id}")) {
        return Err("it is running already".into());
    }
    crate::agent::run_routine(&app, &r);
    Ok(())
}

/// Put plain text on the clipboard (a chat reply). The history picks it up like any copy.
#[tauri::command]
pub async fn clipboard_write(text: String) -> Res<()> {
    task(move || arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)).map_err(|e| e.to_string())).await
}

// ---- speech ----

#[tauri::command]
pub fn speech_status(state: State<'_, AppState>) -> crate::speech::Speech {
    state.speech.lock().clone()
}

/// Try again (Lemonade was closed, a model failed to load), optionally with another
/// Whisper model, which is then remembered.
#[tauri::command]
pub fn speech_retry(app: AppHandle, state: State<'_, AppState>, model: Option<String>) -> Result<(), String> {
    let chosen = {
        let mut s = state.settings.lock();
        if model.is_some() {
            s.speech_model = model;
            crate::settings::save(&app, &s)?;
        }
        s.speech_model.clone()
    };
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        crate::speech::prepare(&app, &state.speech, chosen);
    });
    Ok(())
}
