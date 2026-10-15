//! Web search and page reading, built in: no browser, no extension.
//!
//! Search goes to the person's own SearXNG if they set one, else Brave Search with
//! their key, else DuckDuckGo's plain HTML page, which needs no key at all.
//! Pages come back as readable text: no scripts, menus or styling.

use serde_json::Value;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
/// The most a page may weigh, and the most text handed to the model from it.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXT: usize = 14_000;
/// A web page is read in parts this long: every part read stays in the conversation,
/// and a local model reads about 200 tokens a second.
const PART: usize = 6_000;

#[derive(Debug, Clone)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Default)]
pub struct Search {
    pub searxng: Option<String>,
    pub brave_key: Option<String>,
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent(UA)
        .build()
        .into()
}

pub fn search(cfg: &Search, query: &str, count: usize) -> Result<(Vec<Hit>, &'static str), String> {
    let count = count.clamp(1, 10);
    if let Some(base) = cfg.searxng.as_deref().filter(|s| !s.trim().is_empty()) {
        return searxng(base, query, count).map(|h| (h, "SearXNG"));
    }
    if let Some(key) = cfg.brave_key.as_deref().filter(|s| !s.trim().is_empty()) {
        return brave(key, query, count).map(|h| (h, "Brave Search"));
    }
    duckduckgo(query, count).map(|h| (h, "DuckDuckGo"))
}

fn searxng(base: &str, query: &str, count: usize) -> Result<Vec<Hit>, String> {
    let mut res = agent()
        .get(format!("{}/search", base.trim_end_matches('/')))
        .query("q", query)
        .query("format", "json")
        .call()
        .map_err(|e| format!("SearXNG did not answer: {e}"))?;
    let v: Value = serde_json::from_str(&res.body_mut().read_to_string().map_err(|e| e.to_string())?).map_err(|_| "SearXNG did not send JSON (turn on the json format in its settings)")?;
    Ok(v.get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(count)
        .map(|r| Hit {
            title: r.get("title").and_then(Value::as_str).unwrap_or_default().into(),
            url: r.get("url").and_then(Value::as_str).unwrap_or_default().into(),
            snippet: r.get("content").and_then(Value::as_str).unwrap_or_default().into(),
        })
        .collect())
}

