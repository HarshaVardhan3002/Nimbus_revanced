//! To-dos, reminders, alarms and routines: everything that happens at a time.
//!
//! The Today page and the agent share one list. A to-do may have a time, and then it
//! is a reminder: the notch speaks up when it is due. An alarm rings until dismissed. A
//! task wakes the agent to do something ("at 8, brief me on my mail").
//!
//! Routines are saved step-by-step instructions the agent carries out on its own, by
//! hand or at set times. A routine saved as trusted has the person's OK in advance for
//! the steps that would otherwise ask; the blocked kind stays blocked (see policy.rs).
//!
//! Both lists live in the app's data folder; a scheduler thread checks them every
//! couple of seconds.

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Never,
    Daily,
    Weekdays,
    Weekly,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Something to do; with a time it reminds.
    #[default]
    Todo,
    /// Rings until dismissed.
    Alarm,
    /// Wakes the agent to carry out `task`.
    Task,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    /// When it is due, in milliseconds since 1970 (UTC).
    #[serde(default)]
    pub at: Option<i64>,
    #[serde(default)]
    pub repeat: Repeat,
    #[serde(default)]
    pub kind: Kind,
    /// For a task: what the agent should do when it wakes.
    #[serde(default)]
    pub task: Option<String>,
    /// Its time has come and it has spoken up (until it is moved to a new time).
    #[serde(default)]
    pub fired: bool,
    /// Made by the agent rather than typed in.
    #[serde(default)]
    pub by_agent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "when", rename_all = "snake_case")]
pub enum Trigger {
    /// Only when started by hand (or by the agent).
    Manual,
    /// Every day, on weekdays, or on one day a week, at a local time "HH:MM".
    Daily { time: String },
    Weekdays { time: String },
    Weekly { day: u8, time: String },
    /// Each time Nimbus starts.
    Startup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    pub name: String,
    /// What to do and how, in the person's own words, step by step.
    pub steps: String,
    pub trigger: Trigger,
    /// Steps that would ask go ahead: the person gave their OK when saving it.
    #[serde(default)]
    pub trusted: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub last_run: Option<i64>,
    /// The last run's outcome in a sentence or two.
    #[serde(default)]
    pub last_result: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    todos: Vec<Todo>,
    #[serde(default)]
    routines: Vec<Routine>,
}

#[derive(Default)]
pub struct Book {
    saved: Mutex<Saved>,
    file: Mutex<Option<PathBuf>>,
}

/// Something due, for the notch to say (and, for tasks, for the agent to do).
#[derive(Debug, Clone, Serialize)]
pub struct Due {
    pub id: String,
    pub text: String,
    pub kind: Kind,
}

pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{:x}{:x}", now_ms(), N.fetch_add(1, Ordering::Relaxed))
}

impl Book {
    pub fn open(app: &AppHandle) -> Self {
        let file = app.path().app_data_dir().ok().map(|d| d.join("routines.json"));
        let mut saved: Saved = file
            .as_ref()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // The to-dos the Today page kept before this list existed.
        if saved.todos.is_empty() {
            if let Ok(Some(old)) = crate::store::get(app, "todos") {
                if let Ok(old) = serde_json::from_value::<Vec<Todo>>(old) {
                    saved.todos = old;
                }
            }
        }
        Self { saved: Mutex::new(saved), file: Mutex::new(file) }
    }

    fn save(&self, app: &AppHandle) {
        let saved = self.saved.lock();
        if let Some(f) = self.file.lock().as_ref() {
            if let Ok(json) = serde_json::to_vec_pretty(&*saved) {
                if let Err(e) = crate::store::write_atomic(f, &json) {
                    eprintln!("[nimbus] routines not saved: {e}");
                }
            }
        }
        let _ = app.emit("todos", saved.todos.clone());
        let _ = app.emit("routines", saved.routines.clone());
    }

    pub fn todos(&self) -> Vec<Todo> {
        self.saved.lock().todos.clone()
    }

    pub fn routines(&self) -> Vec<Routine> {
        self.saved.lock().routines.clone()
    }

