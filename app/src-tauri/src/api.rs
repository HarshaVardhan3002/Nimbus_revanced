//! A small local API for Nimbus's plugins: a plugin can pass Nimbus a request (as if the
//! person had typed it) and show a notice in the notch, with buttons.
//!
//! It listens on this computer only, and every call needs a token made fresh at each
//! start. The token is never written anywhere: Nimbus hands it to the plugins it starts
//! (in NIMBUS_API and NIMBUS_TOKEN), so other programs cannot use the API.
//!
//!   POST /ask     { "text": "...", "from": "WhatsApp" }   -> { "answer": "..." }
//!   POST /notice  { "text": "...", "buttons": ["OK"] }    -> { "id": "..." }
//!   GET  /notice/<id>                                       -> { "pressed": "OK" | null }
//!
//! Requests through /ask run like typed ones: every step that asks first still asks, in
//! the chat, and no answer counts as no.

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// The address and token, once listening.
static ADDRESS: OnceLock<(String, String)> = OnceLock::new();
/// Notices shown, and the button pressed on each.
static PRESSED: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

/// How long a request through /ask may run.
const ASK_LIMIT: Duration = Duration::from_secs(15 * 60);
const MAX_BODY: usize = 64 * 1024;

#[derive(Clone, Serialize)]
pub struct Notice {
    pub id: String,
    /// Who shows it: the plugin's name.
    pub from: String,
    pub text: String,
    pub buttons: Vec<String>,
}

/// The API's address and token, for the plugins Nimbus starts.
pub fn address() -> Option<&'static (String, String)> {
    ADDRESS.get()
}

fn pressed() -> &'static Mutex<HashMap<String, Option<String>>> {
    PRESSED.get_or_init(Default::default)
}

/// The person pressed a notice's button.
pub fn answer(id: &str, button: &str) {
    if let Some(slot) = pressed().lock().get_mut(id) {
        *slot = Some(button.to_string());
    }
}

fn token() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut b = [0u8; 24];
    SystemRandom::new().fill(&mut b).expect("random");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn spawn(app: AppHandle) {
    let Ok(listener) = TcpListener::bind("127.0.0.1:0") else {
        eprintln!("[nimbus] api: could not listen");
        return;
    };
    let Ok(addr) = listener.local_addr() else { return };
    let _ = ADDRESS.set((format!("http://{addr}"), token()));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(e) = serve(&app, stream) {
                    eprintln!("[nimbus] api: {e}");
                }
            });
        }
    });
}

fn serve(app: &AppHandle, stream: TcpStream) -> Result<(), String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    let mut parts = line.split_whitespace();
    let (method, path) = (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
    let (mut length, mut auth) = (0usize, String::new());
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).map_err(|e| e.to_string())? == 0 || h.trim().is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or_default();
        match k.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = v.trim().parse().unwrap_or(0),
            "authorization" => auth = v.trim().to_string(),
            _ => {}
        }
    }
    let expected = ADDRESS.get().map(|(_, t)| format!("Bearer {t}")).unwrap_or_default();
    let (status, reply) = if expected.is_empty() || !same(auth.as_bytes(), expected.as_bytes()) {
        (401, json!({ "error": "no or wrong token" }))
    } else if length > MAX_BODY {
        (413, json!({ "error": "too long" }))
    } else {
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).map_err(|e| e.to_string())?;
        let body: Value = if body.is_empty() { Value::Null } else { serde_json::from_slice(&body).unwrap_or(Value::Null) };
        route(app, &method, &path, &body)
    };
    let text = reply.to_string();
    let mut out = stream;
    write!(
        out,
        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        if status == 200 { "OK" } else { "Error" },
        text.len()
    )
    .map_err(|e| e.to_string())?;
    // Ended for the caller even when a program started meanwhile (the model server, on a
    // request that woke it) inherited a copy of this connection and keeps it open.
    let _ = out.flush();
    let _ = out.shutdown(std::net::Shutdown::Both);
    Ok(())
}

/// Compares in the same time whatever the text, so the token cannot be guessed by timing.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn route(app: &AppHandle, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let s = |k: &str| body.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    match (method, path) {
        ("POST", "/ask") => {
            let (text, from) = (s("text"), s("from"));
            if text.is_empty() {
                return (400, json!({ "error": "no text" }));
            }
            let from = if from.is_empty() { "a plugin".to_string() } else { from };
            match ask(app, &text, &from) {
                Ok(answer) => (200, json!({ "answer": answer })),
                Err(e) => (409, json!({ "error": e })),
            }
        }
        ("POST", "/notice") => {
            let text = s("text");
            if text.is_empty() {
                return (400, json!({ "error": "no text" }));
            }
            let buttons: Vec<String> = body["buttons"].as_array().map(|b| b.iter().filter_map(Value::as_str).take(3).map(String::from).collect()).unwrap_or_default();
            let id = crate::routines::new_id();
            pressed().lock().insert(id.clone(), None);
            let from = if s("from").is_empty() { "Plugin".to_string() } else { s("from") };
            let _ = app.emit("notice", Notice { id: id.clone(), from, text, buttons });
            (200, json!({ "id": id }))
        }
        ("GET", p) if p.starts_with("/notice/") => {
            let id = &p["/notice/".len()..];
            match pressed().lock().get(id) {
                Some(b) => (200, json!({ "pressed": b })),
                None => (404, json!({ "error": "no such notice" })),
            }
        }
        _ => (404, json!({ "error": "not here" })),
    }
}

/// Runs a request the way a typed one runs, in a conversation of its own per sender, and
/// waits for the answer.
fn ask(app: &AppHandle, text: &str, from: &str) -> Result<String, String> {
    let key: String = from.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let (tx, rx) = std::sync::mpsc::channel();
    let prompt = format!("(The person wrote this through {from}, not in the chat; your answer goes back there.) {text}");
    crate::agent::start(app, &format!("api:{key}"), &prompt, Some(format!("From {from}")), false, false, move |_, r| {
        let _ = tx.send(r);
    })?;
    rx.recv_timeout(ASK_LIMIT).map_err(|_| "no answer in time".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_tokens_whole() {
        assert!(same(b"Bearer abc", b"Bearer abc"));
        assert!(!same(b"Bearer abd", b"Bearer abc"));
        assert!(!same(b"Bearer ab", b"Bearer abc"));
        assert!(!same(b"", b"Bearer abc"));
    }
}
