//! "Hey Nimbus": asking out loud. Off until switched on in Settings.
//!
//! When on, the microphone is open. Each stretch of speech, cut where the speaker pauses,
//! is written down by the Whisper model Lemonade runs on this computer; the sound is let
//! go as soon as it is written down, and nothing is saved. The words go nowhere unless
//! they begin with the wake phrase ("Hey Nimbus, …", "Okay Nimbus, …", "Nimbus, …"): then
//! the rest goes to the chat as if typed. The wake phrase alone opens a short window, and
//! the next thing said is the request. While Nimbus reads its answer aloud, what the
//! microphone hears is dropped, so it does not hear itself.
//!
//! Push to talk is the other way in: the microphone button in the chat opens the
//! microphone for one request only (see `talk`), whether or not "Hey Nimbus" is on.

use crate::hearing::{is_filler, resample, rms, transcribe, wav};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// After the wake phrase alone, how long the next words count as the request. What
/// counts is when the speaker started on them, not when they were written down: a long
/// request would run past the window otherwise.
const AWAKE: Duration = Duration::from_secs(8);
/// Speech is judged in frames this long.
const FRAME: f32 = 0.02;
/// Kept from just before speech starts, so the first syllable is not clipped.
const PRE_ROLL: f32 = 0.3;
/// A pause this long ends what was said…
const PAUSE: f32 = 0.75;
/// …and anything with less speech than this was a cough or a click.
const MIN_SPEECH: f32 = 0.35;
/// The longest stretch sent at once.
const MAX_SPEECH: f32 = 15.0;
/// Push to talk waits for a longer pause (the person may be thinking) and takes more.
const TALK_PAUSE: f32 = 1.3;
const TALK_MAX: f32 = 30.0;
/// Push to talk gives up when nothing is said for this long.
const TALK_WAIT: i64 = 8_000;
/// Push to talk: what the button is doing.
const IDLE: u8 = 0;
const LISTENING: u8 = 1;
const WRITING: u8 = 2;
/// Speech must be this much louder than the room's own noise, and never quieter than
/// the floor below.
const OVER_NOISE: f32 = 3.0;
const FLOOR: f32 = 0.004;
/// What the microphone heard just after Nimbus stopped speaking may still be its echo.
const ECHO: i64 = 400;
/// Outside the window after the wake phrase, a long stretch (music, a video, a talk in the
/// room) is first written down only this far: the wake phrase comes first or not at all.
/// Writing down all of it, all day, keeps the computer busy.
const HEAD: f32 = 3.0;
/// Given to Whisper as the words before, so it writes the name as "Nimbus" rather than
/// "Nimbos" or "limbus". Checked not to make it hear the name in other words.
const NAME_HINT: &str = "Hey Nimbus.";

#[derive(Default)]
pub struct Voice {
    on: AtomicBool,
    /// The microphone is open (a capture thread runs).
    capturing: AtomicBool,
    /// Push to talk: IDLE, LISTENING or WRITING.
    talk: AtomicU8,
    /// The person pressed the button again: write down what was said so far.
    talk_end: AtomicBool,
    /// When push to talk started listening, in ms since 1970.
    talk_since: AtomicI64,
    /// Nimbus is reading an answer aloud (until this time, in ms since 1970, once done).
    speaking: AtomicBool,
    quiet_after: AtomicI64,
    /// Until when the next words are the request, in ms since 1970.
    awake_until: AtomicI64,
    pending: Mutex<Pending>,
    trouble: Mutex<Option<String>>,
}

#[derive(Default)]
struct Pending {
    samples: Vec<f32>,
    rate: u32,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

impl Voice {
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Whether the microphone should be open.
    fn listening(&self) -> bool {
        self.on() || self.talk.load(Ordering::Relaxed) == LISTENING
    }

    fn awake(&self) -> bool {
        now_ms() < self.awake_until.load(Ordering::Relaxed)
    }

