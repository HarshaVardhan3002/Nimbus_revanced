mod agent;
mod api;
mod chat;
mod plugins;
mod clip;
mod colors;
mod commands;
mod files;
mod hearing;
mod voice;
mod brain;
mod fetch;
mod models;
mod runtime;
mod oauth;
mod island;
mod launcher;
mod mail;
mod overlay;
mod privacy;
mod routines;
mod settings;
mod speech;
mod store;
mod thumbs;

use nimbus_system::stats::Monitor;
use parking_lot::Mutex;
use serde::Serialize;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// The assistant's name, as the person sees it (the frontend's is in brand.ts).
pub const BRAND_NAME: &str = "Nimbus";

/// Opens a link, file or folder with the program the computer uses for it. On Windows it
/// goes through the shell (Explorer), so the browser or app it starts does not close when
/// Nimbus is closed or restarted.
pub(crate) fn open_outside(target: impl AsRef<std::ffi::OsStr>) -> std::io::Result<()> {
    #[cfg(windows)]
    if let Some(t) = target.as_ref().to_str() {
        if nimbus_system::apps::open_via_shell(t) {
            return Ok(());
        }
    }
    open::that_detached(target)
}

pub struct AppState {
    pub monitor: Arc<Mutex<Monitor>>,
    pub settings: Mutex<settings::Settings>,
    pub colors: colors::Colors,
    pub clip: Arc<clip::History>,
    pub apps: Arc<launcher::Cache>,
    pub agent: agent::Agent,
    pub routines: routines::Book,
    pub mail: mail::Mail,
    pub overlay: overlay::Overlay,
    pub hearing: hearing::Hearing,
    pub voice: voice::Voice,
    pub brain: brain::Status,
    pub screen: Arc<agent::screen::Screen>,
    pub speech: speech::Status,
    pub files: Arc<files::Files>,
}

/// Open or close the panel. Opened from the keyboard it takes keyboard focus; opened
/// with the mouse or the tray it never does.
#[derive(Clone, Serialize)]
struct Toggle {
    keyboard: bool,
    /// The page to open on, if not the last one.
    tab: Option<&'static str>,
}

/// Ctrl+Alt+Space opens or closes the panel from anywhere. Space never types a
/// character under AltGr, so this does not clash with European keyboard layouts.
fn toggle_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space)
}

/// Ctrl+Alt+Enter opens straight into chat, ready to type.
fn chat_shortcut() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Enter)
}

/// Whether the first start may download the model server and a model.
fn state_setup(app: &AppHandle) -> bool {
    app.state::<AppState>().settings.lock().brain_auto_setup
}

fn show_island(app: &AppHandle) {
    let Some(win) = island::window(app) else { return };
    island::place(&win);
    let _ = win.show();
    island::make_non_activating(&win);
    // Re-applied on every show and read back: see privacy.rs.
    let on = app.state::<AppState>().settings.lock().hide_from_capture;
    let status = privacy::apply(&win, on);
    if on && !status.verified {
        eprintln!("[nimbus] hide-from-capture not confirmed by the OS: {}", status.mode);
    }
}

