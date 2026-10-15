//! Playing what the person asks for: a video on YouTube in their browser, a song, album,
//! playlist or artist in the Spotify app. Nothing needs signing in.
//!
//! YouTube's own results page names its videos in the page data, best match first. For
//! Spotify a web search finds the open.spotify.com link, and the app is handed
//! `spotify:track:<id>?autoplay=true`: without `autoplay` a running Spotify only shows the
//! song; with it, Spotify plays it whether it was open or not. The media session then
//! tells whether it really plays.

use super::web;
use crate::oauth::enc;
use nimbus_system::media;
use std::time::{Duration, Instant};

/// The id of a YouTube video link ("watch?v=", "youtu.be/", "shorts/").
fn youtube_id(link: &str) -> Option<String> {
    let l = link.trim();
    let host_ok = ["youtube.com/", "youtu.be/", "youtube-nocookie.com/"].iter().any(|h| l.contains(h));
    if !host_ok {
        return None;
    }
    let after = if let Some(i) = l.find("v=") {
        &l[i + 2..]
    } else if let Some(i) = l.find("youtu.be/") {
        &l[i + 9..]
    } else if let Some(i) = l.find("/shorts/") {
        &l[i + 8..]
    } else if let Some(i) = l.find("/embed/") {
        &l[i + 7..]
    } else {
        return None;
    };
    let id: String = after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

/// The first real video in a YouTube results page (ads and shelves come as other kinds),
/// with its title.
fn first_video(page: &str) -> Option<(String, String)> {
    let at = page.find("\"videoRenderer\":{\"videoId\":\"")? + "\"videoRenderer\":{\"videoId\":\"".len();
    let id: String = page[at..].chars().take_while(|c| *c != '"').collect();
    if id.len() != 11 {
        return None;
    }
    let rest = &page[at..];
    let title = rest
        .find("\"title\":{\"runs\":[{\"text\":\"")
        .map(|i| &rest[i + "\"title\":{\"runs\":[{\"text\":\"".len()..])
        .and_then(|t| {
            // The title is a JSON string: read it up to its closing quote, escapes and all.
            let mut end = 0;
            let b = t.as_bytes();
            while end < b.len() && !(b[end] == b'"' && (end == 0 || b[end - 1] != b'\\')) {
                end += 1;
            }
            serde_json::from_str::<String>(&format!("\"{}\"", &t[..end])).ok()
        })
        .unwrap_or_default();
    Some((id, title))
}

/// Find a video and open it in the default browser, where it starts playing.
pub fn youtube(query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("say which video".into());
    }
    if let Some(id) = youtube_id(q) {
        let url = format!("https://www.youtube.com/watch?v={id}");
        crate::open_outside(&url).map_err(|e| format!("the browser did not open: {e}"))?;
        return Ok(format!("Opened {url} in the browser."));
    }
    let page = web::agent()
        .get(&format!("https://www.youtube.com/results?search_query={}&hl=en", enc(q)))
        .header("Accept-Language", "en")
        // Past the cookie question YouTube asks in Europe, so the results come back.
        .header("Cookie", "SOCS=CAI; CONSENT=YES+")
        .call()
        .map_err(|e| format!("YouTube did not answer: {e}"))?
        .body_mut()
        .with_config()
        .limit(8 << 20)
        .read_to_string()
        .map_err(|e| format!("YouTube's answer could not be read: {e}"))?;
    let (id, title) = first_video(&page).ok_or("YouTube found no video for that")?;
    let url = format!("https://www.youtube.com/watch?v={id}");
    crate::open_outside(&url).map_err(|e| format!("the browser did not open: {e}"))?;
    Ok(if title.is_empty() { format!("Playing {url} in the browser.") } else { format!("Playing “{title}” on YouTube in the browser ({url}).") })
}

/// A Spotify link or URI: its kind and id.
fn spotify_ref(s: &str) -> Option<(String, String)> {
    const KINDS: [&str; 5] = ["track", "album", "playlist", "artist", "episode"];
    let s = s.trim();
    let tail = if let Some(i) = s.find("open.spotify.com/") {
        let rest = &s[i + "open.spotify.com/".len()..];
        // Links from other countries carry "intl-de/" first.
        let rest = if rest.starts_with("intl-") { rest.split_once('/').map(|x| x.1).unwrap_or_default() } else { rest };
        rest.replacen('/', ":", 1)
    } else {
        s.strip_prefix("spotify:")?.to_string()
    };
    let (kind, rest) = tail.split_once(':')?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    (KINDS.contains(&kind) && id.len() == 22).then(|| (kind.to_string(), id))
}

/// Whether the Spotify app is on this computer.
fn spotify_installed() -> bool {
    #[cfg(windows)]
    {
        let env = |k: &str| std::env::var_os(k).map(std::path::PathBuf::from);
        env("APPDATA").is_some_and(|d| d.join("Spotify/Spotify.exe").exists())
            || env("LOCALAPPDATA").is_some_and(|d| d.join("Microsoft/WindowsApps/Spotify.exe").exists())
    }
    #[cfg(target_os = "macos")]
    {
        std::path::Path::new("/Applications/Spotify.app").exists()
            || std::env::var_os("HOME").is_some_and(|h| std::path::Path::new(&h).join("Applications/Spotify.app").exists())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("spotify").exists()))
            || std::path::Path::new("/var/lib/flatpak/app/com.spotify.Client").exists()
            || std::path::Path::new("/snap/bin/spotify").exists()
    }
}

