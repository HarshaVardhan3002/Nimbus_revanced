//! Keeping a conversation inside the model's context window (125 000 tokens unless
//! Settings says otherwise).
//!
//! Nimbus mostly runs short, tool-heavy tasks, so what fills the window is screenshots
//! and tool output, not long prose. Compaction goes from cheap to costly and stops as
//! soon as the conversation fits:
//!
//! 1. Always: only the two newest screenshots stay; older ones become a one-line note.
//! 2. Old tool results are cut to their head and tail (the recent ones stay whole).
//! 3. The oldest turns are summarised by the model into a few hundred words that keep
//!    goals, facts and open tasks; the summary rides along in the system prompt.
//! 4. Should one message alone still not fit, it is cut down. Nothing ever fails for
//!    being too long.

use serde_json::{Value, json};

/// A rough token count: about four characters a token, and a fixed price per picture.
const CHARS_PER_TOKEN: usize = 4;
const IMAGE_TOKENS: usize = 1500;
const MESSAGE_TOKENS: usize = 6;

/// Compaction starts when the conversation passes this share of the window, leaving
/// room for the tools list and the reply.
pub const START_AT: f32 = 0.70;
/// What the newest turns, kept word for word, may take after a summary.
pub const KEEP_RECENT: f32 = 0.30;
/// Screenshots kept as pictures.
const KEEP_IMAGES: usize = 2;
/// Tool results in the newest this-many messages stay whole.
const KEEP_TOOLS_WHOLE: usize = 8;

pub fn tokens(v: &Value) -> usize {
    match v {
        Value::String(s) => s.len() / CHARS_PER_TOKEN,
        Value::Array(parts) => parts.iter().map(tokens).sum(),
        Value::Object(o) => {
            if o.get("type").and_then(Value::as_str) == Some("image_url") {
                return IMAGE_TOKENS;
            }
            o.iter().filter(|(k, _)| *k != "role").map(|(_, v)| tokens(v)).sum::<usize>()
        }
        _ => 1,
    }
}

pub fn estimate(messages: &[Value]) -> usize {
    messages.iter().map(|m| tokens(m) + MESSAGE_TOKENS).sum()
}

fn has_image(m: &Value) -> bool {
    m.get("content").and_then(Value::as_array).is_some_and(|parts| parts.iter().any(|p| p.get("type").and_then(Value::as_str) == Some("image_url")))
}

/// Step 1: only the newest screenshots stay as pictures.
pub fn drop_old_images(messages: &mut [Value]) {
    let mut seen = 0;
    for m in messages.iter_mut().rev() {
        if !has_image(m) {
            continue;
        }
        seen += 1;
        if seen <= KEEP_IMAGES {
            continue;
        }
        if let Some(parts) = m.get_mut("content").and_then(Value::as_array_mut) {
            for p in parts.iter_mut() {
                if p.get("type").and_then(Value::as_str) == Some("image_url") {
                    *p = json!({ "type": "text", "text": "[an older screenshot was here]" });
                }
            }
        }
    }
}

/// The model turned pictures down: every picture becomes a note. True if any was there.
pub fn drop_all_images(messages: &mut [Value]) -> bool {
    let mut any = false;
    for m in messages.iter_mut().filter(|m| has_image(m)) {
        if let Some(parts) = m.get_mut("content").and_then(Value::as_array_mut) {
            for p in parts.iter_mut().filter(|p| p.get("type").and_then(Value::as_str) == Some("image_url")) {
                *p = json!({ "type": "text", "text": "[a screen picture, left out: this model cannot see pictures]" });
                any = true;
            }
        }
    }
    any
}

fn cut(text: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= head + tail + 40 {
        return text.to_string();
    }
    let h: String = chars[..head].iter().collect();
    let t: String = chars[chars.len() - tail..].iter().collect();
    format!("{h}\n… [{} characters left out to save room] …\n{t}", chars.len() - head - tail)
}

