//! Signing in to mail through the browser, for Google and Microsoft accounts.
//!
//! This is the sign-in desktop mail apps use (OAuth 2.0 for native apps, RFC 8252, with
//! PKCE, RFC 7636). The person signs in on Google's or Microsoft's own page in their usual
//! browser, so Nimbus never sees the password, and two-step checks, passkeys and work or
//! school sign-in all work as they do on the web. The browser then comes back to a
//! one-time address on this computer (127.0.0.1, a free port) with a code, which is
//! traded for tokens. Only the long-lived refresh token is kept, in the system's
//! credential store; the access token, good for an hour, lives in memory.
//!
//! The scopes asked for are the mail (read over IMAP, which Nimbus keeps read-only) and
//! the address it belongs to. Nothing else.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// How long the browser sign-in may take before it is given up.
const WAIT: Duration = Duration::from_secs(300);

/// Bumped by every new sign-in and by Cancel, so only the newest one keeps waiting.
static EPOCH: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Google,
    Microsoft,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Google => "Google",
            Kind::Microsoft => "Microsoft",
        }
    }

    pub fn imap_host(self) -> &'static str {
        match self {
            Kind::Google => "imap.gmail.com",
            Kind::Microsoft => "outlook.office365.com",
        }
    }
}

/// Where to sign in, and as which app.
#[derive(Debug, Clone)]
pub struct Issuer {
    pub kind: Kind,
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    /// Google gives desktop apps a "secret" too. It is not secret (every copy of the app
    /// holds it), but its token endpoint asks for it.
    pub client_secret: Option<String>,
    pub scope: &'static str,
}

/// The sign-in app IDs: the person's own (Settings) or those built in when Nimbus was
/// built with `NIMBUS_GOOGLE_CLIENT_ID`, `NIMBUS_GOOGLE_CLIENT_SECRET` and
/// `NIMBUS_MICROSOFT_CLIENT_ID` set.
pub fn issuer(kind: Kind, s: &crate::settings::Settings) -> Option<Issuer> {
    let pick = |own: &Option<String>, built: Option<&'static str>| own.clone().filter(|v| !v.trim().is_empty()).or(built.map(String::from)).map(|v| v.trim().to_string());
    match kind {
        Kind::Google => Some(Issuer {
            kind,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            client_id: pick(&s.google_client_id, option_env!("NIMBUS_GOOGLE_CLIENT_ID"))?,
            client_secret: pick(&s.google_client_secret, option_env!("NIMBUS_GOOGLE_CLIENT_SECRET")),
            scope: "https://mail.google.com/ openid email",
        }),
        Kind::Microsoft => Some(Issuer {
            kind,
            // "common" takes both personal accounts (Outlook.com, Hotmail) and work or school ones.
            auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".into(),
            token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".into(),
            client_id: pick(&s.microsoft_client_id, option_env!("NIMBUS_MICROSOFT_CLIENT_ID"))?,
            client_secret: None,
            scope: "https://outlook.office.com/IMAP.AccessAsUser.All offline_access openid email",
        }),
    }
}

/// What a sign-in or a refresh gives back.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: String,
    pub expires: Instant,
    /// Microsoft may hand out a new one on each refresh; keep the newest.
    pub refresh: Option<String>,
    /// The address the account belongs to, from the ID token.
    pub email: Option<String>,
}

fn random(n: usize) -> String {
    let mut buf = vec![0u8; n];
    ring::rand::SystemRandom::new().fill(&mut buf).expect("the system's random numbers");
    URL_SAFE_NO_PAD.encode(buf)
}

/// The PKCE pair: the verifier stays here, its SHA-256 goes out with the browser.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes()))
}

