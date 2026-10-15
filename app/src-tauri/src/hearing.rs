//! Listening to the computer's own sound (videos, calls, music), so the agent can be
//! asked what was just said. Off until switched on in Settings.
//!
//! What the speakers play is captured from the system's mix (loopback; the microphone
//! is never opened), cut into short pieces at quiet moments, and written down by the
//! Whisper model Lemonade runs on this computer. Only the words are kept, and only for
//! the last twenty seconds: each line is dropped once it is older than that. The sound
//! itself is let go as soon as it is written down. Nothing is saved to disk.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// How far back the transcript reaches.
pub const KEEP: Duration = Duration::from_secs(20);
/// A piece is cut at the first quiet moment after this much sound…
const MIN_PIECE: f32 = 2.5;
/// …or here at the latest.
const MAX_PIECE: f32 = 8.0;
/// Below this loudness (RMS) a stretch counts as quiet, and a whole piece as silence.
const QUIET: f32 = 0.006;
const SILENT: f32 = 0.0035;
/// Whisper hears 16 kHz mono.
const RATE: u32 = 16_000;

#[derive(Debug, Clone, Serialize)]
pub struct Line {
    /// When this stretch of sound began and ended, in ms since 1970.
    pub from: i64,
    pub to: i64,
    pub text: String,
}

#[derive(Default)]
pub struct Hearing {
    on: AtomicBool,
    /// Bumped each time listening is switched on, so an old capture thread knows to end.
    epoch: AtomicU64,
    lines: Mutex<VecDeque<Line>>,
    /// Captured sound waiting to be cut, already mono at the device's rate.
    pending: Mutex<Pending>,
    /// Why nothing is being heard, if something is wrong (for Settings).
    trouble: Mutex<Option<String>>,
}

#[derive(Default)]
struct Pending {
    samples: Vec<f32>,
    rate: u32,
}

fn ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

pub(crate) fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
}