    /// Whether what the microphone hears now should be dropped.
    fn deaf(&self) -> bool {
        self.speaking.load(Ordering::Relaxed) || now_ms() < self.quiet_after.load(Ordering::Relaxed)
    }
}

/// What Settings and the notch show.
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub on: bool,
    /// The wake phrase was heard and the request is awaited.
    pub awake: bool,
    pub trouble: Option<String>,
}

impl Snapshot {
    pub fn of(v: &Voice) -> Self {
        Self { on: v.on(), awake: v.on() && v.awake(), trouble: v.trouble.lock().clone() }
    }
}

fn changed(app: &AppHandle) {
    let _ = app.emit("voice", Snapshot::of(&app.state::<crate::AppState>().voice));
}

pub fn set(app: &AppHandle, on: bool) {
    let v = &app.state::<crate::AppState>().voice;
    let was = v.on.swap(on, Ordering::SeqCst);
    if !on {
        v.pending.lock().samples.clear();
        v.awake_until.store(0, Ordering::Relaxed);
        *v.trouble.lock() = None;
    }
    if on && !was {
        capture(app.clone());
    }
    changed(app);
}

/// Push to talk, as the chat's microphone button shows it.
#[derive(Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Talk {
    Listening,
    Writing,
    /// What was said; empty when nothing was.
    Done { text: String },
    Failed { message: String },
}

fn talk_is(app: &AppHandle, t: Talk) {
    let _ = app.emit("talk", t);
}

/// The microphone button was pressed: listen for one request.
pub fn talk_start(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<crate::AppState>();
    if !matches!(&*state.speech.lock(), crate::speech::Speech::Ready { .. }) {
        return Err("Speech to text is not ready yet. Settings shows how far it is.".into());
    }
    let v = &state.voice;
    if v.talk.compare_exchange(IDLE, LISTENING, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("I'm still listening or writing down the last request.".into());
    }
    v.talk_end.store(false, Ordering::SeqCst);
    v.talk_since.store(now_ms(), Ordering::SeqCst);
    talk_is(app, Talk::Listening);
    capture(app.clone());
    Ok(())
}

/// The microphone could not be used: push to talk says so at once rather than wait.
fn talk_failed(app: &AppHandle, why: &str) {
    let v = &app.state::<crate::AppState>().voice;
    if v.talk.compare_exchange(LISTENING, IDLE, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        let mut why = why.to_string();
        if let Some(c) = why.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        talk_is(app, Talk::Failed { message: format!("{why}.") });
    }
}

/// The button was pressed again: `keep` writes down what was said so far, otherwise
/// it is dropped.
pub fn talk_stop(app: &AppHandle, keep: bool) {
    let v = &app.state::<crate::AppState>().voice;
    if keep {
        v.talk_end.store(true, Ordering::SeqCst);
    } else if v.talk.compare_exchange(LISTENING, IDLE, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        talk_is(app, Talk::Done { text: String::new() });
    }
}

/// The page says when it starts and stops reading an answer aloud.
pub fn speaking(app: &AppHandle, on: bool) {
    let v = &app.state::<crate::AppState>().voice;
    v.speaking.store(on, Ordering::SeqCst);
    if !on {
        v.quiet_after.store(now_ms() + ECHO, Ordering::SeqCst);
    }
    v.pending.lock().samples.clear();
}

fn trouble(app: &AppHandle, why: Option<String>) {
    let v = &app.state::<crate::AppState>().voice;
    let mut t = v.trouble.lock();
    if *t != why {
        *t = why;
        drop(t);
        changed(app);
    }
}

/// Words that start like the name, as Whisper writes it when it mishears.
fn is_name(w: &str) -> bool {
    matches!(w, "nimbus" | "nimbas" | "nimbos" | "nimbis" | "nimbuss" | "nimbusz" | "nimbo" | "nimbu" | "nimbes" | "nimbys" | "limbus" | "nimbles")
}

fn is_greeting(w: &str) -> bool {
    matches!(w, "hey" | "hi" | "hello" | "okay" | "ok" | "yo" | "hallo" | "hej" | "he")
}