pub fn enc(v: &str) -> String {
    v.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn dec(v: &str) -> String {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'+', _) => out.push(b' '),
            (b'%', Some(x)) => {
                out.push(x);
                i += 2;
            }
            (x, _) => out.push(x),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(q: &str) -> Vec<(String, String)> {
    q.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').unwrap_or((p, ""))).map(|(k, v)| (dec(k), dec(v))).collect()
}

fn authorize_url(iss: &Issuer, redirect: &str, challenge: &str, state: &str, hint: Option<&str>) -> String {
    let mut url = format!(
        "{}?client_id={}&response_type=code&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        iss.auth_url,
        enc(&iss.client_id),
        enc(redirect),
        enc(iss.scope),
        enc(state),
        enc(challenge)
    );
    match iss.kind {
        // Ask for a refresh token every time, so signing in again always yields one.
        Kind::Google => url.push_str("&access_type=offline&prompt=consent"),
        Kind::Microsoft => url.push_str("&response_mode=query&prompt=select_account"),
    }
    if let Some(h) = hint.map(str::trim).filter(|h| h.contains('@')) {
        url.push_str(&format!("&login_hint={}", enc(h)));
    }
    url
}

/// The address from an ID token. It came straight from the token endpoint over TLS, so
/// its signature is not checked again here.
fn email_of(id_token: &str) -> Option<String> {
    let body = id_token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(body.trim_end_matches('=')).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    ["email", "preferred_username", "upn"]
        .iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).filter(|e| e.contains('@')))
        .map(|e| e.trim().to_lowercase())
}

fn http() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn token_call(iss: &Issuer, form: &[(&str, &str)]) -> Result<Tokens, String> {
    let mut form: Vec<(&str, &str)> = form.to_vec();
    form.push(("client_id", &iss.client_id));
    if let Some(s) = iss.client_secret.as_deref() {
        form.push(("client_secret", s));
    }
    if iss.kind == Kind::Microsoft {
        form.push(("scope", iss.scope));
    }
    let mut res = http().post(&iss.token_url).send_form(form).map_err(|e| format!("{} did not answer: {e}", iss.kind.name()))?;
    let text = res.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| format!("{} sent something unexpected", iss.kind.name()))?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        let why = v.get("error_description").and_then(Value::as_str).unwrap_or(e);
        // Microsoft's descriptions run long: the first sentence says it.
        let why = why.split(['\r', '\n']).next().unwrap_or(why);
        return Err(if e == "invalid_grant" { format!("the sign-in has expired or was withdrawn ({why})") } else { why.to_string() });
    }
    let access = v.get("access_token").and_then(Value::as_str).ok_or("no access token came back")?.to_string();
    let secs = v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600);
    Ok(Tokens {
        access,
        expires: Instant::now() + Duration::from_secs(secs.saturating_sub(60)),
        refresh: v.get("refresh_token").and_then(Value::as_str).map(String::from),
        email: v.get("id_token").and_then(Value::as_str).and_then(email_of),
    })
}

/// A fresh access token from a kept refresh token.
pub fn refresh(iss: &Issuer, refresh_token: &str) -> Result<Tokens, String> {
    token_call(iss, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token)])
}

/// The SASL XOAUTH2 answer for IMAP's AUTHENTICATE.
pub fn xoauth2(user: &str, access: &str) -> String {
    STANDARD.encode(format!("user={user}\x01auth=Bearer {access}\x01\x01"))
}

/// Stop a sign-in that is waiting for the browser.
pub fn cancel() {
    EPOCH.fetch_add(1, Ordering::SeqCst);
}

/// The one-time address on this computer the browser comes back to. On 127.0.0.1, and
/// on ::1 at the same port when it is free, since "localhost" may go to either.
fn listen() -> Result<(u16, Vec<TcpListener>), String> {
    let v4 = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("no local port for the sign-in: {e}"))?;
    let port = v4.local_addr().map_err(|e| e.to_string())?.port();
    let mut all = vec![v4];
    if let Ok(v6) = TcpListener::bind(("::1", port)) {
        all.push(v6);
    }
    for l in &all {
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
    }
    Ok((port, all))
}

const PAGE_DONE: &str = "Signed in. You can close this tab and go back to Nimbus.";