/// To 16 kHz by averaging the samples that fall in each output step (a box filter, which
/// keeps the loudest aliasing out of speech).
pub(crate) fn resample(s: &[f32], from: u32) -> Vec<f32> {
    if from == RATE || from == 0 {
        return s.to_vec();
    }
    let step = from as f64 / RATE as f64;
    let n = (s.len() as f64 / step) as usize;
    (0..n)
        .map(|i| {
            let a = (i as f64 * step) as usize;
            let b = (((i + 1) as f64 * step) as usize).clamp(a + 1, s.len());
            s[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect()
}

pub(crate) fn wav(s: &[f32]) -> Vec<u8> {
    let data = (s.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for x in s {
        w.extend_from_slice(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    w
}

/// Whisper writes these over silence, applause or music when there are no words.
pub(crate) fn is_filler(t: &str) -> bool {
    let low = t.to_lowercase();
    let bare: String = low.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect();
    let bare = bare.split_whitespace().collect::<Vec<_>>().join(" ");
    bare.is_empty()
        || (low.starts_with('[') && low.ends_with(']'))
        || matches!(
            bare.as_str(),
            "thank you" | "thanks for watching" | "thank you for watching" | "you" | "bye" | "subtitles by the amaraorg community" | "untertitel im auftrag des zdf 2017"
        )
}

/// Send a piece to Lemonade's Whisper and get its words back. `hint` is text Whisper takes
/// as what came just before, which steers how it spells names it hears; `language` is
/// what is spoken ("en"), or None to let Whisper guess.
pub(crate) fn transcribe(url: &str, model: &str, wav: Vec<u8>, hint: Option<&str>, language: Option<&str>) -> Result<String, String> {
    let boundary = format!("nimbus{}", ms(SystemTime::now()));
    let mut body = Vec::with_capacity(wav.len() + 512);
    let field = |body: &mut Vec<u8>, name: &str, value: &str| {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    };
    field(&mut body, "model", model);
    field(&mut body, "response_format", "json");
    if let Some(h) = hint {
        field(&mut body, "prompt", h);
    }
    if let Some(l) = language.filter(|l| !l.is_empty()) {
        field(&mut body, "language", l);
    }
    body.extend_from_slice(
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"piece.wav\"\r\nContent-Type: audio/wav\r\n\r\n").as_bytes(),
    );
    body.extend_from_slice(&wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(800)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut res = agent
        .post(&format!("{url}/audio/transcriptions"))
        .header("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send(&body[..])
        .map_err(|e| e.to_string())?;
    let status = res.status().as_u16();
    let raw = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(|_| format!("unexpected answer: {}", raw.chars().take(160).collect::<String>()))?;
    if status >= 400 {
        return Err(v.to_string());
    }
    // Whisper puts each sentence on a line of its own: one line per piece reads better.
    Ok(v.get("text").and_then(|t| t.as_str()).unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" "))
}

impl Hearing {
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// The lines from the last twenty seconds, oldest first.
    pub fn lines(&self) -> Vec<Line> {
        self.lines.lock().iter().cloned().collect()
    }

    pub fn trouble(&self) -> Option<String> {
        self.trouble.lock().clone()
    }

    /// Drop lines older than twenty seconds. True when any went.
    fn prune(&self) -> bool {
        let edge = ms(SystemTime::now()) - KEEP.as_millis() as i64;
        let mut q = self.lines.lock();
        let before = q.len();
        q.retain(|l| l.to >= edge);
        q.len() != before
    }

    fn forget(&self) {
        self.lines.lock().clear();
        let mut p = self.pending.lock();
        p.samples.clear();
        p.samples.shrink_to_fit();
    }

    /// The transcript for the agent: each line with how long ago it ended.
    pub fn transcript(&self) -> String {
        let now = ms(SystemTime::now());
        let lines = self.lines();
        if lines.is_empty() {
            return "Nothing was said in the last 20 seconds.".into();
        }
        lines.iter().map(|l| format!("[{} s ago] {}", ((now - l.to) / 1000).max(0), l.text)).collect::<Vec<_>>().join("\n")
    }
}

fn changed(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let _ = app.emit("hearing", Snapshot::of(&state.hearing));
}

/// What Settings shows.
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub on: bool,
    pub lines: Vec<Line>,
    pub trouble: Option<String>,
}

impl Snapshot {
    pub fn of(h: &Hearing) -> Self {
        Self { on: h.on(), lines: h.lines(), trouble: h.trouble() }
    }
}

pub fn set(app: &AppHandle, on: bool) {
    let state = app.state::<crate::AppState>();
    let was = state.hearing.on.swap(on, Ordering::SeqCst);
    if !on {
        state.hearing.forget();
        *state.hearing.trouble.lock() = None;
    }
    if on && !was {
        let mine = state.hearing.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        capture(app.clone(), mine);
    }
    changed(app);
}

fn trouble(app: &AppHandle, why: Option<String>) {
    let state = app.state::<crate::AppState>();
    let mut t = state.hearing.trouble.lock();
    if *t != why {
        *t = why;
        drop(t);
        changed(app);
    }
}

/// Capture the speakers' mix while listening is on, on a thread of its own (an audio
/// stream may not cross threads). Follows the default output when it changes.
fn capture(app: AppHandle, mine: u64) {
    std::thread::Builder::new()
        .name("hearing-capture".into())
        .spawn(move || {
            let hearing = &app.state::<crate::AppState>().hearing;
            let current = || hearing.on() && hearing.epoch.load(Ordering::SeqCst) == mine;
            while current() {
                let failed = Arc::new(AtomicBool::new(false));
                let (stream, name) = match open(&app, failed.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        trouble(&app, Some(e));
                        std::thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                };
                trouble(&app, None);
                let _ = stream.play();
                // Hold the stream until listening stops, it fails, or the output moves.
                let mut checked = Instant::now();
                while current() && !failed.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    if checked.elapsed() > Duration::from_secs(3) {
                        checked = Instant::now();
                        let now = cpal::default_host().default_output_device().and_then(|d| d.name().ok());
                        if now.as_deref() != Some(name.as_str()) {
                            break;
                        }
                    }
                }
                drop(stream);
                hearing.pending.lock().samples.clear();
            }
        })
        .ok();
}

fn open(app: &AppHandle, failed: Arc<AtomicBool>) -> Result<(cpal::Stream, String), String> {
    let device = cpal::default_host().default_output_device().ok_or("there is no sound output to listen to")?;
    let name = device.name().unwrap_or_default();
    let config = device.default_output_config().map_err(|e| format!("the sound output could not be read: {e}"))?;
    let channels = config.channels().max(1) as usize;
    let rate = config.sample_rate().0;
    app.state::<crate::AppState>().hearing.pending.lock().rate = rate;
    let sink = app.clone();
    let push = move |mono: &mut dyn Iterator<Item = f32>| {
        let state = sink.state::<crate::AppState>();
        let mut p = state.hearing.pending.lock();
        p.samples.extend(mono);
        // Never more than the longest piece and a bit, even if the writer falls behind.
        let cap = (rate as f32 * (MAX_PIECE + 4.0)) as usize;
        if p.samples.len() > cap {
            let over = p.samples.len() - cap;
            p.samples.drain(..over);
        }
    };
    let on_err = move |e: cpal::StreamError| {
        eprintln!("[nimbus] listening stopped: {e}");
        failed.store(true, Ordering::Relaxed);
    };
    // On Windows an input stream on an output device captures what it plays (loopback).
    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.config(),
            move |d: &[f32], _: &cpal::InputCallbackInfo| push(&mut d.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32)),
            on_err,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.config(),
            move |d: &[i16], _: &cpal::InputCallbackInfo| {
                push(&mut d.chunks(channels).map(|f| f.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / channels as f32))
            },
            on_err,
            None,
        ),
        other => return Err(format!("the sound output uses {other:?}, which Nimbus cannot read yet")),
    }
    .map_err(|e| format!("the sound output could not be listened to: {e}"))?;
    Ok((stream, name))
}

/// A piece ready to be written down: cut at the first quiet moment after enough sound,
/// or at the longest a piece may be. Silence is let go without being sent anywhere.
fn take_piece(p: &mut Pending) -> Option<(Vec<f32>, u32, f32)> {
    let rate = p.rate.max(1) as f32;
    let secs = p.samples.len() as f32 / rate;
    if secs < MIN_PIECE {
        return None;
    }
    let tail = &p.samples[p.samples.len().saturating_sub((rate * 0.3) as usize)..];
    if secs < MAX_PIECE && rms(tail) >= QUIET {
        return None;
    }
    let samples = std::mem::take(&mut p.samples);
    (rms(&samples) >= SILENT).then_some((samples, p.rate, secs))
}

/// Cuts pieces at quiet moments, writes them down, and lets old lines go. Runs for the
/// life of the app; idle when listening is off.
pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("hearing-writer".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(250));
                let state = app.state::<crate::AppState>();
                let h = &state.hearing;
                if h.prune() {
                    changed(&app);
                }
                if !h.on() {
                    continue;
                }
                let piece = take_piece(&mut h.pending.lock());
                let Some((samples, rate, secs)) = piece else { continue };
                let to = SystemTime::now();
                let from = to - Duration::from_secs_f32(secs);
                let ready = match &*state.speech.lock() {
                    crate::speech::Speech::Ready { model, url } => Some((url.clone(), model.clone())),
                    _ => None,
                };
                let Some((url, model)) = ready else {
                    trouble(&app, Some("waiting for speech to text to be ready".into()));
                    continue;
                };
                match transcribe(&url, &model, wav(&resample(&samples, rate)), None, None) {
                    Ok(text) if !is_filler(&text) && h.on() => {
                        trouble(&app, None);
                        h.lines.lock().push_back(Line { from: ms(from), to: ms(to), text });
                        h.prune();
                        changed(&app);
                    }
                    Ok(_) => trouble(&app, None),
                    Err(e) => trouble(&app, Some(format!("speech to text did not answer: {e}"))),
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_and_encodes() {
        let s: Vec<f32> = (0..48_000).map(|i| (i as f32 / 48.0).sin() * 0.5).collect();
        let r = resample(&s, 48_000);
        assert_eq!(r.len(), 16_000);
        let w = wav(&r);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 32_000);
    }

    #[test]
    fn filler_is_dropped() {
        assert!(is_filler(" Thank you."));
        assert!(is_filler("[BLANK_AUDIO]"));
        assert!(is_filler(" . "));
        assert!(!is_filler("The meeting moves to Thursday."));
    }

    /// Needs Lemonade with Whisper loaded, and a 16 kHz mono 16-bit WAV in NIMBUS_TEST_WAV.
    #[test]
    #[ignore]
    fn live_transcribe() {
        let raw = std::fs::read(std::env::var("NIMBUS_TEST_WAV").unwrap()).unwrap();
        let data = raw.windows(4).position(|w| w == b"data").unwrap() + 8;
        let s: Vec<f32> = raw[data..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        // As if captured at 48 kHz: up, then back down through our own path.
        let up: Vec<f32> = s.iter().flat_map(|&x| [x, x, x]).collect();
        let text = transcribe("http://localhost:13305/api/v1", "Whisper-Large-v3-Turbo", wav(&resample(&up, 48_000)), None, None).unwrap();
        println!("heard: {text}");
        assert!(text.to_lowercase().contains("thursday"));
    }

    #[test]
    fn pieces_are_cut_at_quiet_moments() {
        let speech = |secs: f32| (0..(48_000.0 * secs) as usize).map(|i| (i as f32 / 9.0).sin() * 0.2).collect::<Vec<f32>>();
        let quiet = |secs: f32| vec![0.0f32; (48_000.0 * secs) as usize];
        let mut p = Pending { samples: speech(1.5), rate: 48_000 };
        assert!(take_piece(&mut p).is_none(), "too short");
        p.samples.extend(speech(1.5));
        assert!(take_piece(&mut p).is_none(), "still talking");
        p.samples.extend(quiet(0.4));
        let (piece, _, secs) = take_piece(&mut p).expect("cut at the pause");
        assert!((secs - 3.4).abs() < 0.01 && piece.len() == (48_000.0 * 3.4) as usize && p.samples.is_empty());
        let mut long = Pending { samples: speech(8.1), rate: 48_000 };
        assert!(take_piece(&mut long).is_some(), "cut at the longest");
        let mut hush = Pending { samples: quiet(3.0), rate: 48_000 };
        assert!(take_piece(&mut hush).is_none() && hush.samples.is_empty(), "silence goes unsent");
    }

    #[test]
    fn old_lines_go() {
        let h = Hearing::default();
        let now = ms(SystemTime::now());
        h.lines.lock().push_back(Line { from: now - 30_000, to: now - 25_000, text: "old".into() });
        h.lines.lock().push_back(Line { from: now - 6_000, to: now - 2_000, text: "new".into() });
        assert!(h.prune());
        assert_eq!(h.lines().len(), 1);
        assert!(h.transcript().contains("new"));
    }
}
