//! User settings, saved as JSON in the app's config folder.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Hide the island from screenshots, recordings and screen shares.
    pub hide_from_capture: bool,
    /// Folder the wallpaper picker shows.
    pub wallpaper_dir: Option<String>,
    /// Keep a history of copied text, links and images, on this PC only.
    pub clipboard_history: bool,
    /// The user's own accent colour (#RRGGBB) instead of the wallpaper's.
    pub accent: Option<String>,
    /// Where typed chat goes (an OpenAI-style address); `None` finds a local server.
    pub chat_url: Option<String>,
    /// The model to chat with; `None` takes the server's first chat model.
    pub chat_model: Option<String>,
    /// Key for a cloud server. Kept in this file, on this computer only.
    pub chat_key: Option<String>,
    /// The Whisper model Lemonade loads for speech; `None` loads the fastest one on this computer:
    /// Whisper-Small, else Turbo.
    pub speech_model: Option<String>,
    /// The language the person speaks to Nimbus in ("en", "de", …), handed to Whisper.
    /// Left to guess, Whisper takes short English requests for Urdu or Arabic. None
    /// lets it guess.
    pub speech_language: Option<String>,
    /// The model's context window in tokens; older turns are compacted to stay inside it.
    pub context_window: u32,
    /// Web search through the person's own SearXNG, or Brave Search with their key.
    /// Neither set: DuckDuckGo, which needs no key.
    pub search_searxng: Option<String>,
    pub search_brave_key: Option<String>,
    /// Keep a transcript of the computer's own sound for the last twenty seconds.
    pub listen_audio: bool,
    /// Keep the last ten seconds of the main screen in memory, for the agent to look at.
    pub see_screen: bool,
    /// Let the agent click and type in apps, by name (see `agent::hands`). Off until the
    /// person turns it on.
    pub control: bool,
    /// The person's own sign-in app for "Continue with Google" (a Google Cloud OAuth
    /// client of type Desktop app). Google calls part of it a secret, but a desktop app's
    /// copy is not one; it is kept here like the rest.
    pub google_client_id: Option<String>,
    pub google_client_secret: Option<String>,
    /// The person's own sign-in app for "Continue with Microsoft" (a Microsoft Entra app
    /// registration's Application ID).
    pub microsoft_client_id: Option<String>,
    /// Find files by name from the Apps search, with an index kept on this computer.
    pub file_search: bool,
    /// Listen to the microphone for "Hey Nimbus".
    pub voice: bool,
    /// Read answers to spoken requests aloud.
    pub speak_replies: bool,
    /// Start a model on this computer's GPU for the chat when nothing else offers one.
    pub local_brain: bool,
    /// The llama.cpp server program and model file for it; `None` looks for them.
    pub brain_server: Option<String>,
    pub brain_model: Option<String>,
    /// The llama.cpp build to use instead of the one chosen for the graphics chip.
    pub brain_backend: Option<crate::runtime::Backend>,
    /// Download the model server and a model on the first start when none is found.
    pub brain_auto_setup: bool,
    /// Minutes without a message before the model is unloaded from the chip; the next
    /// message loads it again. 0 keeps it loaded.
    pub brain_idle_minutes: u32,
    /// Apps Nimbus never looks inside, clicks or types in (names or title words).
    pub hands_off: Vec<String>,
    /// Programs that add tools (Model Context Protocol servers over stdio).
    pub plugins: Vec<crate::plugins::PluginConfig>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hide_from_capture: true,
            wallpaper_dir: None,
            // Only Windows marks which copies are passwords, so elsewhere the history
            // starts switched off rather than keep them.
            clipboard_history: cfg!(windows),
            accent: None,
            chat_url: None,
            chat_model: None,
            chat_key: None,
            speech_model: None,
            speech_language: Some("en".into()),
            context_window: 125_000,
            search_searxng: None,
            search_brave_key: None,
            listen_audio: false,
            see_screen: false,
            control: false,
            google_client_id: None,
            google_client_secret: None,
            microsoft_client_id: None,
            file_search: true,
            voice: false,
            speak_replies: false,
            local_brain: true,
            brain_server: None,
            brain_model: None,
            brain_backend: None,
            brain_auto_setup: true,
            brain_idle_minutes: 30,
            hands_off: Vec::new(),
            plugins: Vec::new(),
        }
    }
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Settings {
    path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, s: &Settings) -> Result<(), String> {
    let p = path(app).ok_or("no config folder")?;
    let json = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    crate::store::write_atomic(&p, json.as_bytes())
}