/// What follows the wake phrase: `Some("")` for the phrase alone, `None` when the words
/// do not start with it. A misheard name only counts after a greeting.
pub fn after_wake(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let bare = |w: &str| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let mut i = words.iter().position(|w| !bare(w).is_empty())?;
    let greeted = is_greeting(&bare(words[i]));
    if greeted {
        i += 1;
    }
    let name = bare(words.get(i)?);
    if !(name == "nimbus" || (greeted && is_name(&name))) {
        return None;
    }
    let rest = words[i + 1..].join(" ");
    Some(rest.trim_start_matches(|c: char| !c.is_alphanumeric()).trim_end().to_string())
}

/// Cuts the microphone's sound into what was said, at pauses, against the room's own
/// noise level, which it learns as it goes.
struct Cutter {
    rate: f32,
    noise: f32,
    carry: Vec<f32>,
    pre: VecDeque<f32>,
    said: Vec<f32>,
    speaking: bool,
    voiced: f32,
    pause: f32,
    /// The pause that ends a stretch, and the longest stretch, in seconds.
    ends_after: f32,
    longest: f32,
}

impl Cutter {
    fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1) as f32,
            noise: 0.0,
            carry: Vec::new(),
            pre: VecDeque::new(),
            said: Vec::new(),
            speaking: false,
            voiced: 0.0,
            pause: 0.0,
            ends_after: PAUSE,
            longest: MAX_SPEECH,
        }
    }

    /// For push to talk.
    fn talk(rate: u32) -> Self {
        Self { ends_after: TALK_PAUSE, longest: TALK_MAX, ..Self::new(rate) }
    }

    /// What is being said right now, if it is speech so far (the button was pressed again).
    fn rest(&mut self) -> Option<Vec<f32>> {
        let said = std::mem::take(&mut self.said);
        (self.speaking && self.voiced >= MIN_SPEECH).then_some(said)
    }

    fn heard_speech(&self) -> bool {
        self.speaking
    }

    /// Feed new sound; gives back each stretch of speech once it ends.
    fn feed(&mut self, s: &[f32]) -> Vec<Vec<f32>> {
        let mut out = Vec::new();
        self.carry.extend_from_slice(s);
        let n = (self.rate * FRAME) as usize;
        let frames = self.carry.len() / n.max(1);
        let all = std::mem::take(&mut self.carry);
        for f in all.chunks(n.max(1)).take(frames) {
            if let Some(done) = self.frame(f) {
                out.push(done);
            }
        }
        self.carry = all[frames * n..].to_vec();
        out
    }

    fn frame(&mut self, f: &[f32]) -> Option<Vec<f32>> {
        let level = rms(f);
        if self.noise == 0.0 {
            self.noise = level.max(1e-5);
        }
        let loud = level > (self.noise * OVER_NOISE).max(FLOOR);
        if !self.speaking {
            // The room's noise: follow it down quickly, up slowly.
            self.noise = if level < self.noise { self.noise * 0.8 + level * 0.2 } else { self.noise * 0.995 + level * 0.005 };
            if loud {
                self.speaking = true;
                self.said = self.pre.drain(..).collect();
                self.said.extend_from_slice(f);
                self.voiced = FRAME;
                self.pause = 0.0;
            } else {
                self.pre.extend(f);
                let keep = (self.rate * PRE_ROLL) as usize;
                while self.pre.len() > keep {
                    self.pre.pop_front();
                }
            }
            return None;
        }
        self.said.extend_from_slice(f);
        if loud {
            self.voiced += FRAME;
            self.pause = 0.0;
        } else {
            self.pause += FRAME;
        }
        let long = self.said.len() as f32 / self.rate >= self.longest;
        if self.pause < self.ends_after && !long {
            return None;
        }
        self.speaking = false;
        let said = std::mem::take(&mut self.said);
        (self.voiced >= MIN_SPEECH).then_some(said)
    }
}

