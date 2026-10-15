//! Plugins: programs that give the assistant more tools. Each speaks the Model Context
//! Protocol over its standard input and output (one JSON message a line), the way MCP
//! tool servers do, so any such server works as a plugin.
//!
//! Plugins are listed in Settings (`plugins`), started with Nimbus and stopped with it.
//! Each gets the local API's address and token (see `api`), so it can hand Nimbus a
//! request or show a notice. Its tools ask the person first, except those the person
//! listed in the plugin's `allow`; nothing a plugin says changes the safety rules.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PluginConfig {
    /// Shown with its steps and notices.
    pub name: String,
    /// The program, and what it is started with.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Tools that run without asking.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Tools that may run without an extra question after a mail was read in the request
    /// (nothing they do can carry the mail's words to someone else).
    #[serde(default)]
    pub allow_after_mail: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

/// How long one tool call may take.
const CALL_LIMIT: Duration = Duration::from_secs(10 * 60);

struct Conn {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    waiting: Mutex<HashMap<u64, Sender<Value>>>,
    next: AtomicU64,
}

impl Conn {
    fn send(&self, msg: &Value) -> Result<(), String> {
        let mut w = self.stdin.lock();
        writeln!(w, "{msg}").and_then(|_| w.flush()).map_err(|e| format!("the plugin stopped listening: {e}"))
    }

    fn request(&self, method: &str, params: Value, limit: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = channel();
        self.waiting.lock().insert(id, tx);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let reply = rx.recv_timeout(limit).map_err(|_| {
            self.waiting.lock().remove(&id);
            format!("the plugin did not answer {method} in time")
        })?;
        match reply.get("error") {
            Some(e) => Err(e["message"].as_str().unwrap_or("the plugin failed").to_string()),
            None => Ok(reply["result"].clone()),
        }
    }

    fn alive(&self) -> bool {
        matches!(self.child.lock().try_wait(), Ok(None))
    }
}

struct Plugin {
    config: PluginConfig,
    conn: Option<Arc<Conn>>,
    tools: Vec<Value>,
}

static PLUGINS: OnceLock<Mutex<Vec<Plugin>>> = OnceLock::new();

fn all() -> &'static Mutex<Vec<Plugin>> {
    PLUGINS.get_or_init(Default::default)
}

/// Starts the plugins in Settings, each in the background.
pub fn spawn(configs: Vec<PluginConfig>, builtin: Vec<String>) {
    for config in configs.into_iter().filter(|c| c.enabled) {
        let builtin = builtin.clone();
        std::thread::spawn(move || {
            let name = config.name.clone();
            match start(&config) {
                Ok((conn, mut tools)) => {
                    tools.retain(|t| {
                        let n = t["name"].as_str().unwrap_or_default();
                        let clash = builtin.iter().any(|b| b == n);
                        if clash {
                            eprintln!("[nimbus] plugin {name}: its tool {n} has the name of one of Nimbus's own; left out");
                        }
                        !n.is_empty() && !clash
                    });
                    eprintln!("[nimbus] plugin {name}: {} tools", tools.len());
                    let mut list = all().lock();
                    list.retain(|p| p.config.name != name);
                    list.push(Plugin { config, conn: Some(conn), tools });
                }
                Err(e) => eprintln!("[nimbus] plugin {name} did not start: {e}"),
            }
        });
    }
}