/// "Never Gonna Give You Up - song and lyrics by Rick Astley | Spotify" -> the part before " | Spotify".
fn hit_name(title: &str) -> String {
    title.split(" | Spotify").next().unwrap_or(title).trim().to_string()
}

/// Find it and play it in the Spotify app (or Spotify's web player when the app is not
/// installed).
pub fn spotify(query: &str, kind: &str, search: &web::Search) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("say what to play".into());
    }
    let wanted = match kind {
        "album" | "playlist" | "artist" | "episode" => kind,
        _ => "track",
    };
    let (kind, id, name) = match spotify_ref(q) {
        Some((k, id)) => (k, id, String::new()),
        None => {
            let (hits, _) = web::search(search, &format!("site:open.spotify.com/{wanted} {q}"), 8)?;
            let found = hits
                .iter()
                .filter_map(|h| spotify_ref(&h.url).map(|(k, id)| (k, id, hit_name(&h.title))))
                .find(|(k, ..)| k == wanted)
                .or_else(|| hits.iter().find_map(|h| spotify_ref(&h.url).map(|(k, id)| (k, id, hit_name(&h.title)))));
            found.ok_or_else(|| format!("no Spotify {wanted} was found for “{q}”"))?
        }
    };
    if !spotify_installed() {
        let url = format!("https://open.spotify.com/{kind}/{id}");
        crate::open_outside(&url).map_err(|e| format!("the browser did not open: {e}"))?;
        return Ok(format!("The Spotify app is not installed, so {} opened in Spotify's web player ({url}). It may need a press on play.", if name.is_empty() { "it" } else { &name }));
    }
    crate::open_outside(format!("spotify:{kind}:{id}?autoplay=true")).map_err(|e| format!("Spotify did not open: {e}"))?;
    // Spotify takes a few seconds to start; the media session says when it plays.
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(12) {
        std::thread::sleep(Duration::from_millis(700));
        if let Ok(Some(now)) = media::now_playing(false) {
            if now.app.to_lowercase().contains("spotify") && now.playing && (kind != "track" || name.is_empty() || name.to_lowercase().contains(&now.title.to_lowercase())) {
                return Ok(format!("Spotify is playing “{}” by {}.", now.title, now.artist));
            }
        }
    }
    Ok(format!(
        "Opened {} in Spotify, but it did not report playing yet. It may need a moment, or a press on play.",
        if name.is_empty() { format!("{kind} {id}") } else { name }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_links() {
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(youtube_id("https://youtu.be/dQw4w9WgXcQ?si=x").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(youtube_id("https://www.youtube.com/shorts/abcdefghijk").as_deref(), Some("abcdefghijk"));
        assert_eq!(youtube_id("lofi beats"), None);
        assert_eq!(spotify_ref("https://open.spotify.com/intl-de/track/4cOdK2wGLETKBW3PvgPWqT?si=1"), Some(("track".into(), "4cOdK2wGLETKBW3PvgPWqT".into())));
        assert_eq!(spotify_ref("spotify:album:1A2GTWGtFfWp7KSQTwWOyo"), Some(("album".into(), "1A2GTWGtFfWp7KSQTwWOyo".into())));
        assert_eq!(spotify_ref("https://open.spotify.com/user/abc"), None);
        assert_eq!(hit_name("Blinding Lights - song and lyrics by The Weeknd | Spotify"), "Blinding Lights - song and lyrics by The Weeknd");
    }

    #[test]
    fn finds_the_first_video() {
        let page = r#"..."adSlotRenderer":{}..."videoRenderer":{"videoId":"AbCdEfGhIjK","thumbnail":{},"title":{"runs":[{"text":"Lo-fi \"beats\" & rain"}]}}..."videoRenderer":{"videoId":"zzzzzzzzzzz""#;
        assert_eq!(first_video(page), Some(("AbCdEfGhIjK".into(), "Lo-fi \"beats\" & rain".into())));
    }

    /// Asks YouTube for real and checks a video comes back; opens nothing.
    #[test]
    #[ignore = "reaches YouTube"]
    fn live_youtube_search() {
        let page = web::agent()
            .get("https://www.youtube.com/results?search_query=lofi+hip+hop+radio&hl=en")
            .header("Cookie", "SOCS=CAI; CONSENT=YES+")
            .call()
            .unwrap()
            .body_mut()
            .with_config()
            .limit(8 << 20)
            .read_to_string()
            .unwrap();
        let hit = first_video(&page);
        println!("{hit:?}");
        assert!(hit.is_some());
    }

    /// Finds a song through the web search; opens nothing.
    #[test]
    #[ignore = "reaches the web"]
    fn live_spotify_find() {
        let (hits, by) = web::search(&web::Search::default(), "site:open.spotify.com/track blinding lights the weeknd", 8).unwrap();
        let refs: Vec<_> = hits.iter().filter_map(|h| spotify_ref(&h.url).map(|r| (r, hit_name(&h.title)))).collect();
        println!("{by}: {refs:?}");
        assert!(refs.iter().any(|((k, _), _)| k == "track"));
    }
}
