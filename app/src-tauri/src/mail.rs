//! Mail, read-only. The person signs in with a password or app password, or through
//! Google or Microsoft in their browser (see `oauth`), and the assistant can list and read
//! mail for a briefing. Nothing else is possible from here, by design:
//!
//! - The IMAP client below can send exactly six commands: LOGIN or AUTHENTICATE XOAUTH2
//!   (signing in), EXAMINE (opens a mailbox read-only), UID SEARCH, UID FETCH with
//!   BODY.PEEK (leaves mails unread) and LOGOUT. It cannot move, flag, delete or append,
//!   and there is no SMTP code at all.
//! - Writing goes through a draft in the browser or mail app; the person sends it.
//! - Passwords and browser sign-ins live in the system's credential store (Windows
//!   Credential Manager, macOS Keychain, Secret Service), never in Nimbus's own files.

use mail_parser::{MessageParser, MimeHeaders};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use crate::oauth::{self, Kind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const KEYRING: &str = "Nimbus mail";
/// Where passwords were kept while the app was called Jarvis; moved over on first start.
const OLD_KEYRING: &str = "Jarvis mail";
/// How much of each mail is fetched for a list (enough for headers and a snippet).
const LIST_BYTES: usize = 24 * 1024;
const READ_BYTES: usize = 512 * 1024;
const THREAD_BYTES: usize = 160 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub host: String,
    pub port: u16,
    /// The IMAP login (usually the address).
    pub user: String,
    /// Signed in through Google or Microsoft in the browser; `None` is a password.
    #[serde(default)]
    pub auth: Option<Kind>,
}

/// A mail in a list.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub account: String,
    pub uid: u32,
    pub from: String,
    pub subject: String,
    /// Sent, in ms since 1970.
    pub date: Option<i64>,
    pub unread: bool,
    pub snippet: String,
    /// "inbox", or "sent" for the person's own mails.
    pub folder: &'static str,
    pub to: String,
    /// Marked as answered (the person replied to it from their mail app).
    pub answered: bool,
    #[serde(skip)]
    pub message_id: Option<String>,
}

/// A mail read in full.
#[derive(Debug, Clone, Serialize)]
pub struct Letter {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: Option<i64>,
    pub text: String,
    pub attachments: Vec<String>,
    #[serde(skip)]
    pub message_id: Option<String>,
    /// The mails this one answers, oldest first (In-Reply-To and References).
    #[serde(skip)]
    pub references: Vec<String>,
}

/// Where a provider's IMAP lives, and what the person needs to know to sign in.
#[derive(Debug, Clone, Serialize)]
pub struct Provider {
    pub name: &'static str,
    pub host: String,
    pub port: u16,
    /// What to use as the password, in a sentence.
    pub hint: &'static str,
    /// Where to make an app password, if the provider wants one.
    pub link: Option<&'static str>,
    /// The provider no longer accepts passwords for IMAP.
    pub blocked: bool,
    /// The sign-in name is not the address: what to call it in the form.
    pub login: Option<&'static str>,
    /// The provider signs in through Google or Microsoft in the browser.
    pub browser: Option<Kind>,
}

/// Outlook.com's own addresses (as against work or school mail at Microsoft 365).
const OUTLOOK_HOME: &[&str] = &["outlook.com", "hotmail.com", "live.com", "msn.com", "outlook.de", "hotmail.de", "live.de"];

fn domain(email: &str) -> String {
    email.rsplit('@').next().unwrap_or_default().trim().to_lowercase()
}

pub fn provider(email: &str) -> Provider {
    let domain = domain(email);
    let p = |name, host: &str, hint, link| Provider { name, host: host.into(), port: 993, hint, link, blocked: false, login: None, browser: None };
    match domain.as_str() {
        "gmail.com" | "googlemail.com" => Provider {
            browser: Some(Kind::Google),
            ..p(
                "Gmail",
                "imap.gmail.com",
                "Continue with Google above, or use an app password here (not your Google password; it needs 2-step verification on).",
                Some("https://myaccount.google.com/apppasswords"),
            )
        },
        "icloud.com" | "me.com" | "mac.com" => {
            p("iCloud Mail", "imap.mail.me.com", "Use an app-specific password from your Apple account.", Some("https://account.apple.com/account/manage"))
        }
        "yahoo.com" | "yahoo.de" | "ymail.com" => {
            p("Yahoo Mail", "imap.mail.yahoo.com", "Use an app password from Yahoo's account security page.", Some("https://login.yahoo.com/account/security"))
        }
        "gmx.de" | "gmx.net" | "gmx.at" | "gmx.ch" => {
            p("GMX", "imap.gmx.net", "Use your GMX password. Turn on POP3/IMAP access in GMX's settings first.", None)
        }
        "web.de" => p("WEB.DE", "imap.web.de", "Use your WEB.DE password. Turn on POP3/IMAP access in WEB.DE's settings first.", None),
        "t-online.de" => p("Telekom Mail", "secureimap.t-online.de", "Use a mail password (Passwort für E-Mail-Programme) from Telekom's settings.", None),
        "posteo.de" | "posteo.net" => p("Posteo", "posteo.de", "Use your Posteo password.", None),
        "mailbox.org" => p("mailbox.org", "imap.mailbox.org", "Use your mailbox.org password.", None),
        "fastmail.com" | "fastmail.fm" => {
            p("Fastmail", "imap.fastmail.com", "Use an app password from Fastmail's settings.", Some("https://app.fastmail.com/settings/security/apps"))
        }
        "ovgu.de" | "st.ovgu.de" | "uni-magdeburg.de" | "st.uni-magdeburg.de" => Provider {
            login: Some("OVGU account name"),
            ..p("OVGU Mail", "cyrus.ovgu.de", "Sign in with your OVGU account name and its password, not the mail address.", None)
        },
        "proton.me" | "protonmail.com" | "pm.me" => Provider {
            name: "Proton Mail",
            host: "127.0.0.1".into(),
            port: 1143,
            hint: "Proton Mail works through Proton Mail Bridge on this computer; use the password the Bridge shows.",
            link: Some("https://proton.me/mail/bridge"),
            blocked: false,
            login: None,
            browser: None,
        },
        d if OUTLOOK_HOME.contains(&d) => Provider {
            name: "Outlook.com",
            host: "outlook.office365.com".into(),
            port: 993,
            hint: "Outlook.com signs in with Microsoft in your browser: use Continue with Microsoft above. Mail apps can no longer use its password.",
            link: None,
            blocked: true,
            login: None,
            browser: Some(Kind::Microsoft),
        },
        _ => p("Mail", &format!("imap.{domain}"), "Use your mail password, or an app password if your provider has them.", None),
    }
}