fn start(config: &PluginConfig) -> Result<(Arc<Conn>, Vec<Value>), String> {
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some((url, token)) = crate::api::address() {
        cmd.env("NIMBUS_API", url).env("NIMBUS_TOKEN", token);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // no console window
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", config.command))?;
    let (stdin, stdout, stderr) = (child.stdin.take().ok_or("no stdin")?, child.stdout.take().ok_or("no stdout")?, child.stderr.take());
    let conn = Arc::new(Conn { stdin: Mutex::new(stdin), child: Mutex::new(child), waiting: Mutex::default(), next: AtomicU64::new(0) });
    let name = config.name.clone();
    if let Some(err) = stderr {
        let name = name.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                eprintln!("[{name}] {line}");
            }
        });
    }
    let reader = conn.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
            match (msg.get("id"), msg.get("method")) {
                // A request of the plugin's own (sampling, roots): not offered.
                (Some(id), Some(_)) => {
                    let _ = reader.send(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "not offered by Nimbus" } }));
                }
                (Some(id), None) => {
                    if let Some(tx) = id.as_u64().and_then(|id| reader.waiting.lock().remove(&id)) {
                        let _ = tx.send(msg);
                    }
                }
                _ => {}
            }
        }
        // The plugin ended: whoever waits hears so.
        reader.waiting.lock().clear();
    });
    conn.request(
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": crate::BRAND_NAME, "version": env!("CARGO_PKG_VERSION") } }),
        Duration::from_secs(20),
    )?;
    conn.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))?;
    let listed = conn.request("tools/list", json!({}), Duration::from_secs(20))?;
    Ok((conn, listed["tools"].as_array().cloned().unwrap_or_default()))
}

/// The plugins' tools, the way the model is offered tools.
pub fn specs() -> Vec<Value> {
    all()
        .lock()
        .iter()
        .flat_map(|p| p.tools.iter())
        .map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t["name"],
                    "description": t["description"].as_str().unwrap_or_default(),
                    "parameters": if t["inputSchema"].is_object() { t["inputSchema"].clone() } else { json!({ "type": "object", "properties": {} }) },
                }
            })
        })
        .collect()
}

/// What the person allowed for a plugin's tool: (runs without asking, may run after a mail
/// was read). None when no plugin has the tool.
pub fn allowed(tool: &str) -> Option<(bool, bool)> {
    let list = all().lock();
    let p = list.iter().find(|p| p.tools.iter().any(|t| t["name"] == tool))?;
    Some((p.config.allow.iter().any(|a| a == tool), p.config.allow_after_mail.iter().any(|a| a == tool)))
}

/// A plain line for the chat: the tool's own title, with its plugin's name.
pub fn title(tool: &str) -> Option<String> {
    let list = all().lock();
    let p = list.iter().find(|p| p.tools.iter().any(|t| t["name"] == tool))?;
    let t = p.tools.iter().find(|t| t["name"] == tool)?;
    let label = t["title"].as_str().or_else(|| t["annotations"]["title"].as_str()).unwrap_or(tool);
    Some(format!("{label} ({})", p.config.name))
}

/// Runs a plugin's tool; the text it answers goes to the model. A plugin that ended is
/// started again once.
pub fn call(tool: &str, args: &Value) -> Option<Result<String, String>> {
    let (conn, config) = {
        let list = all().lock();
        let p = list.iter().find(|p| p.tools.iter().any(|t| t["name"] == tool))?;
        (p.conn.clone(), p.config.clone())
    };
    let conn = match conn.filter(|c| c.alive()) {
        Some(c) => c,
        None => match start(&config) {
            Ok((c, tools)) => {
                if let Some(p) = all().lock().iter_mut().find(|p| p.config.name == config.name) {
                    p.conn = Some(c.clone());
                    p.tools = tools;
                }
                c
            }
            Err(e) => return Some(Err(format!("the plugin {} is not running: {e}", config.name))),
        },
    };
    let mut args = args.clone();
    if let Some(o) = args.as_object_mut() {
        o.retain(|k, _| !k.starts_with('_'));
    }
    Some(conn.request("tools/call", json!({ "name": tool, "arguments": args }), CALL_LIMIT).and_then(|r| {
        let text = r["content"]
            .as_array()
            .map(|c| c.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        if r["isError"] == true { Err(if text.is_empty() { "the plugin's tool failed".into() } else { text }) } else { Ok(text) }
    }))
}

/// Stops the plugins, with Nimbus.
pub fn stop() {
    for p in all().lock().iter() {
        if let Some(c) = &p.conn {
            let _ = c.child.lock().kill();
        }
    }
}