    pub fn add_todo(&self, app: &AppHandle, mut t: Todo) -> Todo {
        if t.id.is_empty() {
            t.id = new_id();
        }
        t.fired = false;
        self.saved.lock().todos.push(t.clone());
        self.save(app);
        t
    }

    /// Change a to-do in place; `None` when there is no such to-do.
    pub fn edit_todo(&self, app: &AppHandle, id: &str, change: impl FnOnce(&mut Todo)) -> Option<Todo> {
        let out = {
            let mut s = self.saved.lock();
            let t = s.todos.iter_mut().find(|t| t.id == id)?;
            let before = t.at;
            change(t);
            if t.at != before {
                t.fired = false;
            }
            t.clone()
        };
        self.save(app);
        Some(out)
    }

    pub fn remove_todo(&self, app: &AppHandle, id: &str) -> bool {
        let gone = {
            let mut s = self.saved.lock();
            let n = s.todos.len();
            s.todos.retain(|t| t.id != id);
            s.todos.len() != n
        };
        if gone {
            self.save(app);
        }
        gone
    }

    /// Say it again in a few minutes. A repeating one keeps its series and gets a
    /// one-off copy for the snooze.
    pub fn snooze(&self, app: &AppHandle, id: &str, minutes: i64) -> Option<Todo> {
        let at = now_ms() + minutes * 60_000;
        let t = self.todos().into_iter().find(|t| t.id == id)?;
        if t.repeat == Repeat::Never {
            return self.edit_todo(app, id, |t| t.at = Some(at));
        }
        Some(self.add_todo(app, Todo { id: String::new(), at: Some(at), repeat: Repeat::Never, ..t }))
    }

    pub fn clear_done(&self, app: &AppHandle) {
        self.saved.lock().todos.retain(|t| !t.done);
        self.save(app);
    }

    pub fn put_routine(&self, app: &AppHandle, mut r: Routine) -> Routine {
        if r.id.is_empty() {
            r.id = new_id();
        }
        {
            let mut s = self.saved.lock();
            match s.routines.iter_mut().find(|x| x.id == r.id) {
                Some(x) => {
                    // The record of past runs is the scheduler's, not the editor's.
                    r.last_run = x.last_run;
                    r.last_result = x.last_result.clone();
                    *x = r.clone();
                }
                None => s.routines.push(r.clone()),
            }
        }
        self.save(app);
        r
    }

    pub fn remove_routine(&self, app: &AppHandle, id: &str) {
        self.saved.lock().routines.retain(|r| r.id != id);
        self.save(app);
    }

    pub fn routine(&self, id: &str) -> Option<Routine> {
        self.saved.lock().routines.iter().find(|r| r.id == id).cloned()
    }

    pub fn finished(&self, app: &AppHandle, id: &str, result: String) {
        if let Some(r) = self.saved.lock().routines.iter_mut().find(|r| r.id == id) {
            r.last_result = Some(result);
        }
        self.save(app);
    }

    /// To-dos and routines due now. Moves repeating ones on to their next time.
    fn due(&self, now: DateTime<Local>, started: bool) -> (Vec<Due>, Vec<Routine>) {
        let mut s = self.saved.lock();
        let ms = now.timestamp_millis();
        let mut out = Vec::new();
        for t in s.todos.iter_mut() {
            let Some(at) = t.at else { continue };
            if t.done || t.fired || at > ms {
                continue;
            }
            // Far past (the computer was off): say it once, without a ring.
            out.push(Due { id: t.id.clone(), text: t.text.clone(), kind: if ms - at > 15 * 60_000 && t.kind == Kind::Alarm { Kind::Todo } else { t.kind } });
            match next_after(at, t.repeat, ms) {
                Some(next) => t.at = Some(next),
                None => t.fired = true,
            }
        }
        let mut routines = Vec::new();
        for r in s.routines.iter_mut().filter(|r| r.enabled) {
            let fire = match &r.trigger {
                Trigger::Manual => false,
                Trigger::Startup => started,
                Trigger::Daily { time } => slot_due(now, time, r.last_run, |_| true),
                Trigger::Weekdays { time } => slot_due(now, time, r.last_run, |d| !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)),
                Trigger::Weekly { day, time } => slot_due(now, time, r.last_run, |d| d.weekday().num_days_from_monday() == u32::from(*day) % 7),
            };
            if fire {
                r.last_run = Some(ms);
                routines.push(r.clone());
            }
        }
        (out, routines)
    }

    pub fn mark_run(&self, id: &str) {
        if let Some(r) = self.saved.lock().routines.iter_mut().find(|r| r.id == id) {
            r.last_run = Some(now_ms());
        }
    }
}

