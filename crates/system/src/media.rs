//! "Now playing" and transport controls for whatever media app is active
//! (Spotify, a browser tab, the system player).

use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct NowPlaying {
    /// The app that owns the session, e.g. "Spotify.exe" or "spotify".
    pub app: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// Cover art as a data: URL, when the system provides it.
    pub artwork: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
}

/// The active media session, or `None` when nothing is playing or paused.
pub fn now_playing(with_artwork: bool) -> Result<Option<NowPlaying>> {
    imp::now_playing(with_artwork)
}

/// Run a transport action and return the session state afterwards.
pub fn control(action: Action) -> Result<Option<NowPlaying>> {
    imp::control(action)?;
    // Players apply the change asynchronously; give them a moment before reading back.
    std::thread::sleep(std::time::Duration::from_millis(250));
    now_playing(false)
}

#[cfg(windows)]
mod imp {
    use super::{Action, NowPlaying};
    use crate::Result;
    use base64::Engine;
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    };
    use windows::Storage::Streams::{DataReader, InputStreamOptions};

    fn session() -> Result<Option<Session>> {
        let manager = Manager::RequestAsync()?.join()?;
        Ok(manager.GetCurrentSession().ok())
    }

    /// WinRT TimeSpan ticks are 100 ns.
    fn ms(ticks: i64) -> Option<u64> {
        (ticks > 0).then(|| (ticks / 10_000) as u64)
    }

    pub fn now_playing(with_artwork: bool) -> Result<Option<NowPlaying>> {
        let Some(s) = session()? else { return Ok(None) };
        let props = s.TryGetMediaPropertiesAsync()?.join()?;
        let status = s.GetPlaybackInfo()?.PlaybackStatus()?;
        let timeline = s.GetTimelineProperties()?;
        let artwork = if with_artwork { read_artwork(&props).ok().flatten() } else { None };
        Ok(Some(NowPlaying {
            app: s.SourceAppUserModelId()?.to_string(),
            title: props.Title()?.to_string(),
            artist: props.Artist()?.to_string(),
            album: props.AlbumTitle()?.to_string(),
            playing: status == Status::Playing,
            position_ms: ms(timeline.Position()?.Duration),
            duration_ms: ms(timeline.EndTime()?.Duration),
            artwork,
        }))
    }

    fn read_artwork(
        props: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties,
    ) -> Result<Option<String>> {
        let Ok(thumb) = props.Thumbnail() else { return Ok(None) };
        let stream = thumb.OpenReadAsync()?.join()?;
        let size = stream.Size()? as u32;
        if size == 0 {
            return Ok(None);
        }
        let mime = stream.ContentType().map(|m| m.to_string()).unwrap_or_default();
        let reader = DataReader::CreateDataReader(&stream)?;
        reader.SetInputStreamOptions(InputStreamOptions::ReadAhead)?;
        reader.LoadAsync(size)?.join()?;
        let mut bytes = vec![0u8; size as usize];
        reader.ReadBytes(&mut bytes)?;
        let mime = if mime.is_empty() { "image/png".to_string() } else { mime };
        Ok(Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))))
    }

    pub fn control(action: Action) -> Result<()> {
        if let Some(s) = session()? {
            let done = match action {
                Action::PlayPause => s.TryTogglePlayPauseAsync()?.join()?,
                Action::Play => s.TryPlayAsync()?.join()?,
                Action::Pause => s.TryPauseAsync()?.join()?,
                Action::Next => s.TrySkipNextAsync()?.join()?,
                Action::Previous => s.TrySkipPreviousAsync()?.join()?,
            };
            if done {
                return Ok(());
            }
        }
        // No session, or the app refused: fall back to the media keys.
        media_key(action);
        Ok(())
    }

    fn media_key(action: Action) {
        use windows::Win32::UI::Input::KeyboardAndMouse::*;
        let vk = match action {
            Action::PlayPause | Action::Play | Action::Pause => VK_MEDIA_PLAY_PAUSE,
            Action::Next => VK_MEDIA_NEXT_TRACK,
            Action::Previous => VK_MEDIA_PREV_TRACK,
        };
        let key = |flags| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: flags, ..Default::default() } },
        };
        unsafe {
            SendInput(&[key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)], std::mem::size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Action, NowPlaying};
    use crate::Result;
    use crate::cmd::osascript;

    /// Spotify first, then Apple Music; both are scriptable without extra permissions
    /// beyond the one-time Automation prompt.
    const APPS: [&str; 2] = ["Spotify", "Music"];

    fn running(app: &str) -> bool {
        osascript(&format!("application \"{app}\" is running")).map(|s| s == "true").unwrap_or(false)
    }

    pub fn now_playing(_with_artwork: bool) -> Result<Option<NowPlaying>> {
        for app in APPS {
            if !running(app) {
                continue;
            }
            let script = format!(
                "tell application \"{app}\"\n\
                 if player state is stopped then return \"\"\n\
                 set t to current track\n\
                 return (player state as text) & \"\\t\" & (name of t) & \"\\t\" & (artist of t) & \"\\t\" & (album of t) & \"\\t\" & ((player position * 1000) as integer) & \"\\t\" & (duration of t)\n\
                 end tell"
            );
            let out = osascript(&script)?;
            let f: Vec<&str> = out.split('\t').collect();
            if f.len() < 6 {
                continue;
            }
            // Spotify reports duration in ms, Music in seconds.
            let duration = f[5].replace(',', ".").parse::<f64>().ok().map(|d| {
                if app == "Music" { (d * 1000.0) as u64 } else { d as u64 }
            });
            return Ok(Some(NowPlaying {
                app: app.to_string(),
                title: f[1].into(),
                artist: f[2].into(),
                album: f[3].into(),
                playing: f[0] == "playing",
                position_ms: f[4].parse().ok(),
                duration_ms: duration,
                artwork: None,
            }));
        }
        Ok(None)
    }

    pub fn control(action: Action) -> Result<()> {
        let verb = match action {
            Action::PlayPause => "playpause",
            Action::Play => "play",
            Action::Pause => "pause",
            Action::Next => "next track",
            Action::Previous => "previous track",
        };
        for app in APPS {
            if running(app) {
                osascript(&format!("tell application \"{app}\" to {verb}"))?;
                return Ok(());
            }
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{Action, NowPlaying};
    use crate::Result;
    use crate::cmd::run;

    /// MPRIS through `playerctl`, which every desktop player speaks.
    pub fn now_playing(_with_artwork: bool) -> Result<Option<NowPlaying>> {
        let fmt = "{{status}}\t{{title}}\t{{artist}}\t{{album}}\t{{position}}\t{{mpris:length}}\t{{playerName}}\t{{mpris:artUrl}}";
        let Ok(out) = run("playerctl", &["metadata", "--format", fmt]) else { return Ok(None) };
        let f: Vec<&str> = out.split('\t').collect();
        if f.len() < 8 || f[0] == "Stopped" {
            return Ok(None);
        }
        // playerctl reports microseconds.
        let us = |s: &str| s.parse::<u64>().ok().map(|v| v / 1000);
        let art = f[7];
        Ok(Some(NowPlaying {
            app: f[6].into(),
            title: f[1].into(),
            artist: f[2].into(),
            album: f[3].into(),
            playing: f[0] == "Playing",
            position_ms: us(f[4]),
            duration_ms: us(f[5]),
            artwork: art.starts_with("http").then(|| art.to_string()),
        }))
    }

    pub fn control(action: Action) -> Result<()> {
        let verb = match action {
            Action::PlayPause => "play-pause",
            Action::Play => "play",
            Action::Pause => "pause",
            Action::Next => "next",
            Action::Previous => "previous",
        };
        run("playerctl", &[verb]).map(|_| ())
    }
}