fn page(title: &str, line: &str) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>Nimbus</title>\
<style>:root{{color-scheme:dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0b0b10;color:#ececf1;font:16px/1.5 system-ui,sans-serif}}\
main{{max-width:420px;padding:28px 32px;border-radius:24px;background:#16161d;box-shadow:0 0 0 1px #ffffff12}}h1{{margin:0 0 6px;font-size:20px}}p{{margin:0;color:#a9a9b6}}</style>\
<main><h1>{}</h1><p>{}</p></main></html>",
        esc(title),
        esc(line)
    )
}

fn answer(mut s: &TcpStream, status: &str, html: &str) {
    let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{html}", html.len());
    let _ = s.flush();
}

/// One request to the local address: the code, a refusal, or nothing useful (a stray
/// request such as the browser asking for an icon).
enum Visit {
    Code(String),
    Refused(String),
    Stray,
}

fn visit(mut s: TcpStream, state: &str) -> Visit {
    let _ = s.set_nonblocking(false);
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head.lines().next().and_then(|l| l.strip_prefix("GET ")).and_then(|l| l.split(' ').next()).unwrap_or("");
    let Some(q) = target.strip_prefix("/?") else {
        answer(&s, "404 Not Found", &page("Nothing here", "This address only takes the sign-in."));
        return Visit::Stray;
    };
    let params = query(q);
    let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    if get("state").as_deref() != Some(state) {
        answer(&s, "400 Bad Request", &page("Not this sign-in", "This answer belongs to another sign-in. Start again from Nimbus."));
        return Visit::Stray;
    }
    if let Some(e) = get("error") {
        let why = get("error_description").unwrap_or_else(|| e.clone());
        let why = why.split(['\r', '\n']).next().unwrap_or(&why).to_string();
        let why = if e == "access_denied" { "you said no on the sign-in page".to_string() } else { why };
        answer(&s, "200 OK", &page("Not signed in", &format!("Nimbus was not given access: {why}. You can close this tab.")));
        return Visit::Refused(why);
    }
    match get("code") {
        Some(code) => {
            answer(&s, "200 OK", &page("You're signed in", PAGE_DONE));
            Visit::Code(code)
        }
        None => {
            answer(&s, "400 Bad Request", &page("No code", "The sign-in came back without a code. Start again from Nimbus."));
            Visit::Stray
        }
    }
}

/// Sign in through the browser: `open` shows the person the sign-in page. Waits up to
/// five minutes for them, unless Cancel is pressed or another sign-in starts.
pub fn sign_in(iss: &Issuer, hint: Option<&str>, open: impl FnOnce(&str) -> Result<(), String>) -> Result<Tokens, String> {
    let mine = EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let (port, listeners) = listen()?;
    // Google's desktop clients take any port on 127.0.0.1; Microsoft's are registered
    // as http://localhost and take any port there.
    let redirect = match iss.kind {
        Kind::Google => format!("http://127.0.0.1:{port}"),
        Kind::Microsoft => format!("http://localhost:{port}"),
    };
    let verifier = random(48);
    let state = random(18);
    open(&authorize_url(iss, &redirect, &challenge(&verifier), &state, hint))?;

    let began = Instant::now();
    let code = 'wait: loop {
        if EPOCH.load(Ordering::SeqCst) != mine {
            return Err("the sign-in was cancelled".into());
        }
        if began.elapsed() > WAIT {
            return Err("the sign-in took longer than five minutes; start it again".into());
        }
        for l in &listeners {
            if let Ok((s, _)) = l.accept() {
                match visit(s, &state) {
                    Visit::Code(c) => break 'wait c,
                    Visit::Refused(why) => return Err(format!("not signed in: {why}")),
                    Visit::Stray => {}
                }
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    drop(listeners);
    token_call(iss, &[("grant_type", "authorization_code"), ("code", &code), ("redirect_uri", &redirect), ("code_verifier", &verifier)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        // 48 random bytes make a 64-character verifier, inside PKCE's 43 to 128.
        let v = random(48);
        assert_eq!(v.len(), 64);
        assert_ne!(v, random(48));
    }

    #[test]
    fn query_round_trip() {
        assert_eq!(dec("a%20b+c%2Fd%"), "a b c/d%");
        assert_eq!(dec(&enc("4/0Ab_x?y=z&ü")), "4/0Ab_x?y=z&ü");
        let q = query("code=4%2F0AbCd&state=xyz&scope=email+openid");
        assert_eq!(q[0], ("code".into(), "4/0AbCd".into()));
        assert_eq!(q[2].1, "email openid");
    }

    #[test]
    fn address_from_id_token() {
        let body = URL_SAFE_NO_PAD.encode(r#"{"email":"Ana@Outlook.com","name":"Ana"}"#);
        assert_eq!(email_of(&format!("h.{body}.sig")).as_deref(), Some("ana@outlook.com"));
        let ms = URL_SAFE_NO_PAD.encode(r#"{"preferred_username":"ada@uni.example"}"#);
        assert_eq!(email_of(&format!("h.{ms}.sig")).as_deref(), Some("ada@uni.example"));
        assert_eq!(email_of("not-a-token"), None);
    }

    #[test]
    fn xoauth2_string() {
        let raw = STANDARD.decode(xoauth2("a@b.de", "tok")).unwrap();
        assert_eq!(raw, b"user=a@b.de\x01auth=Bearer tok\x01\x01");
    }

    fn fake(kind: Kind, base: &str) -> Issuer {
        Issuer {
            kind,
            auth_url: format!("{base}/authorize"),
            token_url: format!("{base}/token"),
            client_id: "test-client".into(),
            client_secret: (kind == Kind::Google).then(|| "test-secret".into()),
            scope: "https://mail.google.com/ openid email",
        }
    }

    type Seen = std::sync::Arc<parking_lot::Mutex<String>>;

    /// A stand-in for Google's or Microsoft's token endpoint: checks the PKCE verifier
    /// against the challenge the browser carried, and answers once.
    fn token_server(challenge_seen: Seen) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let h = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = s.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    if body.len() >= len {
                        break;
                    }
                }
            }
            let text = String::from_utf8_lossy(&buf).to_string();
            let body = text.split_once("\r\n\r\n").unwrap().1.to_string();
            let form = query(&body);
            let get = |k: &str| form.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
            let ok = challenge(&get("code_verifier")) == *challenge_seen.lock() && get("code") == "code-123";
            let id = URL_SAFE_NO_PAD.encode(r#"{"email":"me@gmail.com"}"#);
            let reply = if ok {
                format!(r#"{{"access_token":"at-1","expires_in":3599,"refresh_token":"rt-1","id_token":"h.{id}.s"}}"#)
            } else {
                r#"{"error":"invalid_grant","error_description":"Bad verifier"}"#.to_string()
            };
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
            body
        });
        (base, h)
    }

    /// A stand-in browser: reads the sign-in page's address and comes straight back
    /// to the local address, as the provider would after the person signs in.
    fn browser(url: &str, challenge_seen: &parking_lot::Mutex<String>, outcome: &'static str, then: impl FnOnce(&str) + Send + 'static) {
        let q = query(url.split_once('?').unwrap().1);
        let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(get("code_challenge_method"), "S256");
        assert_eq!(get("client_id"), "test-client");
        *challenge_seen.lock() = get("code_challenge");
        let back = format!("{}/?{outcome}&state={}", get("redirect_uri"), enc(&get("state")));
        std::thread::spawn(move || {
            let host = back.split("://").nth(1).unwrap().split('/').next().unwrap().replace("localhost", "127.0.0.1");
            // A stray first, as browsers ask for an icon.
            let mut s = TcpStream::connect(&host).unwrap();
            write!(s, "GET /favicon.ico HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
            let mut sink = String::new();
            let _ = s.read_to_string(&mut sink);
            let path = &back[back.find("/?").unwrap()..];
            let mut s = TcpStream::connect(&host).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
            let mut page = String::new();
            let _ = s.read_to_string(&mut page);
            then(&page);
        });
    }

    // The two sign-in tests share EPOCH, so they run one after the other.
    static ONE_AT_A_TIME: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn browser_sign_in_end_to_end() {
        let _one = ONE_AT_A_TIME.lock();
        let seen: Seen = Default::default();
        let (base, server) = token_server(seen.clone());
        let iss = fake(Kind::Google, &base);
        let page: Seen = Default::default();
        let p2 = page.clone();
        let t = sign_in(&iss, Some("me@gmail.com"), |url| {
            assert!(url.contains("login_hint=me%40gmail.com") && url.contains("access_type=offline"));
            browser(url, &seen, "code=code-123", move |p| *p2.lock() = p.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!((t.access.as_str(), t.refresh.as_deref(), t.email.as_deref()), ("at-1", Some("rt-1"), Some("me@gmail.com")));
        let sent = server.join().unwrap();
        assert!(sent.contains("grant_type=authorization_code") && sent.contains("client_secret=test-secret"), "{sent}");
        std::thread::sleep(Duration::from_millis(150));
        assert!(page.lock().contains(PAGE_DONE));
    }

    #[test]
    fn saying_no_in_the_browser() {
        let _one = ONE_AT_A_TIME.lock();
        let seen = parking_lot::Mutex::new(String::new());
        let iss = fake(Kind::Microsoft, "http://127.0.0.1:9");
        let err = sign_in(&iss, None, |url| {
            assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A") && url.contains("prompt=select_account"));
            browser(url, &seen, "error=access_denied", |_| {});
            Ok(())
        })
        .unwrap_err();
        assert!(err.contains("you said no"), "{err}");
    }
}
