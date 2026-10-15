//! The agent's hands: mouse and keyboard, on the main screen.
//!
//! While the agent is in control a see-through overlay covers the screen: a soft
//! glowing border and the agent's own cursor, which glides to each spot before the
//! click lands, so it is always plain who is moving things. Escape stops the agent at
//! once. The overlay is hidden from capture, so the agent never sees it in a frame.

use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};

pub const OVERLAY: &str = "overlay";
/// How long the cursor takes to glide to a spot before acting there.
const GLIDE: Duration = Duration::from_millis(420);
/// After the last action, the overlay waits this long before fading out.
const LINGER: Duration = Duration::from_millis(900);

#[derive(Default)]
pub struct Control {
    /// Runs that have taken the mouse and keyboard.
    holders: AtomicUsize,
    last: Mutex<Option<Instant>>,
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Show {
    /// The border comes up (or goes down).
    Control { on: bool },
    /// The cursor moves to a spot, in screen pixels, and does something there.
    Cursor { x: i32, y: i32, action: String, label: Option<String> },
}

pub fn escape() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

/// The overlay window, made once when Nimbus starts and shown only while in control.
pub fn create_overlay(app: &AppHandle) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("overlay.html".into()))
        .title("Nimbus is in control")
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    let _ = win.set_ignore_cursor_events(true);
    cover_main_screen(&win);
    Ok(())
}

fn cover_main_screen(win: &tauri::WebviewWindow) {
    if let Ok(Some(m)) = win.primary_monitor() {
        let _ = win.set_position(*m.position());
        let _ = win.set_size(*m.size());
    }
}

/// The agent takes the mouse and keyboard: the overlay comes up and Escape stops it.
pub fn take(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    *state.control.last.lock() = Some(Instant::now());
    if state.control.holders.fetch_add(1, Ordering::SeqCst) > 0 {
        return;
    }
    if let Some(win) = app.get_webview_window(OVERLAY) {
        cover_main_screen(&win);
        let _ = win.show();
        let _ = win.set_ignore_cursor_events(true);
        crate::privacy::apply(&win, true);
    }
    let _ = app.emit_to(OVERLAY, "overlay", Show::Control { on: true });
    let _ = app.emit("agent-control", true);
    if let Err(e) = app.global_shortcut().register(escape()) {
        eprintln!("[nimbus] Escape could not be taken to stop the agent: {e}");
    }
}

/// The run is done with the mouse and keyboard. The overlay fades once no run holds them.
pub fn give_back(app: &AppHandle) {
    let state = app.state::<crate::AppState>();
    let left = state.control.holders.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
    if left > 0 {
        return;
    }
    let _ = app.global_shortcut().unregister(escape());
    let _ = app.emit_to(OVERLAY, "overlay", Show::Control { on: false });
    let _ = app.emit("agent-control", false);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(LINGER);
        let state = app.state::<crate::AppState>();
        if state.control.holders.load(Ordering::SeqCst) == 0 {
            if let Some(win) = app.get_webview_window(OVERLAY) {
                let _ = win.hide();
            }
        }
    });
}

fn key_named(name: &str) -> Result<Key, String> {
    let n = name.trim().to_lowercase();
    Ok(match n.as_str() {
        "ctrl" | "control" => Key::Control,
        "alt" | "option" => Key::Alt,
        "shift" => Key::Shift,
        "win" | "windows" | "meta" | "cmd" | "command" | "super" => Key::Meta,
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "esc" | "escape" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        s if s.chars().count() == 1 => Key::Unicode(s.chars().next().unwrap_or(' ')),
        other => return Err(format!("there is no key called \"{other}\"")),
    })
}

/// Press a combination like "ctrl+shift+t": modifiers down, the key, all up again.
fn press(enigo: &mut Enigo, combo: &str) -> Result<(), String> {
    let keys = combo.split('+').map(key_named).collect::<Result<Vec<_>, _>>()?;
    let Some((last, mods)) = keys.split_last() else { return Err("no key given".into()) };
    for m in mods {
        enigo.key(*m, Direction::Press).map_err(|e| e.to_string())?;
    }
    let r = enigo.key(*last, Direction::Click).map_err(|e| e.to_string());
    for m in mods.iter().rev() {
        let _ = enigo.key(*m, Direction::Release);
    }
    r
}