/// Today's time slot for a routine has come, and it has not run since.
fn slot_due(now: DateTime<Local>, time: &str, last: Option<i64>, day_ok: impl Fn(NaiveDate) -> bool) -> bool {
    let Some(t) = parse_hm(time) else { return false };
    let today = now.date_naive();
    if !day_ok(today) {
        return false;
    }
    let Some(slot) = Local.from_local_datetime(&today.and_time(t)).earliest() else { return false };
    // Within an hour of the slot: a computer woken at noon does not run the 8 o'clock briefing.
    let late = now.signed_duration_since(slot);
    late.num_seconds() >= 0 && late.num_minutes() < 60 && last.is_none_or(|l| l < slot.timestamp_millis())
}

fn parse_hm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok().or_else(|| NaiveTime::parse_from_str(s.trim(), "%H:%M:%S").ok())
}

/// The next time a repeating to-do comes up after `now`.
fn next_after(at: i64, repeat: Repeat, now: i64) -> Option<i64> {
    let step_days = match repeat {
        Repeat::Never => return None,
        Repeat::Daily | Repeat::Weekdays => 1,
        Repeat::Weekly => 7,
    };
    let mut t = Local.timestamp_millis_opt(at).single()?;
    loop {
        let naive = t.naive_local() + chrono::Duration::days(step_days);
        t = Local.from_local_datetime(&naive).earliest()?;
        let weekend = matches!(t.weekday(), Weekday::Sat | Weekday::Sun);
        if t.timestamp_millis() > now && !(repeat == Repeat::Weekdays && weekend) {
            return Some(t.timestamp_millis());
        }
    }
}

/// A time the agent or the person wrote: "2026-10-14 18:30", "18:30" (the next one),
/// "in 20 minutes", "in 2 hours", "tomorrow 07:00", or an RFC 3339 time.
pub fn parse_when(s: &str, now: DateTime<Local>) -> Result<i64, String> {
    let s = s.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix("in ") {
        let mut parts = rest.split_whitespace();
        let n: f64 = parts.next().and_then(|n| n.parse().ok()).ok_or("say how many, like \"in 20 minutes\"")?;
        let unit = parts.next().unwrap_or("minutes");
        let secs = match unit.trim_end_matches('s') {
            "second" | "sec" => n,
            "minute" | "min" => n * 60.0,
            "hour" | "hr" | "h" => n * 3600.0,
            "day" => n * 86400.0,
            "week" => n * 7.0 * 86400.0,
            other => return Err(format!("\"{other}\" is not a unit of time I know")),
        };
        return Ok(now.timestamp_millis() + (secs * 1000.0) as i64);
    }
    if let Ok(t) = DateTime::parse_from_rfc3339(&s.to_uppercase()) {
        return Ok(t.timestamp_millis());
    }
    for f in ["%Y-%m-%d %H:%M", "%Y-%m-%dt%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dt%H:%M:%S"] {
        if let Ok(n) = NaiveDateTime::parse_from_str(&s, f) {
            return Local.from_local_datetime(&n).earliest().map(|t| t.timestamp_millis()).ok_or_else(|| "that time does not exist here".into());
        }
    }
    let (day, time) = match s.strip_prefix("tomorrow") {
        Some(rest) => (now.date_naive() + chrono::Duration::days(1), rest.trim().trim_start_matches("at").trim()),
        None => (now.date_naive(), s.strip_prefix("today").unwrap_or(&s).trim().trim_start_matches("at").trim()),
    };
    let t = parse_hm(time).or_else(|| parse_12h(time)).ok_or_else(|| format!("\"{s}\" is not a time I can read; use \"YYYY-MM-DD HH:MM\" or \"in 20 minutes\""))?;
    let mut when = Local.from_local_datetime(&day.and_time(t)).earliest().ok_or("that time does not exist here")?;
    // A bare time already past today means the next one.
    if !s.starts_with("today") && !s.starts_with("tomorrow") && when <= now {
        when = Local.from_local_datetime(&(day + chrono::Duration::days(1)).and_time(t)).earliest().ok_or("that time does not exist here")?;
    }
    Ok(when.timestamp_millis())
}