/// Listen to the default microphone while voice or push to talk needs it, on a thread
/// of its own (one at most). Follows the default microphone when it changes.
fn capture(app: AppHandle) {
    if app.state::<crate::AppState>().voice.capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("voice-capture".into())
        .spawn(move || {
            let v = &app.state::<crate::AppState>().voice;
            let current = || v.listening();
            loop {
                if !current() {
                    v.capturing.store(false, Ordering::SeqCst);
                    // Switched on again just now: carry on rather than leave it closed.
                    if current() && !v.capturing.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
                let failed = Arc::new(AtomicBool::new(false));
                let (stream, name) = match open(&app, failed.clone()) {
                    Ok(s) => s,
                    Err(e) => {
                        talk_failed(&app, &e);
                        trouble(&app, Some(e));
                        std::thread::sleep(Duration::from_secs(3));
                        continue;
                    }
                };
                trouble(&app, None);
                if let Err(e) = stream.play() {
                    let e = format!("the microphone did not start: {e}");
                    talk_failed(&app, &e);
                    trouble(&app, Some(e));
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
                let mut ticks = 0;
                while current() && !failed.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    ticks += 1;
                    if ticks % 15 == 0 {
                        let now = cpal::default_host().default_input_device().and_then(|d| d.name().ok());
                        if now.as_deref() != Some(name.as_str()) {
                            break;
                        }
                    }
                }
                drop(stream);
                v.pending.lock().samples.clear();
            }
        })
        .ok();
}

fn open(app: &AppHandle, failed: Arc<AtomicBool>) -> Result<(cpal::Stream, String), String> {
    let device = cpal::default_host().default_input_device().ok_or("there is no microphone")?;
    let name = device.name().unwrap_or_default();
    let config = device.default_input_config().map_err(|e| format!("the microphone could not be read ({e}); check that apps may use it in the system's privacy settings"))?;
    let channels = config.channels().max(1) as usize;
    let rate = config.sample_rate().0;
    app.state::<crate::AppState>().voice.pending.lock().rate = rate;
    let sink = app.clone();
    let push = move |mono: &mut dyn Iterator<Item = f32>| {
        let state = sink.state::<crate::AppState>();
        let v = &state.voice;
        if v.deaf() {
            return;
        }
        let mut p = v.pending.lock();
        p.samples.extend(mono);
        // A few seconds at most, even if the writer falls behind.
        let cap = rate as usize * 5;
        if p.samples.len() > cap {
            let over = p.samples.len() - cap;
            p.samples.drain(..over);
        }
    };
    let on_err = move |e: cpal::StreamError| {
        eprintln!("[nimbus] the microphone stopped: {e}");
        failed.store(true, Ordering::Relaxed);
    };
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
        other => return Err(format!("the microphone uses {other:?}, which Nimbus cannot read yet")),
    }
    .map_err(|e| format!("the microphone could not be opened ({e}); check that apps may use it in the system's privacy settings"))?;
    Ok((stream, name))
}

/// A request heard out loud, for the page to send to the chat.
#[derive(Clone, Serialize)]
struct Heard {
    text: String,
}

