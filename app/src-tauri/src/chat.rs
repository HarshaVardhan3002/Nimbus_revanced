//! Typed chat with a language model, over the OpenAI-style HTTP API that Lemonade,
//! Ollama, LM Studio, llama.cpp's server and the cloud services all speak. It is the
//! way in that always works, with or without voice.
//!
//! This finds the server and its models; the agent (agent/) holds the conversation.

use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

/// Local servers tried in order when no address is set: the local brain Nimbus starts
/// itself (brain.rs), Lemonade (both of its usual ports), Ollama, LM Studio, and
/// llama.cpp's own server.
const KNOWN: [(&str, &str); 6] = [
    (crate::brain::URL, "The local brain"),
    ("http://localhost:13305/api/v1", "Lemonade"),
    ("http://localhost:8000/api/v1", "Lemonade"),
    ("http://localhost:11434/v1", "Ollama"),
    ("http://localhost:1234/v1", "LM Studio"),
    ("http://localhost:8080/v1", "llama.cpp"),
];

/// Models that do something other than chat (speech, embeddings) are left out of the list.
const NOT_CHAT: [&str; 9] = ["whisper", "kokoro", "tts", "embed", "rerank", "moonshine", "transcri", "stable-diffusion", "sdxl"];

/// Where the chat goes: found on this computer, or set by the user.
#[derive(Debug, Clone, Serialize)]
pub struct Status {
    /// The server's address, or `None` when nothing answered.
    pub url: Option<String>,
    /// A name for the server ("Lemonade", "Ollama", or the host).
    pub server: String,
    /// Chat models it offers.
    pub models: Vec<String>,
    /// The model replies come from: the chosen one if it is offered, else the first.
    pub model: Option<String>,
    /// Set by the user rather than found.
    pub custom: bool,
    /// Why the set address did not answer, if it did not.
    pub error: Option<String>,
}

fn agent(answer_within: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(1500)))
        .timeout_recv_response(Some(answer_within))
        .http_status_as_error(false)
        .build()
        .into()
}

fn with_key(req: ureq::RequestBuilder<ureq::typestate::WithoutBody>, key: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    if key.is_empty() { req } else { req.header("Authorization", format!("Bearer {key}")) }
}

fn trim_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn server_name(url: &str) -> String {
    KNOWN
        .iter()
        .find(|(u, _)| *u == url)
        .map(|(_, n)| n.to_string())
        .unwrap_or_else(|| {
            let host = url.split("://").nth(1).unwrap_or(url);
            host.split(['/', ':']).next().unwrap_or(host).to_string()
        })
}

/// The error a server sent back, in its own words when it gave any.
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

/// The chat models a server offers.
fn models(url: &str, key: &str) -> Result<Vec<String>, String> {
    let mut res = with_key(agent(Duration::from_secs(4)).get(format!("{url}/models")), key)
        .call()
        .map_err(|e| match e {
            ureq::Error::ConnectionFailed | ureq::Error::Io(_) | ureq::Error::HostNotFound => "nothing answered there".to_string(),
            ureq::Error::Timeout(_) => "it did not answer in time".to_string(),
            e => e.to_string(),
        })?;
    let status = res.status().as_u16();
    let body = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    if status >= 400 {
        return Err(server_error(status, &body));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| "it does not speak the OpenAI API".to_string())?;
    let list = v.get("data").or_else(|| v.get("models")).and_then(Value::as_array).ok_or("it listed no models")?;
    Ok(list
        .iter()
        .filter(|m| {
            let labels = m.get("labels").and_then(Value::as_array).map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .any(|l| ["transcription", "tts", "embeddings", "reranking", "image"].contains(&l))
            });
            m.get("downloaded").and_then(Value::as_bool) != Some(false) && labels != Some(true)
        })
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .filter(|id| {
            let id = id.to_lowercase();
            !NOT_CHAT.iter().any(|n| id.contains(n))
        })
        .map(str::to_string)
        .collect())
}

/// Find where the chat goes. A set address is used as it is; otherwise the first
/// local server with a chat model wins, then the first that answered at all.
pub fn status(url: Option<&str>, key: &str, chosen: Option<&str>) -> Status {
    let pick = |models: &[String]| chosen.filter(|c| models.iter().any(|m| m == c)).map(str::to_string).or_else(|| models.first().cloned());
    if let Some(url) = url.map(trim_url).filter(|u| !u.is_empty()) {
        return match models(&url, key) {
            Ok(m) => Status { server: server_name(&url), model: pick(&m), models: m, url: Some(url), custom: true, error: None },
            Err(e) => Status { server: server_name(&url), models: vec![], model: None, url: Some(url), custom: true, error: Some(e) },
        };
    }
    // Probed together, so the slowest closed port costs one timeout, not five.
    let found: Vec<_> = std::thread::scope(|s| {
        let probes: Vec<_> = KNOWN.iter().map(|(u, _)| s.spawn(move || models(u, ""))).collect();
        probes.into_iter().zip(KNOWN).filter_map(|(p, (u, n))| p.join().ok()?.ok().map(|m| (u, n, m))).collect()
    });
    let best = found.iter().find(|(_, _, m)| !m.is_empty()).or_else(|| found.first());
    match best {
        Some((u, n, m)) => Status { url: Some(u.to_string()), server: n.to_string(), model: pick(m), models: m.clone(), custom: false, error: None },
        None => Status { url: None, server: String::new(), models: vec![], model: None, custom: false, error: None },
    }
}