/// The app was called Jarvis (identifier dev.harsha.jarvis) before it was Nimbus. On the
/// first start under the new name, its folders move over: settings, routines, mail
/// accounts, clipboard history, caches and the web view's own storage. Runs before
/// Tauri opens any of them. Where a new folder already exists, nothing is touched.
fn move_old_folders() {
    const OLD: &str = "dev.harsha.jarvis";
    const NEW: &str = "dev.harsha.nimbus";
    let env = |k: &str| std::env::var_os(k).map(std::path::PathBuf::from);
    let home = env("HOME").or_else(|| env("USERPROFILE"));
    let mut parents: Vec<std::path::PathBuf> = Vec::new();
    if cfg!(windows) {
        parents.extend(env("APPDATA"));
        parents.extend(env("LOCALAPPDATA"));
    } else if cfg!(target_os = "macos") {
        if let Some(h) = &home {
            for sub in ["Library/Application Support", "Library/Caches", "Library/WebKit"] {
                parents.push(h.join(sub));
            }
        }
    } else if let Some(h) = &home {
        parents.push(env("XDG_DATA_HOME").unwrap_or_else(|| h.join(".local/share")));
        parents.push(env("XDG_CONFIG_HOME").unwrap_or_else(|| h.join(".config")));
        parents.push(env("XDG_CACHE_HOME").unwrap_or_else(|| h.join(".cache")));
    }
    for parent in parents {
        let (old, new) = (parent.join(OLD), parent.join(NEW));
        if !old.is_dir() || new.exists() {
            continue;
        }
        // The old web view can hold its files for a moment after the old app quits.
        let mut tries = 0;
        while let Err(e) = std::fs::rename(&old, &new) {
            tries += 1;
            if tries >= 20 {
                eprintln!("[nimbus] {} could not move to {}: {e}", old.display(), new.display());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    move_old_folders();
    let hit = Arc::new(island::HitArea::default());

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = app.emit("toggle-panel", Toggle { keyboard: false, tab: None });
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    if *shortcut == toggle_shortcut() {
                        let _ = app.emit("toggle-panel", Toggle { keyboard: true, tab: None });
                    } else if *shortcut == chat_shortcut() {
                        let _ = app.emit("toggle-panel", Toggle { keyboard: true, tab: Some("chat") });
                    } else if *shortcut == overlay::escape() {
                        // Off the event loop: unregistering Escape from inside its own handler waits on it.
                        let app = app.clone();
                        std::thread::spawn(move || overlay::cancel(&app));
                    }
                })
                .build(),
        )
        .manage(hit.clone())
        .setup(move |app| {
            let handle = app.handle().clone();
            let settings = settings::load(&handle);
            let clip_dir = app.path().app_data_dir()?.join("clipboard");
            let clip = Arc::new(clip::History::open(clip_dir, settings.clipboard_history));
            app.manage(AppState {
                monitor: Arc::new(Mutex::new(Monitor::new())),
                settings: Mutex::new(settings),
                colors: colors::Colors::default(),
                clip: clip.clone(),
                apps: Arc::default(),
                agent: agent::Agent::default(),
                routines: routines::Book::open(&handle),
                mail: mail::Mail::open(&handle),
                overlay: overlay::Overlay::default(),
                hearing: hearing::Hearing::default(),
                voice: voice::Voice::default(),
                brain: brain::new(),
                screen: Arc::default(),
                speech: speech::new(),
                files: Arc::default(),
            });
            routines::spawn(handle.clone());
            hearing::spawn(handle.clone());
            voice::spawn(handle.clone());
            // The local API first: the plugins are handed its address.
            api::spawn(handle.clone());
            {
                let configs = handle.state::<AppState>().settings.lock().plugins.clone();
                let builtin = agent::tool_names();
                plugins::spawn(configs, builtin);
            }
            // The local brain: loaded onto the GPU before the first question, or, when it is
            // loaded only while used, made ready and loaded by the first message.
            {
                let app = handle.clone();
                let setup = state_setup(&handle);
                let load = handle.state::<AppState>().settings.lock().brain_idle_minutes == 0;
                std::thread::Builder::new().name("brain".into()).spawn(move || brain::prepare(&app, setup, load)).ok();
                brain::watch(handle.clone());
                brain::spawn_idle(handle.clone());
            }
            {
                let state = handle.state::<AppState>();
                let see = state.settings.lock().see_screen;
                state.screen.watching.store(see, std::sync::atomic::Ordering::Relaxed);
                agent::screen::spawn(state.screen.clone());
            }
            if handle.state::<AppState>().settings.lock().file_search {
                files::set(&handle, true);
            }
            if handle.state::<AppState>().settings.lock().listen_audio {
                hearing::set(&handle, true);
            }
            if handle.state::<AppState>().settings.lock().voice {
                voice::set(&handle, true);
            }
            // Lemonade and Whisper, ready before the first word is spoken.
            {
                let app = handle.clone();
                std::thread::spawn(move || {
                    let state = app.state::<AppState>();
                    let chosen = state.settings.lock().speech_model.clone();
                    speech::prepare(&app, &state.speech, chosen);
                });
            }

            if let Err(e) = app.global_shortcut().register(toggle_shortcut()) {
                eprintln!("[nimbus] Ctrl+Alt+Space is taken by another app: {e}");
            }
            if let Err(e) = app.global_shortcut().register(chat_shortcut()) {
                eprintln!("[nimbus] Ctrl+Alt+Enter is taken by another app: {e}");
            }

            let open = MenuItem::with_id(app, "open", "Open panel", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Nimbus", true, None::<&str>)?;
            TrayIconBuilder::with_id("tray")
                .icon(app.default_window_icon().cloned().expect("bundled icon"))
                .tooltip("Nimbus")
                .menu(&Menu::with_items(app, &[&open, &quit])?)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => {
                        let _ = app.emit("toggle-panel", Toggle { keyboard: false, tab: None });
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            show_island(&handle);
            if let Err(e) = overlay::create(&handle) {
                eprintln!("[nimbus] the on-screen overlay could not be made: {e}");
            }
            island::spawn_click_through(handle.clone(), hit.clone());
            clip::spawn_watcher(handle.clone(), clip);
            colors::spawn_watcher(handle.clone());
            // Read the apps and draw their icons now, so the Apps tab opens instantly.
            std::thread::spawn(move || {
                if let Ok(cache) = handle.path().app_cache_dir() {
                    handle.state::<AppState>().apps.get(&cache);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::volume_get,
            commands::volume_set,
            commands::volume_mute,
            commands::brightness_get,
            commands::brightness_set,
            commands::media_now,
            commands::media_control,
            commands::audio_peak,
            commands::radios_get,
            commands::radio_set,
            commands::theme_get,
            commands::theme_set,
            commands::power_run,
            commands::wallpaper_get,
            commands::wallpaper_set,
            commands::wallpaper_thumbs,
            commands::wallpaper_pick_folder,
            commands::engine_status,
            commands::engine_list,
            commands::engine_open,
            commands::palette_current,
            commands::accent_set,
            commands::stats_sample,
            commands::apps_list,
            commands::app_launch,
            commands::clipboard_list,
            commands::clipboard_copy,
            commands::clipboard_paste,
            commands::clipboard_pin,
            commands::clipboard_delete,
            commands::clipboard_clear,
            commands::clipboard_enable,
            commands::store_get,
            commands::store_set,
            commands::autostart_get,
            commands::autostart_set,
            commands::set_hit_rect,
            commands::set_keyboard_focus,
            commands::settings_get,
            commands::privacy_set,
            commands::privacy_status,
            commands::chat_status,
            commands::chat_configure,
            commands::agent_send,
            commands::agent_stop,
            commands::agent_reset,
            commands::agent_answer,
            commands::notice_answer,
            commands::todos_list,
            commands::todo_add,
            commands::todo_edit,
            commands::todo_remove,
            commands::todo_snooze,
            commands::todos_clear_done,
            commands::open_link,
            commands::overlay_preview,
            commands::hearing_get,
            commands::hearing_set,
            commands::voice_get,
            commands::brain_get,
            commands::brain_plan,
            commands::brain_use,
            commands::brain_idle_set,
            commands::speech_language_set,
            commands::search_set,
            commands::brain_setup,
            commands::brain_stop_setup,
            commands::brain_open_folder,
            commands::voice_set,
            commands::voice_speaking,
            commands::talk_start,
            commands::talk_stop,
            commands::speak_replies_set,
            commands::screen_get,
            commands::screen_set,
            commands::control_set,
            commands::mail_accounts,
            commands::mail_provider,
            commands::mail_add,
            commands::mail_remove,
            commands::mail_browser_ready,
            commands::mail_browser_setup,
            commands::mail_browser_sign_in,
            commands::mail_browser_cancel,
            commands::routines_list,
            commands::routine_save,
            commands::routine_remove,
            commands::routine_run,
            commands::clipboard_write,
            commands::speech_status,
            commands::speech_retry,
            commands::files_search,
            commands::files_status,
            commands::files_set,
            commands::file_open,
            commands::file_reveal,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Nimbus")
        .run(|app, event| {
            // The plugins end with Nimbus, and so does a model loaded only while used.
            if let tauri::RunEvent::Exit = event {
                plugins::stop();
                brain::on_exit(app);
            }
        });
}