fn brave(key: &str, query: &str, count: usize) -> Result<Vec<Hit>, String> {
    let mut res = agent()
        .get("https://api.search.brave.com/res/v1/web/search")
        .query("q", query)
        .query("count", count.to_string())
        .header("Accept", "application/json")
        .header("X-Subscription-Token", key)
        .call()
        .map_err(|e| format!("Brave Search did not answer: {e}"))?;
    if res.status().as_u16() == 401 || res.status().as_u16() == 403 {
        return Err("Brave Search turned the key down".into());
    }
    let v: Value = serde_json::from_str(&res.body_mut().read_to_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(v.pointer("/web/results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(count)
        .map(|r| Hit {
            title: r.get("title").and_then(Value::as_str).map(strip_tags).unwrap_or_default(),
            url: r.get("url").and_then(Value::as_str).unwrap_or_default().into(),
            snippet: r.get("description").and_then(Value::as_str).map(strip_tags).unwrap_or_default(),
        })
        .collect())
}

fn duckduckgo(query: &str, count: usize) -> Result<Vec<Hit>, String> {
    let mut res = agent()
        .post("https://html.duckduckgo.com/html/")
        .header("Referer", "https://html.duckduckgo.com/")
        .send_form([("q", query), ("kl", "wt-wt")])
        .map_err(|e| format!("DuckDuckGo did not answer: {e}"))?;
    let html = res.body_mut().with_config().limit(MAX_BYTES).read_to_string().map_err(|e| e.to_string())?;
    if html.contains("anomaly-modal") || html.contains("Unfortunately, bots") {
        return Err("DuckDuckGo asked for a human check; add a Brave Search key or a SearXNG address in Settings".into());
    }
    let hits = ddg_hits(&html, count);
    if hits.is_empty() {
        return Err("DuckDuckGo found nothing for that".into());
    }
    Ok(hits)
}

/// The results on a DuckDuckGo HTML page, ads left out.
fn ddg_hits(html: &str, count: usize) -> Vec<Hit> {
    let mut hits = Vec::new();
    // Each result's body carries "result__body" somewhere in its class list.
    for block in html.split("result__body").skip(1) {
        let Some(a) = block.find("class=\"result__a\"") else { continue };
        let link = &block[a..];
        let href = attr(&block[..a + link.find('>').unwrap_or(0)], "href").unwrap_or_default();
        let title = link.find('>').and_then(|s| link[s + 1..].find("</a>").map(|e| &link[s + 1..s + 1 + e])).map(strip_tags).unwrap_or_default();
        let snippet = block
            .find("class=\"result__snippet\"")
            .and_then(|s| {
                let rest = &block[s..];
                let open = rest.find('>')?;
                let close = rest[open..].find("</a>").or_else(|| rest[open..].find("</div>"))?;
                Some(strip_tags(&rest[open + 1..open + close]))
            })
            .unwrap_or_default();
        let url = real_url(&decode_entities(&href));
        // Ads link through duckduckgo.com/y.js; they are not results.
        if url.is_empty() || url.contains("duckduckgo.com/y.js") {
            continue;
        }
        hits.push(Hit { title, url, snippet });
        if hits.len() >= count {
            break;
        }
    }
    hits
}

/// The value of `name="…"` in a tag.
fn attr(tag: &str, name: &str) -> Option<String> {
    let at = tag.rfind(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[at..].find('"')?;
    Some(tag[at..at + end].to_string())
}

/// DuckDuckGo wraps links as //duckduckgo.com/l/?uddg=<the real one>.
fn real_url(href: &str) -> String {
    if let Some(i) = href.find("uddg=") {
        let enc = &href[i + 5..];
        let enc = enc.split('&').next().unwrap_or(enc);
        return percent_decode(enc);
    }
    if href.starts_with("//") { format!("https:{href}") } else { href.to_string() }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => match b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 3;
                    continue;
                }
                None => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" | "#x27" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "rsquo" => Some('’'),
            "lsquo" => Some('‘'),
            "ldquo" => Some('“'),
            "rdquo" => Some('”'),
            n if n.starts_with("#x") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    decode_entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A web page as readable text, with its title.
/// Title and full text of a web page; `part` cuts it to size.
pub fn read(url: &str) -> Result<(String, String), String> {
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("only http and https pages can be read".into());
    }
    let mut res = agent().get(url).header("Accept", "text/html,text/plain,application/json;q=0.9,*/*;q=0.5").call().map_err(|e| format!("the page did not load: {e}"))?;
    let status = res.status().as_u16();
    if status >= 400 {
        return Err(format!("the site answered with error {status}"));
    }
    let kind = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if !(kind.is_empty() || kind.contains("text") || kind.contains("json") || kind.contains("xml")) {
        return Err(format!("that is not a page but a file ({kind})"));
    }
    let body = res.body_mut().with_config().limit(MAX_BYTES).read_to_string().map_err(|e| format!("the page could not be read: {e}"))?;
    if !kind.contains("html") && !body.trim_start().starts_with('<') {
        return Ok((String::new(), body));
    }
    Ok(page_text(&body))
}

/// Part `n` (from 1) of a page's text, and how many parts there are. Parts end at a
/// line break where one is near.
pub fn part(text: &str, n: usize) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut bounds = vec![0];
    while chars.len() - bounds[bounds.len() - 1] > PART {
        let end = bounds[bounds.len() - 1] + PART;
        let cut = (end - PART / 5..end).rev().find(|&i| chars[i] == '\n').map(|i| i + 1).unwrap_or(end);
        bounds.push(cut);
    }
    bounds.push(chars.len());
    let total = bounds.len() - 1;
    let n = n.clamp(1, total);
    (chars[bounds[n - 1]..bounds[n]].iter().collect(), total)
}

fn clip(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_TEXT).collect();
    format!("{kept}\n… [the rest of the page is left out]")
}

/// Title and text of an HTML page, without scripts, styles, menus and footers.
pub fn readable(html: &str) -> (String, String) {
    let (title, text) = page_text(html);
    (title, clip(&text))
}

