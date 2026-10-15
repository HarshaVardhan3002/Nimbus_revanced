//! One call to a model over the OpenAI-style API, streamed: words go out as they come,
//! and any tools the model asks for are gathered up for the harness to run.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

/// How often a reply checks for Stop, and how long it may go quiet mid-reply.
const TICK: Duration = Duration::from_millis(150);
const STALL: Duration = Duration::from_secs(90);

pub struct Endpoint<'a> {
    pub url: &'a str,
    pub key: &'a str,
    pub model: &'a str,
}

#[derive(Debug, Clone, Default)]
pub struct Call {
    pub id: String,
    pub name: String,
    pub args: String,
}

#[derive(Debug, Default)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<Call>,
    /// Stopped by the user part-way.
    pub stopped: bool,
}

/// Why a call failed, and whether it was the tools the server objected to.
pub struct Failure {
    pub message: String,
    pub no_tools: bool,
    /// The server answered and turned the request down (not a lost connection or a
    /// timeout): something in what was sent, like a picture, is not taken.
    pub refused: bool,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self { message, no_tools: false, refused: false }
    }
}

fn agent(answer_within: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(2000)))
        .timeout_recv_response(Some(answer_within))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Whether the model takes pictures, for servers that say so (llama.cpp's `/props`);
/// `None` when the server does not tell.
pub fn sees_pictures(at: &Endpoint) -> Option<bool> {
    let base = at.url.trim_end_matches('/').trim_end_matches("/v1");
    let mut r = agent(Duration::from_secs(3)).get(&format!("{base}/props")).call().ok()?;
    if r.status() != 200 {
        return None;
    }
    let v: Value = serde_json::from_str(&r.body_mut().read_to_string().ok()?).ok()?;
    v.pointer("/modalities/vision").and_then(Value::as_bool)
}

fn server_error(status: u16, body: &str) -> String {
    let said = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        v.pointer("/error/message")
            .or_else(|| v.get("error"))
            .or_else(|| v.get("detail"))
            .and_then(|m| m.as_str().map(str::to_string))
    });
    match (status, said) {
        (401 | 403, _) => "the server turned the key down".into(),
        (_, Some(m)) if !m.is_empty() => m,
        (s, _) => format!("the server answered with error {s}"),
    }
}

/// Stream one reply. `tools` empty means plain chat.
pub fn complete(
    at: &Endpoint,
    messages: &[Value],
    tools: &[Value],
    stop: &AtomicBool,
    on_text: &mut dyn FnMut(&str),
) -> Result<Reply, Failure> {
    let mut body = json!({ "model": at.model, "stream": true, "messages": messages });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
        body["tool_choice"] = json!("auto");
    }
    // A local server may load the model first: allow it a few minutes to start answering.
    let mut req = agent(Duration::from_secs(240))
        .post(format!("{}/chat/completions", at.url.trim_end_matches('/')))
        .header("Content-Type", "application/json");
    if !at.key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", at.key));
    }
    let res = req.send(body.to_string()).map_err(|e| match e {
        ureq::Error::ConnectionFailed | ureq::Error::Io(_) | ureq::Error::HostNotFound => "the server stopped answering".to_string(),
        ureq::Error::Timeout(_) => "the model took too long to start".to_string(),
        e => e.to_string(),
    })?;
    let status = res.status().as_u16();
    let streaming = res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.contains("event-stream"));
    let mut body = res.into_body();
    if status >= 400 {
        let message = server_error(status, &body.read_to_string().unwrap_or_default());
        // Servers without tool support say so in many ways; all mention tools.
        let no_tools = !tools.is_empty() && message.to_lowercase().contains("tool");
        return Err(Failure { message, no_tools, refused: true });
    }

    // Some servers ignore "stream" and send the whole reply at once.
    if !streaming {
        let text = body.read_to_string().map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&text).map_err(|_| "the reply was not readable".to_string())?;
        let msg = v.pointer("/choices/0/message").cloned().unwrap_or(Value::Null);
        let content = msg.get("content").and_then(Value::as_str).unwrap_or_default().to_string();
        on_text(&content);
        let calls = msg
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(|calls| {
                calls
                    .iter()
                    .map(|c| Call {
                        id: c.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or_default().to_string(),
                        args: match c.pointer("/function/arguments") {
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                            None => "{}".into(),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Reply { text: content, calls, stopped: false });
    }

    // Read on a side thread, so Stop answers at once and a server that goes quiet
    // mid-reply cannot hold the harness forever.
    let (tx, rx) = channel();
    let reader = body.into_reader();
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let failed = line.is_err();
            if tx.send(line).is_err() || failed {
                break;
            }
        }
    });

    let mut reply = Reply::default();
    let mut quiet = Duration::ZERO;
    loop {
        if stop.load(Ordering::Relaxed) {
            reply.stopped = true;
            break;
        }
        let line = match rx.recv_timeout(TICK) {
            Ok(line) => line,
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {
                quiet += TICK;
                if quiet >= STALL {
                    return Err("it stopped sending words".to_string().into());
                }
                continue;
            }
        };
        quiet = Duration::ZERO;
        let line = line.map_err(|_| "the reply was cut off".to_string())?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data == "[DONE]" {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
        if let Some(e) = v.get("error") {
            let message = e.get("message").and_then(Value::as_str).unwrap_or("the model stopped with an error").to_string();
            return Err(Failure { no_tools: !tools.is_empty() && message.to_lowercase().contains("tool"), message, refused: true });
        }
        let Some(delta) = v.pointer("/choices/0/delta") else { continue };
        if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            reply.text.push_str(text);
            on_text(text);
        }
        // Tool calls arrive in pieces, keyed by their index.
        if let Some(parts) = delta.get("tool_calls").and_then(Value::as_array) {
            for part in parts {
                let i = part.get("index").and_then(Value::as_u64).unwrap_or(reply.calls.len().saturating_sub(1) as u64) as usize;
                while reply.calls.len() <= i {
                    reply.calls.push(Call::default());
                }
                let call = &mut reply.calls[i];
                if let Some(id) = part.get("id").and_then(Value::as_str) {
                    call.id = id.to_string();
                }
                if let Some(name) = part.pointer("/function/name").and_then(Value::as_str) {
                    call.name.push_str(name);
                }
                match part.pointer("/function/arguments") {
                    Some(Value::String(s)) => call.args.push_str(s),
                    Some(v @ Value::Object(_)) => call.args = v.to_string(),
                    _ => {}
                }
            }
        }
    }
    reply.calls.retain(|c| !c.name.is_empty());
    for (i, c) in reply.calls.iter_mut().enumerate() {
        if c.id.is_empty() {
            c.id = format!("call_{i}");
        }
        if c.args.trim().is_empty() {
            c.args = "{}".into();
        }
    }
    Ok(reply)
}

/// A short reply with no streaming and no tools: used to summarise old turns.
pub fn ask(at: &Endpoint, messages: &[Value], stop: &AtomicBool) -> Result<String, String> {
    let mut out = String::new();
    complete(at, messages, &[], stop, &mut |t| out.push_str(t)).map_err(|f| f.message)?;
    Ok(without_thinking(&out))
}

/// The reply without any `<think>…</think>` reasoning, which is not worth keeping.
pub fn without_thinking(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</think>") {
            Some(end) => rest = &rest[start + end + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}