/// Step 2: old tool results down to their head and tail.
pub fn trim_old_tools(messages: &mut [Value]) {
    let n = messages.len();
    for m in messages.iter_mut().take(n.saturating_sub(KEEP_TOOLS_WHOLE)) {
        if m.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        if let Some(Value::String(s)) = m.get_mut("content") {
            if s.len() > 900 {
                *s = cut(s, 400, 200);
            }
        }
    }
}

/// A user message the person typed (or a routine's task), as opposed to the
/// screenshots the harness attaches after a tool.
pub fn is_turn_start(messages: &[Value], i: usize) -> bool {
    messages[i].get("role").and_then(Value::as_str) == Some("user")
        && (i == 0 || messages[i - 1].get("role").and_then(Value::as_str) != Some("tool"))
}

/// Where the kept tail begins: the earliest turn start whose tail fits `room` tokens,
/// never later than the newest turn start. `None` when there is nothing older to
/// summarise.
pub fn split_point(messages: &[Value], room: usize) -> Option<usize> {
    let starts: Vec<usize> = (1..messages.len()).filter(|&i| is_turn_start(messages, i)).collect();
    let newest = *starts.last()?;
    let pick = starts.iter().copied().find(|&i| estimate(&messages[i..]) <= room).unwrap_or(newest);
    (pick > 1).then_some(pick)
}

/// The old turns as a plain transcript for the summariser: tool output and pictures
/// shortened, since the summary only needs what they showed.
pub fn transcript(messages: &[Value]) -> String {
    let mut out = String::new();
    for m in messages {
        let role = m.get("role").and_then(Value::as_str).unwrap_or("?");
        let text = match m.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .map(|p| match p.get("type").and_then(Value::as_str) {
                    Some("text") => p.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
                    _ => "[screenshot]".into(),
                })
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        match role {
            "tool" => out.push_str(&format!("TOOL RESULT: {}\n", cut(&text, 300, 100))),
            "assistant" => {
                if !text.trim().is_empty() {
                    out.push_str(&format!("ASSISTANT: {}\n", cut(&text, 1200, 300)));
                }
                for c in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    let name = c.pointer("/function/name").and_then(Value::as_str).unwrap_or("?");
                    let args = c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("");
                    out.push_str(&format!("ASSISTANT USED {name}({})\n", cut(args, 200, 0)));
                }
            }
            _ => out.push_str(&format!("{}: {}\n", role.to_uppercase(), cut(&text, 2000, 400))),
        }
    }
    out
}

pub const SUMMARY_PROMPT: &str = "You compress the history of an assistant that works on a person's computer. \
Write a summary the assistant can rely on instead of the old messages. Keep: what the person asked for and \
their preferences; what was done, in one line per task, with the outcome; facts worth reusing (names, \
addresses, numbers, file paths, links, times); anything still open or promised. Drop greetings, \
reasoning and the raw tool output. Use short bullet points under 350 words. If an earlier summary is given, \
merge it in.";

/// Step 4: a single message too long for the window is cut down, newest kept whole.
pub fn squeeze(messages: &mut [Value], budget: usize) {
    let n = messages.len();
    for i in 1..n.saturating_sub(1) {
        if estimate(messages) <= budget {
            return;
        }
        if let Some(Value::String(s)) = messages[i].get_mut("content") {
            if s.len() > 4000 {
                *s = cut(s, 1500, 500);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_give_way_to_notes() {
        let picture = |n: u32| json!({ "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{n}") } });
        let mut m = vec![
            json!({ "role": "user", "content": "what is on my screen?" }),
            json!({ "role": "user", "content": [{ "type": "text", "text": "pictures" }, picture(1), picture(2)] }),
            json!({ "role": "user", "content": [picture(3)] }),
        ];
        drop_old_images(&mut m);
        assert!(m.iter().filter(|x| has_image(x)).count() >= 1);
        assert!(drop_all_images(&mut m));
        assert!(!m.iter().any(has_image));
        assert_eq!(m[1]["content"][0]["text"], "pictures");
        assert!(!drop_all_images(&mut m));
    }
}