/// "7pm", "7:30 am", "12am".
fn parse_12h(s: &str) -> Option<NaiveTime> {
    let s = s.replace([' ', '.'], "");
    let (num, pm) = match () {
        _ if s.ends_with("pm") => (&s[..s.len() - 2], true),
        _ if s.ends_with("am") => (&s[..s.len() - 2], false),
        _ => return None,
    };
    let (h, m) = num.split_once(':').unwrap_or((num, "0"));
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if !(1..=12).contains(&h) {
        return None;
    }
    NaiveTime::from_hms_opt(h % 12 + if pm { 12 } else { 0 }, m, 0)
}

pub fn describe_time(ms: i64) -> String {
    Local.timestamp_millis_opt(ms).single().map(|t| t.format("%a %d %b %Y, %H:%M").to_string()).unwrap_or_default()
}

/// The scheduler: every two seconds, says what is due and starts what should run.
pub fn spawn(app: AppHandle) {
    std::thread::spawn(move || {
        let mut started = true;
        loop {
            let book = &app.state::<crate::AppState>().routines;
            let (due, routines) = book.due(Local::now(), started);
            started = false;
            if !due.is_empty() || !routines.is_empty() {
                book.save(&app);
            }
            for d in due {
                if d.kind == Kind::Task {
                    let task = book.todos().into_iter().find(|t| t.id == d.id).and_then(|t| t.task).unwrap_or_else(|| d.text.clone());
                    crate::agent::run_background(&app, &format!("task:{}", d.id), &d.text, &task, false);
                }
                let _ = app.emit("reminder", d);
            }
            for r in routines {
                crate::agent::run_routine(&app, &r);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest().unwrap()
    }

    #[test]
    fn reads_times() {
        let now = at(2026, 10, 14, 9, 0);
        assert_eq!(parse_when("in 20 minutes", now).unwrap(), now.timestamp_millis() + 20 * 60_000);
        assert_eq!(parse_when("18:30", now).unwrap(), at(2026, 10, 14, 18, 30).timestamp_millis());
        assert_eq!(parse_when("08:00", now).unwrap(), at(2026, 10, 15, 8, 0).timestamp_millis());
        assert_eq!(parse_when("tomorrow 7:15", now).unwrap(), at(2026, 10, 15, 7, 15).timestamp_millis());
        assert_eq!(parse_when("2026-12-24 18:00", now).unwrap(), at(2026, 12, 24, 18, 0).timestamp_millis());
        assert_eq!(parse_when("7pm", now).unwrap(), at(2026, 10, 14, 19, 0).timestamp_millis());
        assert!(parse_when("whenever", now).is_err());
    }

    #[test]
    fn repeats_skip_weekends() {
        // Friday 2026-10-16 08:00, weekdays: next is Monday the 19th.
        let fri = at(2026, 10, 16, 8, 0).timestamp_millis();
        assert_eq!(next_after(fri, Repeat::Weekdays, fri), Some(at(2026, 10, 19, 8, 0).timestamp_millis()));
        assert_eq!(next_after(fri, Repeat::Weekly, fri), Some(at(2026, 10, 23, 8, 0).timestamp_millis()));
        assert_eq!(next_after(fri, Repeat::Never, fri), None);
    }

    #[test]
    fn routine_slots() {
        let now = at(2026, 10, 14, 8, 5);
        assert!(slot_due(now, "08:00", None, |_| true));
        assert!(!slot_due(now, "08:00", Some(at(2026, 10, 14, 8, 1).timestamp_millis()), |_| true));
        assert!(!slot_due(now, "09:00", None, |_| true));
        assert!(!slot_due(at(2026, 10, 14, 10, 0), "08:00", None, |_| true));
    }
}