/// Cuts what the microphone hears, writes it down, and passes requests on. Runs for the
/// life of the app; idle when voice is off.
pub fn spawn(app: AppHandle) {
    std::thread::Builder::new()
        .name("voice-writer".into())
        .spawn(move || {
            let mut cutter: Option<Cutter> = None;
            let mut talker: Option<Cutter> = None;
            let mut was_awake = false;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let state = app.state::<crate::AppState>();
                let v = &state.voice;
                // Push to talk comes first: its words are the request, wake phrase or not.
                if v.talk.load(Ordering::SeqCst) == LISTENING {
                    cutter = None;
                    let (fresh, rate) = {
                        let mut p = v.pending.lock();
                        (std::mem::take(&mut p.samples), p.rate)
                    };
                    let c = match &mut talker {
                        Some(c) if c.rate == rate.max(1) as f32 => c,
                        _ => talker.insert(Cutter::talk(rate)),
                    };
                    let mut said = c.feed(&fresh).into_iter().next();
                    if said.is_none() && v.talk_end.load(Ordering::SeqCst) {
                        said = c.rest();
                        if said.is_none() {
                            // Pressed again before saying anything.
                            talker = None;
                            v.talk.store(IDLE, Ordering::SeqCst);
                            talk_is(&app, Talk::Done { text: String::new() });
                            continue;
                        }
                    }
                    let Some(said) = said else {
                        let waited = now_ms() - v.talk_since.load(Ordering::SeqCst);
                        if !c.heard_speech() && waited > TALK_WAIT {
                            talker = None;
                            v.talk.store(IDLE, Ordering::SeqCst);
                            talk_is(&app, Talk::Failed { message: "I did not hear anything. Is the right microphone chosen in Windows?".into() });
                        }
                        continue;
                    };
                    talker = None;
                    v.talk.store(WRITING, Ordering::SeqCst);
                    talk_is(&app, Talk::Writing);
                    let ready = match &*state.speech.lock() {
                        crate::speech::Speech::Ready { model, url } => Some((url.clone(), model.clone())),
                        _ => None,
                    };
                    let language = state.settings.lock().speech_language.clone();
                    let done = match ready {
                        None => Talk::Failed { message: "Speech to text stopped being ready.".into() },
                        Some((url, model)) => match transcribe(&url, &model, wav(&resample(&said, rate)), None, language.as_deref()) {
                            Ok(t) if is_filler(&t) => Talk::Done { text: String::new() },
                            Ok(t) => Talk::Done { text: t.split_whitespace().collect::<Vec<_>>().join(" ") },
                            Err(e) => Talk::Failed { message: format!("Speech to text did not answer: {e}") },
                        },
                    };
                    v.talk.store(IDLE, Ordering::SeqCst);
                    talk_is(&app, done);
                    continue;
                }
                talker = None;
                // The window after the wake phrase closing shows in the notch.
                let awake = v.on() && v.awake();
                if awake != was_awake {
                    was_awake = awake;
                    changed(&app);
                }
                if !v.on() || v.deaf() {
                    cutter = None;
                    continue;
                }
                let (fresh, rate) = {
                    let mut p = v.pending.lock();
                    (std::mem::take(&mut p.samples), p.rate)
                };
                let c = match &mut cutter {
                    Some(c) if c.rate == rate.max(1) as f32 => c,
                    _ => cutter.insert(Cutter::new(rate)),
                };
                // When each stretch began: it ended about now, and is this long.
                let pieces: Vec<(Vec<f32>, i64)> = c
                    .feed(&fresh)
                    .into_iter()
                    .map(|said| {
                        let began = now_ms() - (said.len() as f32 / rate.max(1) as f32 * 1000.0) as i64;
                        (said, began)
                    })
                    .collect();
                for (said, began) in pieces {
                    // Started while the window after the wake phrase was open.
                    let in_window = began < v.awake_until.load(Ordering::SeqCst);
                    let ready = match &*state.speech.lock() {
                        crate::speech::Speech::Ready { model, url } => Some((url.clone(), model.clone())),
                        _ => None,
                    };
                    let Some((url, model)) = ready else {
                        trouble(&app, Some("waiting for speech to text to be ready".into()));
                        continue;
                    };
                    let language = state.settings.lock().speech_language.clone();
                    let head = (rate as f32 * HEAD) as usize;
                    if !in_window && said.len() > head + rate as usize {
                        match transcribe(&url, &model, wav(&resample(&said[..head], rate)), Some(NAME_HINT), language.as_deref()) {
                            Ok(t) if after_wake(&t).is_none() => {
                                if std::env::var_os("NIMBUS_VOICE_DEBUG").is_some() {
                                    let first: Vec<&str> = t.split_whitespace().take(6).collect();
                                    eprintln!("[voice] {:.1} s, first {HEAD} s only: {:?} -> not for Nimbus", said.len() as f32 / rate.max(1) as f32, first.join(" "));
                                }
                                continue;
                            }
                            // It starts with the wake phrase (or could not be checked): all of it.
                            _ => {}
                        }
                    }
                    let text = match transcribe(&url, &model, wav(&resample(&said, rate)), Some(NAME_HINT), language.as_deref()) {
                        Ok(t) => t,
                        Err(e) => {
                            trouble(&app, Some(format!("speech to text did not answer: {e}")));
                            continue;
                        }
                    };
                    trouble(&app, None);
                    // For finding out why the wake phrase was missed: what each stretch of
                    // speech was written as (its first words only), in the developer's console.
                    if std::env::var_os("NIMBUS_VOICE_DEBUG").is_some() {
                        let first: Vec<&str> = text.split_whitespace().take(6).collect();
                        eprintln!(
                            "[voice] {:.1} s, room {:.4}: {:?}{} -> {}",
                            said.len() as f32 / rate.max(1) as f32,
                            c.noise,
                            first.join(" "),
                            if text.split_whitespace().count() > 6 { " …" } else { "" },
                            match after_wake(&text) {
                                Some(r) if r.is_empty() => "wake phrase alone".to_string(),
                                Some(_) => "wake phrase + request".to_string(),
                                None if in_window => "request (after the wake phrase)".to_string(),
                                None => "not for Nimbus".to_string(),
                            }
                        );
                    }
                    if is_filler(&text) || !v.on() {
                        continue;
                    }
                    let request = match after_wake(&text) {
                        Some(rest) if rest.is_empty() => {
                            v.awake_until.store(now_ms() + AWAKE.as_millis() as i64, Ordering::SeqCst);
                            changed(&app);
                            continue;
                        }
                        Some(rest) => rest,
                        None if in_window => text,
                        // Not meant for Nimbus: let it go.
                        None => continue,
                    };
                    v.awake_until.store(0, Ordering::SeqCst);
                    changed(&app);
                    let _ = app.emit("voice-request", Heard { text: request });
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_phrase() {
        assert_eq!(after_wake("Hey Nimbus, play lofi on YouTube.").as_deref(), Some("play lofi on YouTube."));
        assert_eq!(after_wake("Okay, Nimbus. Check my mail").as_deref(), Some("Check my mail"));
        assert_eq!(after_wake("Nimbus, what time is it?").as_deref(), Some("what time is it?"));
        assert_eq!(after_wake(" Hey Nimbus.").as_deref(), Some(""));
        assert_eq!(after_wake("Hey Nimbis, open my mail").as_deref(), Some("open my mail"));
        assert_eq!(after_wake("Hallo Nimbus, spiel Musik").as_deref(), Some("spiel Musik"));
        // A misheard name needs the greeting; the name later in a sentence is not a call.
        assert_eq!(after_wake("Nimble fingers are useful"), None);
        assert_eq!(after_wake("I told Nimbus to wait"), None);
        assert_eq!(after_wake("Hey there"), None);
        assert_eq!(after_wake(""), None);
    }

    #[test]
    fn speech_is_cut_at_pauses() {
        let rate = 16_000;
        let speech = |secs: f32| (0..(rate as f32 * secs) as usize).map(|i| (i as f32 / 7.0).sin() * 0.2).collect::<Vec<f32>>();
        let room = |secs: f32| (0..(rate as f32 * secs) as usize).map(|i| ((i * 7919 % 13) as f32 / 13.0 - 0.5) * 0.002).collect::<Vec<f32>>();
        let mut c = Cutter::new(rate);
        assert!(c.feed(&room(1.0)).is_empty(), "room noise alone");
        assert!(c.feed(&speech(1.2)).is_empty(), "still talking");
        assert!(c.feed(&room(0.3)).is_empty(), "a short breath does not end it");
        assert!(c.feed(&speech(0.8)).is_empty());
        let out = c.feed(&room(1.0));
        assert_eq!(out.len(), 1, "cut at the pause");
        let secs = out[0].len() as f32 / rate as f32;
        assert!((2.9..3.4).contains(&secs), "pre-roll, speech, breath, speech, pause: {secs}");
        // A click is not speech.
        assert!(c.feed(&speech(0.1)).is_empty());
        assert!(c.feed(&room(1.0)).is_empty(), "too short to be speech");
        // A long speech is cut at the longest.
        let mut c = Cutter::new(rate);
        c.feed(&room(0.5));
        assert_eq!(c.feed(&speech(MAX_SPEECH + 0.5)).len(), 1);
        // Push to talk lets the speaker think for a second, and can be ended by hand.
        let mut c = Cutter::talk(rate);
        c.feed(&room(0.5));
        assert!(c.feed(&speech(1.0)).is_empty());
        assert!(c.feed(&room(1.0)).is_empty(), "a second of thinking does not end it");
        assert!(c.feed(&speech(0.5)).is_empty());
        let rest = c.rest().expect("pressed again while talking");
        assert!(rest.len() as f32 / rate as f32 > 2.5);
        let mut c = Cutter::talk(rate);
        c.feed(&room(0.5));
        assert!(c.rest().is_none(), "nothing said");
    }

    /// Push to talk end to end, with a 16 kHz mono 16-bit WAV in NIMBUS_TALK_WAV and
    /// Lemonade's Whisper-Small: cut at the pause, then written down.
    #[test]
    #[ignore]
    fn live_talk() {
        let raw = std::fs::read(std::env::var("NIMBUS_TALK_WAV").unwrap()).unwrap();
        let data = raw.windows(4).position(|w| w == b"data").unwrap() + 8;
        let said: Vec<f32> = raw[data..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let room = |secs: f32| (0..(16_000.0 * secs) as usize).map(|i| ((i * 7919 % 13) as f32 / 13.0 - 0.5) * 0.001).collect::<Vec<f32>>();
        let mut sound = room(0.7);
        sound.extend(&said);
        sound.extend(room(2.0));
        let mut c = Cutter::talk(16_000);
        let pieces: Vec<Vec<f32>> = sound.chunks(160).flat_map(|b| c.feed(b)).collect();
        assert_eq!(pieces.len(), 1, "one request");
        let began = std::time::Instant::now();
        let text = transcribe("http://localhost:13305/api/v1", "Whisper-Small", wav(&resample(&pieces[0], 16_000)), None, Some("en")).unwrap();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        println!("heard: {text:?} in {:?}", began.elapsed());
        assert!(text.to_lowercase().contains("test mail"));
    }

    /// Needs Lemonade with Whisper loaded, and a 48 kHz mono 16-bit WAV of someone saying
    /// "Hey Nimbus, what time is it?" in NIMBUS_VOICE_WAV. Fed in small blocks, as the
    /// microphone gives it, between stretches of room noise.
    #[test]
    #[ignore]
    fn live_wake_and_request() {
        let raw = std::fs::read(std::env::var("NIMBUS_VOICE_WAV").unwrap()).unwrap();
        let data = raw.windows(4).position(|w| w == b"data").unwrap() + 8;
        let said: Vec<f32> = raw[data..].chunks(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect();
        let room = |secs: f32| (0..(48_000.0 * secs) as usize).map(|i| ((i * 7919 % 13) as f32 / 13.0 - 0.5) * 0.001).collect::<Vec<f32>>();
        let mut sound = room(1.0);
        sound.extend(&said);
        sound.extend(room(1.5));
        let mut c = Cutter::new(48_000);
        let pieces: Vec<Vec<f32>> = sound.chunks(480).flat_map(|b| c.feed(b)).collect();
        assert_eq!(pieces.len(), 1, "one stretch of speech");
        let began = std::time::Instant::now();
        let text = transcribe("http://localhost:13305/api/v1", "Whisper-Large-v3-Turbo", wav(&resample(&pieces[0], 48_000)), Some(NAME_HINT), Some("en")).unwrap();
        println!("heard: {text:?} in {:?}; request: {:?}", began.elapsed(), after_wake(&text));
        assert!(after_wake(&text).is_some_and(|r| r.to_lowercase().contains("time")));
    }
}