fn page_text(html: &str) -> (String, String) {
    let title = html
        .find("<title")
        .and_then(|s| {
            let open = html[s..].find('>')? + s + 1;
            let close = html[open..].find("</title>")? + open;
            Some(strip_tags(&html[open..close]))
        })
        .unwrap_or_default();
    // Prefer the main content when the page marks it.
    let lower = html.to_lowercase();
    let start = ["<main", "<article", "<body"].iter().find_map(|t| lower.find(t)).unwrap_or(0);
    let mut s = html[start..].to_string();
    for tag in ["script", "style", "noscript", "svg", "nav", "footer", "header", "form", "iframe", "template", "aside"] {
        s = drop_element(&s, tag);
    }
    let mut out = String::with_capacity(s.len() / 3);
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < s.len() {
        if bytes[i] == b'<' {
            let end = s[i..].find('>').map(|e| i + e + 1).unwrap_or(s.len());
            let tag = s[i + 1..end.saturating_sub(1)].trim_start_matches('/').to_lowercase();
            let name: String = tag.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
            match name.as_str() {
                "p" | "div" | "br" | "tr" | "section" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "ul" | "ol" | "table" | "blockquote" | "pre" => out.push('\n'),
                "li" => out.push_str("\n- "),
                "td" | "th" => out.push_str(" | "),
                _ => out.push(' '),
            }
            i = end;
        } else {
            let next = s[i..].find('<').map(|n| i + n).unwrap_or(s.len());
            out.push_str(&decode_entities(&s[i..next]));
            i = next;
        }
    }
    let text = out
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty() && l != "-" && l != "|")
        .collect::<Vec<_>>()
        .join("\n");
    (title, text)
}

fn drop_element(html: &str, tag: &str) -> String {
    let lower = html.to_lowercase();
    let (open, close) = (format!("<{tag}"), format!("</{tag}>"));
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(s) = lower[at..].find(&open).map(|s| s + at) {
        // "<header" must not match "<headerline": the next character ends the name.
        let after = lower.as_bytes().get(s + open.len()).copied().unwrap_or(b'>');
        if after.is_ascii_alphanumeric() || after == b'-' {
            out.push_str(&html[at..s + open.len()]);
            at = s + open.len();
            continue;
        }
        out.push_str(&html[at..s]);
        at = match lower[s..].find(&close) {
            Some(e) => s + e + close.len(),
            None => lower[s..].find('>').map(|e| s + e + 1).unwrap_or(html.len()),
        };
    }
    out.push_str(&html[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ddg_results() {
        let page = r#"<div class="serp__results"><div class="result results_links results_links_deep web-result ">
<div class="links_main links_deep result__body"><h2 class="result__title">
<a rel="nofollow" class="result__a" href="https://v2.tauri.app/release/">Tauri <b>2</b> releases</a></h2>
<a class="result__snippet" href="https://v2.tauri.app/release/">All the <b>release</b> notes.</a></div></div>
<div class="result results_links web-result result--ad"><div class="links_main links_deep result__body">
<a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad=1">Buy now</a></div></div>
<div class="result web-result"><div class="links_main result__body"><h2 class="result__title">
<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=x">Example</a></h2></div></div></div>"#;
        let hits = ddg_hits(page, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://v2.tauri.app/release/");
        assert_eq!(hits[0].title, "Tauri 2 releases");
        assert_eq!(hits[0].snippet, "All the release notes.");
        assert_eq!(hits[1].url, "https://example.com/");
    }

    #[test]
    fn decodes_ddg_links() {
        assert_eq!(real_url("//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1&amp;rut=x"), "https://example.com/a?b=1");
        assert_eq!(percent_decode("a%20b+c%zz"), "a b c%zz");
    }

    #[test]
    fn reads_pages() {
        let (t, x) = readable("<html><head><title>Hi &amp; bye</title><style>p{}</style></head><body><nav>menu</nav><main><h1>Head</h1><p>One&nbsp;two</p><ul><li>a</li><li>b</li></ul><script>x()</script></main><footer>f</footer></body></html>");
        assert_eq!(t, "Hi & bye");
        assert_eq!(x, "Head\nOne two\n- a\n- b");
    }

    #[test]
    fn reads_in_parts() {
        assert_eq!(part("short", 1), ("short".to_string(), 1));
        let line = "x".repeat(99) + "\n";
        let text = line.repeat(130); // 13,000 characters
        let (first, total) = part(&text, 1);
        assert_eq!(total, 3);
        assert_eq!(first.len(), 6_000);
        assert!(first.ends_with('\n'));
        let all: String = (1..=total).map(|n| part(&text, n).0).collect();
        assert_eq!(all, text);
        assert_eq!(part(&text, 9).0, part(&text, 3).0);
    }
}