// ---- the IMAP client ----------------------------------------------------------------

type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// One untagged reply: its line, with each literal's bytes alongside.
struct Reply {
    line: String,
    literals: Vec<Vec<u8>>,
}

/// What this client can ask a server. Nothing here changes a mailbox.
enum Cmd<'a> {
    Login(&'a str, &'a str),
    /// SASL XOAUTH2 with its answer sent at once (an access token, never a password).
    XOauth2(&'a str),
    Examine(&'a str),
    UidSearch(&'a str),
    UidFetch(&'a str, usize),
    List,
    Logout,
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

struct Imap<S: Read + Write = Tls> {
    io: BufReader<S>,
    tag: u32,
}

fn tls_config() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .expect("rustls protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

impl Imap {
    fn connect(host: &str, port: u16) -> Result<Self, String> {
        let addr = (host, port)
            .to_socket_addrs()
            .map_err(|_| format!("{host} could not be found; check the server name"))?
            .next()
            .ok_or_else(|| format!("{host} could not be found"))?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|e| format!("{host} did not answer: {e}"))?;
        tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
        tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
        let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|_| format!("{host} is not a server name"))?;
        let conn = rustls::ClientConnection::new(tls_config(), name).map_err(|e| e.to_string())?;
        Imap::greet(rustls::StreamOwned::new(conn, tcp)).map_err(|e| format!("no secure connection to {host}: {e}"))
    }
}

impl<S: Read + Write> Imap<S> {
    fn greet(stream: S) -> Result<Self, String> {
        let mut me = Imap { io: BufReader::new(stream), tag: 0 };
        let hello = me.line()?;
        if !hello.starts_with("* OK") && !hello.starts_with("* PREAUTH") {
            return Err("the server turned the connection down".into());
        }
        Ok(me)
    }

    fn line(&mut self) -> Result<String, String> {
        let mut buf = Vec::new();
        let n = self.io.read_until(b'\n', &mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("the server closed the connection".into());
        }
        Ok(String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string())
    }

    /// Send a command and collect its untagged replies until the tagged one.
    fn run(&mut self, cmd: Cmd, max_literal: usize) -> Result<Vec<Reply>, String> {
        self.tag += 1;
        let tag = format!("j{}", self.tag);
        let text = match cmd {
            Cmd::Login(user, pass) => format!("LOGIN {} {}", quote(user), quote(pass)),
            Cmd::XOauth2(answer) => format!("AUTHENTICATE XOAUTH2 {answer}"),
            Cmd::Examine(mailbox) => format!("EXAMINE {}", quote(mailbox)),
            Cmd::UidSearch(criteria) => format!("UID SEARCH {criteria}"),
            Cmd::UidFetch(set, bytes) => format!("UID FETCH {set} (UID FLAGS BODY.PEEK[]<0.{bytes}>)"),
            Cmd::List => "LIST \"\" \"*\"".into(),
            Cmd::Logout => "LOGOUT".into(),
        };
        let stream = self.io.get_mut();
        stream.write_all(format!("{tag} {text}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        stream.flush().map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        loop {
            let mut line = self.line()?;
            // A turned-down XOAUTH2 sends its reason as a challenge; an empty answer
            // ends the exchange, and the tagged NO follows.
            if line.starts_with('+') {
                let stream = self.io.get_mut();
                stream.write_all(b"\r\n").map_err(|e| e.to_string())?;
                stream.flush().map_err(|e| e.to_string())?;
                continue;
            }
            if let Some(rest) = line.strip_prefix(&format!("{tag} ")) {
                if rest.starts_with("OK") {
                    return Ok(out);
                }
                // "NO [AUTHENTICATIONFAILED] Invalid credentials" -> the words after the code.
                let why = rest.split_once(' ').map(|(_, w)| w).unwrap_or(rest);
                let why = why.rsplit(']').next().unwrap_or(why).trim();
                return Err(why.to_string());
            }
            let mut literals = Vec::new();
            // A line ending in {n} is followed by n raw bytes, then the line goes on.
            while let Some(n) = literal_len(&line) {
                let mut bytes = vec![0; n];
                self.io.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                bytes.truncate(max_literal);
                literals.push(bytes);
                let more = self.line()?;
                line.push_str(&more);
            }
            out.push(Reply { line, literals });
        }
    }

    fn logout(&mut self) {
        let _ = self.run(Cmd::Logout, 0);
    }
}

fn literal_len(line: &str) -> Option<usize> {
    let open = line.strip_suffix('}')?.rfind('{')?;
    line[open + 1..line.len() - 1].parse().ok()
}

/// What an account signs in with.
enum Login {
    Password(String),
    /// An access token from a browser sign-in.
    Bearer(String),
}

/// Sign in and open the inbox read-only.
fn open(acc: &Account, login: &Login) -> Result<Imap, String> {
    let mut imap = Imap::connect(&acc.host, acc.port)?;
    match login {
        Login::Password(p) => imap.run(Cmd::Login(&acc.user, p), 0),
        Login::Bearer(t) => imap.run(Cmd::XOauth2(&oauth::xoauth2(&acc.user, t)), 0),
    }
    .map_err(|e| format!("sign-in failed: {e}"))?;
    imap.run(Cmd::Examine("INBOX"), 0)?;
    Ok(imap)
}

fn uid_of(line: &str) -> Option<u32> {
    let at = line.find("UID ")? + 4;
    line[at..].split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn unread(line: &str) -> bool {
    !flagged(line, "\\Seen")
}

fn flagged(line: &str, flag: &str) -> bool {
    line.find("FLAGS (").is_some_and(|i| line[i + 7..].split(')').next().unwrap_or_default().split_whitespace().any(|f| f.eq_ignore_ascii_case(flag)))
}

/// The folder sent mail is kept in: the one the server marks \Sent, or a usual name.
fn sent_folder<S: Read + Write>(imap: &mut Imap<S>) -> Option<String> {
    const USUAL: &[&str] = &[
        "sent",
        "sent items",
        "sent mail",
        "sent messages",
        "gesendet",
        "gesendete objekte",
        "gesendete elemente",
        "inbox.sent",
        "inbox/sent",
        "[gmail]/sent mail",
        "[gmail]/gesendet",
    ];
    let mut names = Vec::new();
    for r in imap.run(Cmd::List, 4096).ok()? {
        let Some(rest) = r.line.strip_prefix("* LIST ") else { continue };
        let flags = rest.split(')').next().unwrap_or_default();
        let name = match r.literals.first() {
            Some(lit) => String::from_utf8_lossy(lit).into_owned(),
            None => last_name(rest),
        };
        if flags.split_whitespace().any(|f| f.eq_ignore_ascii_case("\\Sent")) {
            return Some(name);
        }
        names.push(name);
    }
    names.into_iter().find(|n| USUAL.contains(&n.to_lowercase().as_str()))
}

/// The mailbox name at the end of a LIST line: quoted, or a bare word.
fn last_name(rest: &str) -> String {
    let rest = rest.trim_end();
    match rest.strip_suffix('"') {
        Some(inner) => {
            let mut start = inner.len();
            let b = inner.as_bytes();
            while start > 0 && !(b[start - 1] == b'"' && (start < 2 || b[start - 2] != b'\\')) {
                start -= 1;
            }
            inner[start..].replace("\\\"", "\"").replace("\\\\", "\\")
        }
        None => rest.rsplit(' ').next().unwrap_or_default().to_string(),
    }
}

/// Search words for IMAP: no line breaks (they would end the command), quoted.
fn search_text(s: &str) -> String {
    quote(&s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>())
}

/// "SINCE 01-Aug-2026 FROM "ana" TEXT "invoice"", marked as UTF-8 when it needs to be.
fn criteria(days: u32, field: &str, who: Option<&str>, about: Option<&str>) -> String {
    let since = (chrono::Local::now() - chrono::Duration::days(days as i64)).format("%d-%b-%Y");
    let mut c = format!("SINCE {since}");
    if let Some(w) = who {
        c.push_str(&format!(" {field} {}", search_text(w)));
    }
    if let Some(a) = about {
        c.push_str(&format!(" TEXT {}", search_text(a)));
    }
    if c.is_ascii() { c } else { format!("CHARSET UTF-8 {c}") }
}

fn addr(a: Option<&mail_parser::Address>) -> String {
    let Some(a) = a else { return String::new() };
    a.iter()
        .map(|x| match (x.name(), x.address()) {
            (Some(n), Some(e)) => format!("{n} <{e}>"),
            (Some(n), None) => n.to_string(),
            (None, Some(e)) => e.to_string(),
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for l in text.lines() {
        let l = l.trim_end();
        // Quoted history and long runs of blank lines say nothing new.
        if l.starts_with('>') {
            continue;
        }
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(l);
        out.push('\n');
    }
    out.trim().to_string()
}

fn body_text(m: &mail_parser::Message) -> String {
    m.body_text(0).map(|t| clean(&t)).unwrap_or_default()
}

/// The text, or the readable part of the HTML when a mail has no plain text.
fn text_of(m: &mail_parser::Message) -> String {
    let text = body_text(m);
    if !text.is_empty() {
        return text;
    }
    m.body_html(0).map(|h| clean(&crate::agent::web::readable(&h).1)).unwrap_or_default()
}

/// The newest mails in the inbox, newest first.
fn list(acc: &Account, login: &Login, count: usize, unread_only: bool) -> Result<Vec<Summary>, String> {
    let mut imap = open(acc, login)?;
    let out = newest(&mut imap, &acc.id, count, unread_only);
    imap.logout();
    out
}

fn newest<S: Read + Write>(imap: &mut Imap<S>, account: &str, count: usize, unread_only: bool) -> Result<Vec<Summary>, String> {
    let since = (chrono::Local::now() - chrono::Duration::days(30)).format("%d-%b-%Y").to_string();
    let criteria = if unread_only { format!("UNSEEN SINCE {since}") } else { format!("SINCE {since}") };
    matching(imap, account, &criteria, count, "inbox", 220)
}

/// The newest `count` mails in the open folder that match an IMAP search, newest first.
fn matching<S: Read + Write>(imap: &mut Imap<S>, account: &str, criteria: &str, count: usize, folder: &'static str, snip: usize) -> Result<Vec<Summary>, String> {
    matching_bytes(imap, account, criteria, count, folder, snip, LIST_BYTES)
}

/// The same, reading up to `bytes` of each mail (more for a conversation's few mails, so
/// a long quoted history or a big HTML part does not cut the text off).
fn matching_bytes<S: Read + Write>(imap: &mut Imap<S>, account: &str, criteria: &str, count: usize, folder: &'static str, snip: usize, bytes: usize) -> Result<Vec<Summary>, String> {
    let found = imap.run(Cmd::UidSearch(criteria), 0)?;
    let mut uids: Vec<u32> = found
        .iter()
        .filter_map(|r| r.line.strip_prefix("* SEARCH"))
        .flat_map(|rest| rest.split_whitespace().filter_map(|n| n.parse().ok()).collect::<Vec<u32>>())
        .collect();
    uids.sort_unstable();
    let newest: Vec<String> = uids.iter().rev().take(count).map(u32::to_string).collect();
    if newest.is_empty() {
        return Ok(Vec::new());
    }
    let replies = imap.run(Cmd::UidFetch(&newest.join(","), bytes), bytes)?;
    let mut out: Vec<Summary> = replies
        .iter()
        .filter(|r| r.line.contains(" FETCH "))
        .filter_map(|r| {
            let uid = uid_of(&r.line)?;
            let raw = r.literals.first()?;
            let m = MessageParser::default().parse(raw)?;
            let snippet: String = text_of(&m).split_whitespace().collect::<Vec<_>>().join(" ").chars().take(snip).collect();
            Some(Summary {
                account: account.to_string(),
                uid,
                from: addr(m.from()),
                subject: m.subject().unwrap_or("(no subject)").to_string(),
                date: m.date().map(|d| d.to_timestamp() * 1000),
                unread: unread(&r.line),
                snippet,
                folder,
                to: addr(m.to()),
                answered: flagged(&r.line, "\\Answered"),
                message_id: m.message_id().map(str::to_string),
            })
        })
        .collect();
    out.sort_by(|a, b| b.uid.cmp(&a.uid));
    Ok(out)
}

/// Mails from someone (or about something) in the inbox, and the person's own mails to
/// them from sent mail: the whole conversation, newest first.
fn find(acc: &Account, login: &Login, who: Option<&str>, about: Option<&str>, days: u32, count: usize) -> Result<Vec<Summary>, String> {
    let mut imap = open(acc, login)?;
    let mut out = matching(&mut imap, &acc.id, &criteria(days, "FROM", who, about), count, "inbox", 300)?;
    if let Some(sent) = sent_folder(&mut imap) {
        if imap.run(Cmd::Examine(&sent), 0).is_ok() {
            if let Ok(mine) = matching(&mut imap, &acc.id, &criteria(days, "TO", who, about), count, "sent", 300) {
                out.extend(mine);
            }
        }
    }
    imap.logout();
    Ok(newest_per_folder(out, count))
}

/// Newest first, at most `count` from each folder, so a busy inbox does not crowd out
/// the person's own sent mails (and the other way round).
fn newest_per_folder(mut all: Vec<Summary>, count: usize) -> Vec<Summary> {
    all.sort_by(|a, b| b.date.cmp(&a.date));
    let (mut inbox, mut sent) = (0, 0);
    all.retain(|m| {
        let n = if m.folder == "sent" { &mut sent } else { &mut inbox };
        *n += 1;
        *n <= count
    });
    all
}

/// The earlier mails of a conversation, found by Message-ID in the inbox and in sent
/// mail, oldest first.
fn thread(acc: &Account, login: &Login, ids: &[String]) -> Result<Vec<Summary>, String> {
    // The last few references are the nearest ones.
    let ids: Vec<&String> = ids.iter().rev().take(6).collect();
    let Some(crit) = ids.iter().map(|id| format!("HEADER Message-ID {}", search_text(id))).reduce(|later, earlier| format!("OR {earlier} {later}")) else {
        return Ok(Vec::new());
    };
    let mut imap = open(acc, login)?;
    let mut out = matching_bytes(&mut imap, &acc.id, &crit, 6, "inbox", 900, THREAD_BYTES).unwrap_or_default();
    if let Some(sent) = sent_folder(&mut imap) {
        if imap.run(Cmd::Examine(&sent), 0).is_ok() {
            out.extend(matching_bytes(&mut imap, &acc.id, &crit, 6, "sent", 900, THREAD_BYTES).unwrap_or_default());
        }
    }
    imap.logout();
    out.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(out)
}

fn read(acc: &Account, login: &Login, uid: u32, folder: &str) -> Result<Letter, String> {
    let mut imap = open(acc, login)?;
    if folder == "sent" {
        let sent = sent_folder(&mut imap).ok_or("this account has no sent mail folder")?;
        imap.run(Cmd::Examine(&sent), 0)?;
    }
    let replies = imap.run(Cmd::UidFetch(&uid.to_string(), READ_BYTES), READ_BYTES)?;
    imap.logout();
    let raw = replies.iter().find(|r| uid_of(&r.line) == Some(uid)).and_then(|r| r.literals.first()).ok_or("that mail is not in the inbox any more")?;
    let m = MessageParser::default().parse(raw).ok_or("that mail could not be read")?;
    let text = text_of(&m);
    Ok(Letter {
        from: addr(m.from()),
        to: addr(m.to()),
        subject: m.subject().unwrap_or("(no subject)").to_string(),
        date: m.date().map(|d| d.to_timestamp() * 1000),
        text,
        attachments: m.attachments().map(|a| a.attachment_name().unwrap_or("attachment").to_string()).collect(),
        message_id: m.message_id().map(str::to_string),
        references: {
            let mut refs: Vec<String> = m.references().as_text_list().unwrap_or_default().iter().map(|r| r.to_string()).collect();
            for r in m.in_reply_to().as_text_list().unwrap_or_default() {
                if !refs.iter().any(|x| x == r.as_ref()) {
                    refs.push(r.to_string());
                }
            }
            refs
        },
    })
}

// ---- accounts -------------------------------------------------------------------------

#[derive(Default)]
pub struct Mail {
    accounts: Mutex<Vec<Account>>,
    file: Mutex<Option<PathBuf>>,
    app: Mutex<Option<AppHandle>>,
    /// Access tokens from browser sign-ins, by account, until they run out. Memory only.
    tokens: Mutex<HashMap<String, (String, Instant)>>,
}

fn unseen_count<S: Read + Write>(imap: &mut Imap<S>) -> Result<usize, String> {
    let unseen = imap.run(Cmd::UidSearch("UNSEEN"), 0)?;
    Ok(unseen.iter().filter_map(|r| r.line.strip_prefix("* SEARCH")).map(|s| s.split_whitespace().count()).sum())
}

/// Move a password from the store's old name to the new one. The old entry goes only
/// once the new one reads back the same.
fn move_old_password(id: &str) {
    let (Ok(new), Ok(old)) = (secret(id), keyring::Entry::new(OLD_KEYRING, id)) else { return };
    if new.get_password().is_ok() {
        return;
    }
    let Ok(password) = old.get_password() else { return };
    if new.set_password(&password).is_ok() && new.get_password().ok().as_deref() == Some(password.as_str()) {
        let _ = old.delete_credential();
    }
}

fn secret(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(KEYRING, id).map_err(|e| format!("the credential store is not available: {e}"))
}

impl Mail {
    pub fn open(app: &AppHandle) -> Self {
        let file = app.path().app_data_dir().ok().map(|d| d.join("mail.json"));
        let accounts: Vec<Account> = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        for a in &accounts {
            move_old_password(&a.id);
        }
        Mail { accounts: Mutex::new(accounts), file: Mutex::new(file), app: Mutex::new(Some(app.clone())), tokens: Mutex::default() }
    }

    pub fn accounts(&self) -> Vec<Account> {
        self.accounts.lock().clone()
    }

    fn save(&self, app: &AppHandle) {
        let list = self.accounts();
        if let Some(f) = self.file.lock().as_ref() {
            if let Some(dir) = f.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(f, serde_json::to_vec_pretty(&list).unwrap_or_default());
        }
        let _ = app.emit("mail-accounts", list);
    }

    /// Check the sign-in works, then keep the account (the password goes to the
    /// credential store). Returns how many mails are unread.
    pub fn add(&self, app: &AppHandle, email: &str, password: &str, user: Option<String>, host: Option<String>, port: Option<u16>) -> Result<(Account, usize), String> {
        let email = email.trim().to_lowercase();
        if !email.contains('@') {
            return Err("that is not a mail address".into());
        }
        let p = provider(&email);
        if p.blocked && host.is_none() {
            return Err(p.hint.into());
        }
        let acc = Account {
            id: crate::routines::new_id(),
            host: host.filter(|h| !h.trim().is_empty()).map(|h| h.trim().to_string()).unwrap_or(p.host),
            port: port.unwrap_or(p.port),
            user: user.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| email.clone()),
            email,
            auth: None,
        };
        let password = password.replace(' ', "").trim().to_string();
        let mut imap = open(&acc, &Login::Password(password.clone()))?;
        let n = unseen_count(&mut imap)?;
        imap.logout();
        self.keep(app, &acc, &password)?;
        Ok((acc, n))
    }

    /// Sign in through Google or Microsoft in the browser. `hint` is the address typed in
    /// the form, if any, so the sign-in page can start on it.
    pub fn add_browser(&self, app: &AppHandle, kind: Kind, hint: Option<String>) -> Result<(Account, usize), String> {
        let iss = self.issuer(kind)?;
        let hint = hint.map(|h| h.trim().to_lowercase()).filter(|h| h.contains('@'));
        let t = oauth::sign_in(&iss, hint.as_deref(), |url| crate::open_outside(url).map_err(|e| format!("the browser did not open: {e}")))?;
        let email = t.email.clone().or(hint).ok_or("the sign-in did not say which address it is for; type the address and try again")?;
        let refresh = t.refresh.clone().ok_or_else(|| format!("{} gave no lasting sign-in; try again", kind.name()))?;
        let acc = Account { id: crate::routines::new_id(), host: kind.imap_host().into(), port: 993, user: email.clone(), email, auth: Some(kind) };
        let mut imap = open(&acc, &Login::Bearer(t.access.clone())).map_err(|e| {
            if kind == Kind::Google {
                format!("{e}. Check that the sign-in app may use Gmail (the scope https://mail.google.com/)")
            } else {
                format!("{e}. Work or school accounts may need IMAP switched on by their admin")
            }
        })?;
        let n = unseen_count(&mut imap)?;
        imap.logout();
        self.keep(app, &acc, &refresh)?;
        self.tokens.lock().insert(acc.id.clone(), (t.access, t.expires));
        Ok((acc, n))
    }

    fn issuer(&self, kind: Kind) -> Result<oauth::Issuer, String> {
        let app = self.app.lock().clone().ok_or("mail is not ready yet")?;
        let s = app.state::<crate::AppState>().settings.lock().clone();
        oauth::issuer(kind, &s).ok_or_else(|| format!("signing in with {} is not set up yet; see Settings, Mail", kind.name()))
    }

    /// Keep an account that just signed in: its password or refresh token goes to the
    /// credential store, and it replaces an older entry for the same address.
    fn keep(&self, app: &AppHandle, acc: &Account, kept: &str) -> Result<(), String> {
        secret(&acc.id)?.set_password(kept).map_err(|e| format!("the sign-in could not be stored safely: {e}"))?;
        {
            let mut list = self.accounts.lock();
            // Signing in again replaces the old entry for that address.
            if let Some(old) = list.iter().position(|a| a.email == acc.email) {
                let gone = list.remove(old);
                if let Ok(s) = secret(&gone.id) {
                    let _ = s.delete_credential();
                }
            }
            list.push(acc.clone());
        }
        self.save(app);
        Ok(())
    }

    pub fn remove(&self, app: &AppHandle, id: &str) {
        self.accounts.lock().retain(|a| a.id != id);
        self.tokens.lock().remove(id);
        if let Ok(s) = secret(id) {
            let _ = s.delete_credential();
        }
        self.save(app);
    }

    /// How an account signs in right now: its password, or an access token (refreshed
    /// when the last one ran out).
    fn login(&self, a: &Account) -> Result<Login, String> {
        let kept = secret(&a.id)?.get_password().map_err(|_| format!("the sign-in for {} is missing; sign in again in Settings", a.email))?;
        let Some(kind) = a.auth else { return Ok(Login::Password(kept)) };
        if let Some((t, until)) = self.tokens.lock().get(&a.id) {
            if Instant::now() < *until {
                return Ok(Login::Bearer(t.clone()));
            }
        }
        let t = oauth::refresh(&self.issuer(kind)?, &kept).map_err(|e| format!("{e}; sign in to {} again in Settings", a.email))?;
        if let Some(r) = t.refresh.as_deref().filter(|r| *r != kept) {
            let _ = secret(&a.id)?.set_password(r);
        }
        self.tokens.lock().insert(a.id.clone(), (t.access.clone(), t.expires));
        Ok(Login::Bearer(t.access))
    }

    fn pick(&self, which: Option<&str>) -> Result<Vec<(Account, Login)>, String> {
        let list = self.accounts();
        if list.is_empty() {
            return Err("no mail account is signed in; the person can add one in Settings, Mail".into());
        }
        let chosen: Vec<Account> = match which.map(str::trim).filter(|w| !w.is_empty()) {
            Some(w) => {
                let w = w.to_lowercase();
                let hit: Vec<Account> = list.into_iter().filter(|a| a.id == w || a.email.contains(&w)).collect();
                if hit.is_empty() {
                    return Err(format!("no signed-in account matches \"{w}\""));
                }
                hit
            }
            None => list,
        };
        chosen
            .into_iter()
            .map(|a| {
                let login = self.login(&a)?;
                Ok((a, login))
            })
            .collect()
    }

    /// The newest mail across accounts (or one), newest first.
    pub fn recent(&self, which: Option<&str>, count: usize, unread_only: bool) -> Result<Vec<Summary>, String> {
        let mut all = Vec::new();
        let mut errors = Vec::new();
        for (acc, pw) in self.pick(which)? {
            match list(&acc, &pw, count, unread_only) {
                Ok(l) => all.extend(l),
                Err(e) => errors.push(format!("{}: {e}", acc.email)),
            }
        }
        if all.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        all.sort_by(|a, b| b.date.cmp(&a.date));
        all.truncate(count);
        Ok(all)
    }

    pub fn letter(&self, account: &str, uid: u32, folder: &str) -> Result<Letter, String> {
        let (acc, pw) = self.pick(Some(account))?.into_iter().next().ok_or("no such account")?;
        read(&acc, &pw, uid, folder)
    }

    /// Mails from someone or about something, with the person's own mails to them, across
    /// accounts (or one), newest first.
    pub fn find(&self, which: Option<&str>, who: Option<&str>, about: Option<&str>, days: u32, count: usize) -> Result<Vec<Summary>, String> {
        let mut all = Vec::new();
        let mut errors = Vec::new();
        for (acc, login) in self.pick(which)? {
            match find(&acc, &login, who, about, days, count) {
                Ok(l) => all.extend(l),
                Err(e) => errors.push(format!("{}: {e}", acc.email)),
            }
        }
        if all.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(newest_per_folder(all, count))
    }

    /// The earlier mails a mail answers.
    pub fn thread(&self, account: &str, refs: &[String]) -> Result<Vec<Summary>, String> {
        let (acc, login) = self.pick(Some(account))?.into_iter().next().ok_or("no such account")?;
        thread(&acc, &login, refs)
    }

    /// Where an account's mail opens in the browser, and whether the link goes to the one
    /// mail asked for (Gmail can; the others open the inbox).
    pub fn web_link(&self, which: Option<&str>, message_id: Option<&str>) -> Result<(String, bool), String> {
        let list = self.accounts();
        let acc = match which.map(str::trim).filter(|w| !w.is_empty()) {
            Some(w) => list.iter().find(|a| a.id == w || a.email.contains(&w.to_lowercase())),
            None => list.first(),
        }
        .ok_or("no mail account is signed in; the person can add one in Settings, Mail")?;
        let d = domain(&acc.email);
        if acc.host == "imap.gmail.com" {
            let base = format!("https://mail.google.com/mail/u/{}/", oauth::enc(&acc.email));
            return Ok(match message_id {
                Some(id) => (format!("{base}#search/rfc822msgid%3A{}", oauth::enc(id)), true),
                None => (format!("{base}#inbox"), false),
            });
        }
        let web = if acc.host == "outlook.office365.com" {
            if OUTLOOK_HOME.contains(&d.as_str()) { "https://outlook.live.com/mail/0/inbox" } else { "https://outlook.office.com/mail/inbox" }
        } else {
            match d.as_str() {
                "gmx.de" | "gmx.net" | "gmx.at" | "gmx.ch" => "https://www.gmx.net/",
                "web.de" => "https://web.de/",
                "yahoo.com" | "yahoo.de" | "ymail.com" => "https://mail.yahoo.com/",
                "icloud.com" | "me.com" | "mac.com" => "https://www.icloud.com/mail",
                "posteo.de" | "posteo.net" => "https://posteo.de/webmail",
                "ovgu.de" | "st.ovgu.de" | "uni-magdeburg.de" | "st.uni-magdeburg.de" => "https://webmail.ovgu.de/",
                _ => return Err(format!("the web address of {}'s mail is not known; ask the person for it and use open_url", acc.email)),
            }
        };
        Ok((web.to_string(), false))
    }

    /// Where a draft opens: Gmail's or Outlook's web compose for those accounts, the
    /// mail app otherwise. The person sends it from there.
    pub fn draft_link(&self, which: Option<&str>, to: &str, subject: &str, body: &str) -> String {
        let list = self.accounts();
        let acc = match which {
            Some(w) => list.iter().find(|a| a.id == w || a.email.contains(&w.to_lowercase())),
            None => list.first(),
        };
        let e = oauth::enc;
        match acc {
            Some(a) if a.host == "imap.gmail.com" => format!("https://mail.google.com/mail/?view=cm&fs=1&to={}&su={}&body={}", e(to), e(subject), e(body)),
            Some(a) if a.host == "outlook.office365.com" => {
                let base = if OUTLOOK_HOME.contains(&domain(&a.email).as_str()) {
                    "https://outlook.live.com/mail/0/deeplink/compose"
                } else {
                    "https://outlook.office.com/mail/deeplink/compose"
                };
                format!("{base}?to={}&subject={}&body={}", e(to), e(subject), e(body))
            }
            _ => format!("mailto:{}?subject={}&body={}", e(to), e(subject), e(body)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_searches() {
        assert_eq!(last_name(r#"(\HasNoChildren \Sent) "/" "[Gmail]/Sent Mail""#), "[Gmail]/Sent Mail");
        assert_eq!(last_name(r#"(\HasNoChildren) "." INBOX.Sent"#), "INBOX.Sent");
        assert_eq!(last_name(r#"() "/" "say \"hi\"""#), "say \"hi\"");
        assert!(flagged("* 3 FETCH (UID 8 FLAGS (\\Seen \\Answered) BODY[]<0> {20}", "\\Answered"));
        assert!(!flagged("* 3 FETCH (UID 8 FLAGS (\\Seen) BODY[]<0> {20}", "\\Answered"));
        let c = criteria(30, "FROM", Some("ana\r\nj9 LOGOUT"), Some("Rechnung"));
        assert!(c.starts_with("SINCE ") && c.contains(r#"FROM "ana  j9 LOGOUT" TEXT "Rechnung""#), "{c}");
        assert!(criteria(30, "TO", Some("Jürgen"), None).starts_with("CHARSET UTF-8 SINCE "));
    }

    #[test]
    fn finds_the_sent_folder() {
        let replies = "* OK ready\r\n* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n* LIST (\\HasNoChildren \\Sent) \"/\" \"Gesendete Objekte\"\r\nj1 OK done\r\n";
        let mut held = Imap::greet(Script { replies: std::io::Cursor::new(replies.as_bytes().to_vec()), sent: Vec::new() }).unwrap();
        assert_eq!(sent_folder(&mut held).as_deref(), Some("Gesendete Objekte"));
        let replies = "* OK ready\r\n* LIST (\\HasNoChildren) \".\" INBOX.Sent\r\nj1 OK done\r\n";
        let mut held = Imap::greet(Script { replies: std::io::Cursor::new(replies.as_bytes().to_vec()), sent: Vec::new() }).unwrap();
        assert_eq!(sent_folder(&mut held).as_deref(), Some("INBOX.Sent"));
    }

    #[test]
    fn sent_mail_is_not_crowded_out() {
        let m = |uid: u32, date: i64, folder: &'static str| Summary {
            account: "a".into(), uid, from: String::new(), subject: String::new(), date: Some(date), unread: false,
            snippet: String::new(), folder, to: String::new(), answered: false, message_id: None,
        };
        // Three newer inbox mails, two older sent ones, two of each kept.
        let all = vec![m(1, 50, "inbox"), m(2, 40, "inbox"), m(3, 30, "inbox"), m(4, 20, "sent"), m(5, 10, "sent")];
        let kept: Vec<_> = newest_per_folder(all, 2).iter().map(|s| (s.uid, s.folder)).collect();
        assert_eq!(kept, [(1, "inbox"), (2, "inbox"), (4, "sent"), (5, "sent")]);
    }

    #[test]
    fn literals_and_flags() {
        assert_eq!(literal_len("* 3 FETCH (UID 812 FLAGS (\\Seen) BODY[]<0> {2048}"), Some(2048));
        assert_eq!(literal_len("* OK done"), None);
        assert_eq!(uid_of("* 3 FETCH (UID 812 FLAGS (\\Seen) BODY[]<0> {20}"), Some(812));
        assert!(!unread("* 3 FETCH (UID 812 FLAGS (\\Seen \\Answered) BODY[]<0> {20}"));
        assert!(unread("* 3 FETCH (UID 812 FLAGS () BODY[]<0> {20}"));
    }

    /// A server that answers from a script and keeps what it was sent.
    struct Script {
        replies: std::io::Cursor<Vec<u8>>,
        sent: Vec<u8>,
    }
    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.replies.read(buf)
        }
    }
    impl Write for Script {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.sent.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn lists_newest_unread_without_changing_anything() {
        let mail = |subject: &str| format!("From: Ana <ana@example.de>\r\nSubject: {subject}\r\nDate: Tue, 13 Oct 2026 09:30:00 +0200\r\n\r\nBody of {subject}\r\n");
        let (a, b) = (mail("First"), mail("Second"));
        let replies = format!(
            "* OK ready\r\n* SEARCH 5 9\r\nj1 OK done\r\n* 1 FETCH (UID 9 FLAGS () BODY[]<0> {{{}}}\r\n{b})\r\n* 2 FETCH (UID 5 FLAGS (\\Seen) BODY[]<0> {{{}}}\r\n{a})\r\nj2 OK done\r\n* BYE\r\nj3 OK bye\r\n",
            b.len(),
            a.len()
        );
        let mut held = Imap::greet(Script { replies: std::io::Cursor::new(replies.into_bytes()), sent: Vec::new() }).unwrap();
        let list = newest(&mut held, "acc", 10, true).unwrap();
        held.logout();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].uid, list[0].subject.as_str(), list[0].unread), (9, "Second", true));
        assert_eq!((list[1].uid, list[1].unread), (5, false));
        assert_eq!(list[0].snippet, "Body of Second");
        let sent = String::from_utf8(held.io.get_ref().sent.clone()).unwrap();
        for line in sent.lines() {
            let verb = line.split(' ').nth(1).unwrap_or_default();
            assert!(["UID", "LOGOUT"].contains(&verb), "unexpected command: {line}");
            assert!(!line.contains("STORE") && !line.contains("EXPUNGE") && !line.contains("BODY[]<"), "must only peek: {line}");
        }
        assert!(sent.contains("UID SEARCH UNSEEN SINCE"));
        assert!(sent.contains("UID FETCH 9,5 (UID FLAGS BODY.PEEK[]<0.24576>)"));
    }

    #[test]
    fn a_refused_token_ends_cleanly() {
        let replies = "* OK ready\r\n+ eyJzdGF0dXMiOiI0MDAifQ==\r\nj1 NO [AUTHENTICATIONFAILED] Invalid credentials (Failure)\r\n";
        let mut held = Imap::greet(Script { replies: std::io::Cursor::new(replies.as_bytes().to_vec()), sent: Vec::new() }).unwrap();
        let err = held.run(Cmd::XOauth2(&oauth::xoauth2("a@gmail.com", "old")), 0).err().unwrap();
        assert_eq!(err, "Invalid credentials (Failure)");
        let sent = String::from_utf8(held.io.get_ref().sent.clone()).unwrap();
        assert!(sent.starts_with("j1 AUTHENTICATE XOAUTH2 ") && sent.ends_with("\r\n\r\n"), "{sent:?}");
    }

    #[test]
    fn quoting_cannot_break_out() {
        assert_eq!(quote(r#"pa"ss\word"#), r#""pa\"ss\\word""#);
    }

    /// Reaches Gmail's server over TLS and gets a clean refusal for a made-up login.
    #[test]
    #[ignore = "needs the network"]
    fn live_refusal() {
        let acc = Account { id: "t".into(), email: "nobody@example.invalid".into(), host: "imap.gmail.com".into(), port: 993, user: "nobody@example.invalid".into(), auth: None };
        let err = open(&acc, &Login::Password("not-a-password".into())).err().expect("a made-up login must fail");
        println!("server said: {err}");
        assert!(err.starts_with("sign-in failed"), "{err}");
    }

    #[test]
    fn providers() {
        assert_eq!(provider("Me@Gmail.com").host, "imap.gmail.com");
        assert_eq!(provider("x@web.de").host, "imap.web.de");
        assert!(provider("x@outlook.com").blocked);
        assert_eq!(provider("x@hotmail.de").browser, Some(Kind::Microsoft));
        assert_eq!(provider("x@gmail.com").browser, Some(Kind::Google));
        assert_eq!(provider("x@web.de").browser, None);
        assert_eq!(provider("x@example.org").host, "imap.example.org");
        assert_eq!(provider("x@ovgu.de").host, "cyrus.ovgu.de");
        assert!(provider("x@st.ovgu.de").login.is_some());
    }

    #[test]
    fn parses_a_mail() {
        let raw = b"From: =?UTF-8?Q?J=C3=B6rg?= <jorg@example.de>\r\nTo: me@example.org\r\nSubject: Termin morgen\r\nDate: Tue, 13 Oct 2026 09:30:00 +0200\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHallo,\r\n\r\n\r\n\r\npasst 10 Uhr?\r\n> old quoted line\r\n";
        let m = MessageParser::default().parse(&raw[..]).unwrap();
        assert_eq!(addr(m.from()), "Jörg <jorg@example.de>");
        assert_eq!(body_text(&m), "Hallo,\n\npasst 10 Uhr?");
    }
}