fn point(args: &Value, x: &str, y: &str) -> Option<(f64, f64)> {
    Some((args.get(x)?.as_f64()?, args.get(y)?.as_f64()?))
}

/// Carry out one action. `to_screen` turns the model's coordinates into screen pixels
/// (they come from a screenshot, or straight from window positions).
pub fn perform(app: &AppHandle, args: &Value, to_screen: &dyn Fn(f64, f64) -> (i32, i32), label: Option<String>) -> Result<String, String> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or_default();
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("the mouse and keyboard could not be reached: {e}"))?;
    *app.state::<crate::AppState>().control.last.lock() = Some(Instant::now());

    let show_at = |x: i32, y: i32, what: &str| {
        let _ = app.emit_to(OVERLAY, "overlay", Show::Cursor { x, y, action: what.into(), label: label.clone() });
        std::thread::sleep(GLIDE);
    };
    let at = |enigo: &mut Enigo, what: &str| -> Result<(i32, i32), String> {
        let (x, y) = point(args, "x", "y").ok_or("this action needs x and y")?;
        let (sx, sy) = to_screen(x, y);
        show_at(sx, sy, what);
        enigo.move_mouse(sx, sy, Coordinate::Abs).map_err(|e| e.to_string())?;
        Ok((sx, sy))
    };

    let done = match action {
        "move" => {
            let (x, y) = at(&mut enigo, "move")?;
            format!("Moved the pointer to {x}, {y}.")
        }
        "click" | "double_click" | "right_click" | "middle_click" => {
            let (x, y) = at(&mut enigo, action)?;
            let button = match action {
                "right_click" => Button::Right,
                "middle_click" => Button::Middle,
                _ => Button::Left,
            };
            enigo.button(button, Direction::Click).map_err(|e| e.to_string())?;
            if action == "double_click" {
                std::thread::sleep(Duration::from_millis(60));
                enigo.button(button, Direction::Click).map_err(|e| e.to_string())?;
            }
            let did = match action {
                "double_click" => "Double-clicked",
                "right_click" => "Right-clicked",
                "middle_click" => "Middle-clicked",
                _ => "Clicked",
            };
            format!("{did} at {x}, {y}.")
        }
        "drag" => {
            let (fx, fy) = at(&mut enigo, "drag")?;
            let (tx, ty) = point(args, "to_x", "to_y").ok_or("a drag needs to_x and to_y")?;
            let (tx, ty) = to_screen(tx, ty);
            enigo.button(Button::Left, Direction::Press).map_err(|e| e.to_string())?;
            show_at(tx, ty, "drag");
            let r = enigo.move_mouse(tx, ty, Coordinate::Abs).map_err(|e| e.to_string());
            let _ = enigo.button(Button::Left, Direction::Release);
            r?;
            format!("Dragged from {fx}, {fy} to {tx}, {ty}.")
        }
        "scroll" => {
            if point(args, "x", "y").is_some() {
                at(&mut enigo, "scroll")?;
            }
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(3).clamp(-30, 30) as i32;
            enigo.scroll(amount, Axis::Vertical).map_err(|e| e.to_string())?;
            format!("Scrolled {} by {}.", if amount >= 0 { "down" } else { "up" }, amount.abs())
        }
        "type" => {
            let text = args.get("text").and_then(Value::as_str).ok_or("type needs text")?;
            if point(args, "x", "y").is_some() {
                at(&mut enigo, "click")?;
                enigo.button(Button::Left, Direction::Click).map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_millis(120));
            }
            enigo.text(text).map_err(|e| e.to_string())?;
            format!("Typed {} characters.", text.chars().count())
        }
        "key" => {
            let keys = args.get("keys").and_then(Value::as_str).ok_or("key needs keys, like ctrl+c")?;
            press(&mut enigo, keys)?;
            format!("Pressed {keys}.")
        }
        "wait" => {
            let s = args.get("seconds").and_then(Value::as_f64).unwrap_or(1.0).clamp(0.2, 10.0);
            std::thread::sleep(Duration::from_secs_f64(s));
            format!("Waited {s:.1} s.")
        }
        other => return Err(format!("there is no action called \"{other}\"")),
    };
    // Let the screen catch up before anyone looks at it.
    std::thread::sleep(Duration::from_millis(350));
    Ok(done)
}
