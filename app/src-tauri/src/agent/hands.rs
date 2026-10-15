//! The agent's hands, by name. A look reads an app window's buttons, fields, links and
//! text from the system's accessibility tree (Windows UI Automation) and numbers the
//! controls; an action works on one number. Most actions go through the app's own
//! accessibility actions (press, set the text, tick, open), so the person's mouse
//! stays where it is. Only a control without one gets a real click on its centre, and
//! the pointer goes back where it was. A control keeps its number while its window
//! stays open, so a number from an older look still means the same control.
//!
//! No pictures are needed, so this works with a model that cannot see. A model that can
//! see also gets a picture of the window with each look, and can click a spot in it
//! ("click_spot") for things the accessibility tree has no name for.

use serde_json::Value;
use std::collections::{HashMap, HashSet};

/// What one look shows at most: numbered controls, and lines of plain text around them.
const MAX_CONTROLS: usize = 120;
/// After a click or typing, the answer lists only what changed, at most this many.
const MAX_BRIEF: usize = 50;
/// Fewer named controls than this, and the words do not show the window: a picture goes
/// with the look (a game, a canvas, a remote desktop).
const THIN: usize = 8;
const MAX_TEXTS: usize = 60;
/// The longest name or value shown for one control.
const MAX_LABEL: usize = 80;

/// The window and control a step is about, for the safety rules and the step's title.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Target {
    /// The window's title.
    pub window: String,
    /// The app the window belongs to, as the system names it.
    pub app: String,
    /// "terminal", "web", "chat", "secret" (password managers, Windows Security) or "".
    pub kind: &'static str,
    pub role: String,
    pub name: String,
    pub password: bool,
}

impl Target {
    /// The facts the safety rules read, added to a step's arguments under "_" names.
    pub fn add_to(&self, args: &mut Value) {
        if let Some(o) = args.as_object_mut() {
            o.insert("_window".into(), self.window.clone().into());
            o.insert("_app".into(), self.app.clone().into());
            o.insert("_kind".into(), self.kind.into());
            o.insert("_role".into(), self.role.clone().into());
            o.insert("_name".into(), self.name.clone().into());
            o.insert("_password".into(), self.password.into());
        }
    }
}

/// What kind of window this is, from its class, app and title.
pub fn kind_of(class: &str, app: &str, title: &str) -> &'static str {
    let app = app.to_lowercase();
    let title = title.to_lowercase();
    let any = |words: &[&str]| words.iter().any(|w| app.contains(w) || title.contains(w));
    if any(&["1password", "bitwarden", "keepass", "dashlane", "lastpass", "proton pass", "windows security", "windows-sicherheit", "credential"]) {
        "secret"
    } else if matches!(class, "ConsoleWindowClass" | "CASCADIA_HOSTING_WINDOW_CLASS" | "PseudoConsoleWindow" | "mintty" | "VirtualConsoleClass")
        || any(&["windowsterminal", "alacritty", "wezterm", "tabby", "conemu"])
    {
        // By the window's kind: a PowerShell script's own window is no terminal.
        "terminal"
    } else if any(&["whatsapp", "teams", "outlook", "thunderbird", "slack", "discord", "telegram", "signal", "messenger", "mail"]) {
        "chat"
    } else if matches!(class, "Chrome_WidgetWin_1" | "MozillaWindowClass") {
        // Browsers and apps built on them: their fields want real typing.
        "web"
    } else {
        ""
    }
}

fn clean(text: &str) -> String {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= MAX_LABEL {
        return t;
    }
    let cut: String = t.chars().take(MAX_LABEL).collect();
    format!("{cut}…")
}

/// Plain words for a UI Automation control type, and whether it is something to act on.
fn role(t: i32) -> Option<(&'static str, bool)> {
    Some(match t {
        50000 => ("button", true),
        50002 => ("check box", true),
        50003 => ("combo box", true),
        50004 => ("text field", true),
        50005 => ("link", true),
        50007 => ("list item", true),
        50011 => ("menu item", true),
        50013 => ("radio button", true),
        50015 => ("slider", true),
        50016 => ("spinner", true),
        50019 => ("tab", true),
        50024 => ("tree item", true),
        50029 => ("item", true),
        50031 => ("split button", true),
        50035 => ("column header", true),
        50020 => ("text", false),
        50006 => ("image", false),
        50030 => ("document", false),
        50025 => ("control", false),
        50026 => ("group", false),
        50033 => ("pane", false),
        50037 => ("title bar", false),
        _ => return None,
    })
}

/// One line of a look, before numbering.
#[derive(Debug, Clone, Default)]
struct Line {
    role: &'static str,
    name: String,
    value: Option<String>,
    /// Something to act on: gets a number.
    control: bool,
    states: Vec<&'static str>,
    /// In the part of the window that has the keyboard (an open form or dialog).
    near: bool,
}

/// A control's identity between two looks at the same window.
fn key(l: &Line) -> String {
    format!("{}|{}", l.role, l.name)
}

/// Each line's identity, with controls of the same role and name told apart by their
/// order ("Not starred" on every row of a mail list).
fn idents(lines: &[Line]) -> Vec<String> {
    let mut count: HashMap<String, usize> = HashMap::new();
    lines
        .iter()
        .map(|l| {
            let k = key(l);
            let n = count.entry(k.clone()).or_default();
            *n += 1;
            format!("{k}#{n}")
        })
        .collect()
}

/// The numbers given out in one window: the same control gets the same number in
/// every look, and a new one the next free number. Kept until another window is looked at.
#[derive(Default)]
pub struct Numbers {
    given: HashMap<String, usize>,
    next: usize,
}

impl Numbers {
    fn of(&mut self, key: &str) -> usize {
        if let Some(n) = self.given.get(key) {
            return *n;
        }
        self.next += 1;
        self.given.insert(key.to_string(), self.next);
        self.next
    }
}

/// Rows of a list or table repeat one name over several parts (the row, its link, its
/// tick box): a nameless-looking "item" next to a named button, link or box of the same
/// name adds nothing.
fn repeats(lines: &[Line], i: usize) -> bool {
    let l = &lines[i];
    if !matches!(l.role, "item" | "list item") || l.name.is_empty() {
        return false;
    }
    let same = |j: usize| lines.get(j).is_some_and(|o| o.control && o.name == l.name);
    // Another item of the same name just before, or any other part of the row around it.
    (i > 0 && same(i - 1)) || (same(i + 1) && !matches!(lines[i + 1].role, "item" | "list item")) || (same(i + 1) && same(i + 2))
}

/// The text of a look, and each shown control's number with its place in `lines`.
/// `before` holds the controls of the last look at the same window: what is new since
/// then (a form that just opened) is listed first, then the part of the window that has
/// the keyboard, so neither is cut off on a long page. `brief` (the answer after a click
/// or typing) lists only those two: the rest is as the model saw it, with the same numbers.
/// Every word costs the model time to read, on every later step too.
fn render(title: &str, app: &str, lines: &[Line], find: Option<&str>, before: &HashSet<String>, numbers: &mut Numbers, brief: bool) -> (String, Vec<(usize, usize)>) {
    let ids = idents(lines);
    let find = find.map(|f| f.to_lowercase()).filter(|f| !f.trim().is_empty());
    let text_of = |l: &Line| format!("{} {}", l.name, l.value.as_deref().unwrap_or("")).to_lowercase();
    // All the words together, else any of them.
    let mut hits: Vec<bool> = lines.iter().map(|l| find.as_ref().is_none_or(|f| text_of(l).contains(f.as_str()))).collect();
    let mut loose = false;
    if let Some(f) = &find {
        if !lines.iter().zip(&hits).any(|(l, h)| *h && l.control) {
            let words: Vec<&str> = f.split_whitespace().filter(|w| w.chars().count() >= 3).collect();
            hits = lines.iter().map(|l| words.iter().any(|w| text_of(l).contains(w))).collect();
            loose = true;
        }
    }
    let wanted = |i: usize| hits[i] && !(lines[i].control && repeats(lines, i));
    let fresh = |i: usize| !before.is_empty() && lines[i].control && !before.contains(&ids[i]);
    let any_fresh = (0..lines.len()).any(|i| wanted(i) && fresh(i));
    let near = |i: usize| lines[i].near && !(any_fresh && fresh(i));
    let any_near = (0..lines.len()).any(|i| wanted(i) && near(i) && lines[i].control);

    let mut out = format!("Window \"{title}\"{}. Numbered controls can be used with act; a control keeps its number while the window is open.\n", if app.is_empty() { String::new() } else { format!(" ({app})") });
    if loose {
        out.push_str("Nothing has all of those words; these have some of them:\n");
    }
    let mut shown = Vec::new();
    let (mut texts, mut hidden) = (0, 0);
    let cap = if brief { MAX_BRIEF } else { MAX_CONTROLS };
    let mut line_of = |i: usize, out: &mut String, shown: &mut Vec<(usize, usize)>, last: &mut String| {
        let l = &lines[i];
        if l.control {
            if shown.len() >= cap {
                hidden += 1;
                return;
            }
            let n = numbers.of(&ids[i]);
            shown.push((n, i));
            let mut line = format!("[{n}] {}", l.role);
            if !l.name.is_empty() {
                line.push_str(&format!(" \"{}\"", l.name));
            }
            if let Some(v) = l.value.as_ref().filter(|v| !v.is_empty() && **v != l.name) {
                line.push_str(&format!(" = \"{v}\""));
            }
            if !l.states.is_empty() {
                line.push_str(&format!(" ({})", l.states.join(", ")));
            }
            out.push_str(&line);
            out.push('\n');
            *last = l.name.clone();
        } else if !l.name.is_empty() && l.name != *last && texts < MAX_TEXTS {
            texts += 1;
            out.push_str(&format!("- {}\n", l.name));
            *last = l.name.clone();
        }
    };
    let mut last = String::new();
    if any_fresh {
        out.push_str("New since the last look:\n");
        for i in (0..lines.len()).filter(|&i| wanted(i) && fresh(i)) {
            line_of(i, &mut out, &mut shown, &mut last);
        }
    }
    if any_near {
        out.push_str("Where the keyboard is:\n");
        for i in (0..lines.len()).filter(|&i| wanted(i) && near(i)) {
            line_of(i, &mut out, &mut shown, &mut last);
        }
    }
    let rest = |i: usize| wanted(i) && !(any_fresh && fresh(i)) && !(any_near && near(i));
    if brief {
        let others = (0..lines.len()).filter(|&i| rest(i) && lines[i].control).count();
        if any_fresh || any_near {
            out.push_str(&format!("(The other {others} controls are as before, with the same numbers; see_window lists them all.)\n"));
        } else {
            out.push_str("Nothing new appeared. The numbers from the last look still work; see_window lists the window again.\n");
        }
        if hidden > 0 {
            out.push_str(&format!("({hidden} more new controls not shown: look with see_window and find to narrow it down.)\n"));
        }
        return (out, shown);
    }
    if any_fresh || any_near {
        out.push_str("The rest of the window:\n");
    }
    for i in (0..lines.len()).filter(|&i| rest(i)) {
        line_of(i, &mut out, &mut shown, &mut last);
    }
    if shown.is_empty() {
        out.push_str(match find {
            Some(_) => "Nothing here matches those words, so this look has no numbers. Look again without find, or with other words.\n",
            None => "No buttons or fields could be read in this window.\n",
        });
    }
    if hidden > 0 {
        out.push_str(&format!("({hidden} more controls not shown: look again with find to narrow it down.)\n"));
    }
    (out, shown)
}

pub use imp::{act, keys, look, reset, spot_target, target, window_target};

/// The window's own buttons (title bar). Nimbus leaves them alone: minimising or closing
/// a window is the person's to do. English and German names.
const FRAME: &[&str] = &["minimise", "minimize", "maximise", "maximize", "restore", "close", "system", "minimieren", "maximieren", "wiederherstellen", "schließen"];

/// Roles that say nothing without a name: left out of a look when they have none.
fn says_nothing(role: &str) -> bool {
    matches!(role, "group" | "control" | "pane" | "item" | "list item" | "image" | "text" | "document" | "title bar")
}

#[cfg(windows)]
mod imp {
    use super::{FRAME, Line, Numbers, Target, clean, idents, kind_of, render, role, says_nothing};
    use std::collections::{HashMap, HashSet};
    use crate::agent::screen::{self, Frame};
    use enigo::{Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
    use parking_lot::Mutex;
    use std::sync::OnceLock;
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;
    use windows::Win32::Foundation::{HWND, POINT, RECT};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_BSTR, VT_I4, VariantClear};
    use windows::Win32::UI::Accessibility::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow, WindowFromPoint,
    };
    use windows::core::BSTR;

    struct Win {
        hwnd: isize,
        title: String,
        app: String,
        kind: &'static str,
        /// A browser, or an app built on one: its fields want real typing.
        web: bool,
    }

    struct Item {
        el: IUIAutomationElement,
        role: &'static str,
        name: String,
        rect: RECT,
        enabled: bool,
        password: bool,
        invoke: bool,
        value: bool,
        read_only: bool,
        toggle: bool,
        expand: Option<i32>,
        select: bool,
        scroll: bool,
    }

    struct Hands {
        uia: IUIAutomation,
        win: Option<Win>,
        /// The controls of the newest look, by number.
        items: HashMap<usize, Item>,
        numbers: Numbers,
        /// Pictures may go with a look (the model can see, and seeing the screen is on).
        pictures: bool,
        /// A picture was asked for (see_window's picture, or click_spot work); without it a
        /// picture goes only with a window whose words say too little. A picture costs a
        /// local model far more time to read than the list of controls.
        want_picture: bool,
        /// The newest picture of the window, for click_spot.
        shot: Option<Frame>,
        /// The controls of the newest look and its window, to tell what is new in the next.
        seen: HashSet<String>,
        seen_in: isize,
    }

    type Job = Box<dyn FnOnce(&mut Option<Hands>) + Send>;
    static WORKER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

    /// Run on the one thread that holds the accessibility connection.
    fn call<T: Send + 'static>(f: impl FnOnce(&mut Hands) -> Result<T, String> + Send + 'static) -> Result<T, String> {
        let tx = WORKER.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            std::thread::Builder::new()
                .name("hands".into())
                .spawn(move || {
                    unsafe {
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    let mut hands = None;
                    for job in rx {
                        job(&mut hands);
                    }
                })
                .expect("the hands thread starts");
            Mutex::new(tx)
        });
        let (rtx, rrx) = channel();
        let job: Job = Box::new(move |h: &mut Option<Hands>| {
            if h.is_none() {
                match unsafe { CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
                    Ok(uia) => *h = Some(Hands { uia, win: None, items: HashMap::new(), numbers: Numbers::default(), pictures: false, want_picture: false, shot: None, seen: HashSet::new(), seen_in: 0 }),
                    Err(e) => {
                        let _ = rtx.send(Err(format!("Windows accessibility did not start: {e}")));
                        return;
                    }
                }
            }
            let _ = rtx.send(f(h.as_mut().expect("set above")));
        });
        tx.lock().send(job).map_err(|_| "the hands stopped working".to_string())?;
        rrx.recv_timeout(Duration::from_secs(30)).map_err(|_| "the app did not answer in time".to_string())?
    }

    /// Whether a click here would land on one of Nimbus's own windows (the island and its
    /// panel stay on top of everything): the agent never presses its own buttons, Allow
    /// above all. See-through windows such as the overlay are passed over, as the click
    /// would pass through them. The web view inside a window belongs to another process,
    /// so the top-level window is the one compared.
    fn over_nimbus(x: i32, y: i32) -> bool {
        unsafe {
            let hit = WindowFromPoint(POINT { x, y });
            if hit.is_invalid() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(GetAncestor(hit, GA_ROOT), Some(&mut pid));
            pid == std::process::id()
        }
    }

    /// Real mouse and keyboard input, unless switched off for testing with the
    /// NIMBUS_NO_REAL_INPUT environment variable.
    fn real_input() -> Result<(), String> {
        if std::env::var_os("NIMBUS_NO_REAL_INPUT").is_some() {
            return Err("real mouse and keyboard input is switched off for testing".into());
        }
        Ok(())
    }

    fn hwnd(h: isize) -> HWND {
        HWND(h as *mut core::ffi::c_void)
    }

    fn class_of(h: isize) -> String {
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd(h), &mut buf) };
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }

    /// App windows from front to back, Nimbus's own left out.
    fn windows() -> Vec<Win> {
        let me = std::process::id();
        xcap::Window::all()
            .unwrap_or_default()
            .into_iter()
            .filter(|w| w.pid().ok() != Some(me) && !w.is_minimized().unwrap_or(true))
            .filter(|w| w.width().unwrap_or(0) >= 80 && w.height().unwrap_or(0) >= 40)
            .filter_map(|w| {
                let title = w.title().ok().filter(|t| !t.trim().is_empty())?;
                let h = w.id().ok()? as isize;
                let app = w.app_name().unwrap_or_default();
                let class = class_of(h);
                let kind = kind_of(&class, &app, &title);
                let web = matches!(class.as_str(), "Chrome_WidgetWin_1" | "MozillaWindowClass");
                Some(Win { hwnd: h, title, app, kind, web })
            })
            .collect()
    }

    /// The window whose title or app has all these words, or the one in front.
    fn pick(want: Option<&str>) -> Result<Win, String> {
        let all = windows();
        let words: Vec<String> = want.unwrap_or("").to_lowercase().split_whitespace().map(String::from).collect();
        let found = all.iter().position(|w| {
            let hay = format!("{} {}", w.title, w.app).to_lowercase();
            words.iter().all(|x| hay.contains(x))
        });
        match found {
            Some(i) => Ok(all.into_iter().nth(i).expect("found")),
            None if all.is_empty() => Err("no app windows are open".into()),
            None => {
                let names: Vec<String> = all.iter().take(15).map(|w| format!("\"{}\"", w.title)).collect();
                Err(format!("no window matches \"{}\". Open windows: {}", want.unwrap_or(""), names.join(", ")))
            }
        }
    }

    fn prop(el: &IUIAutomationElement, id: UIA_PROPERTY_ID) -> Option<VARIANT> {
        unsafe { el.GetCachedPropertyValue(id).ok() }
    }

    fn text(el: &IUIAutomationElement, id: UIA_PROPERTY_ID) -> Option<String> {
        let mut v = prop(el, id)?;
        let out = unsafe {
            let inner = &v.Anonymous.Anonymous;
            (inner.vt == VT_BSTR).then(|| inner.Anonymous.bstrVal.to_string())
        };
        unsafe {
            let _ = VariantClear(&mut v);
        }
        out
    }

    fn int(el: &IUIAutomationElement, id: UIA_PROPERTY_ID) -> Option<i32> {
        let v = prop(el, id)?;
        unsafe {
            let inner = &v.Anonymous.Anonymous;
            (inner.vt == VT_I4).then(|| inner.Anonymous.lVal)
        }
    }

    fn flag(el: &IUIAutomationElement, id: UIA_PROPERTY_ID) -> bool {
        prop(el, id).is_some_and(|v| unsafe {
            let inner = &v.Anonymous.Anonymous;
            inner.vt == VT_BOOL && inner.Anonymous.boolVal.0 != 0
        })
    }

    const PROPS: &[UIA_PROPERTY_ID] = &[
        UIA_NamePropertyId,
        UIA_ControlTypePropertyId,
        UIA_BoundingRectanglePropertyId,
        UIA_IsOffscreenPropertyId,
        UIA_IsEnabledPropertyId,
        UIA_IsPasswordPropertyId,
        UIA_HasKeyboardFocusPropertyId,
        UIA_IsInvokePatternAvailablePropertyId,
        UIA_IsValuePatternAvailablePropertyId,
        UIA_ValueValuePropertyId,
        UIA_ValueIsReadOnlyPropertyId,
        UIA_IsTogglePatternAvailablePropertyId,
        UIA_ToggleToggleStatePropertyId,
        UIA_IsExpandCollapsePatternAvailablePropertyId,
        UIA_ExpandCollapseExpandCollapseStatePropertyId,
        UIA_IsSelectionItemPatternAvailablePropertyId,
        UIA_SelectionItemIsSelectedPropertyId,
        UIA_IsScrollPatternAvailablePropertyId,
    ];

    impl Hands {
        fn look_at(&mut self, w: Win, find: Option<&str>, brief: bool) -> Result<String, String> {
            if w.kind == "secret" {
                return Err(format!("\"{}\" keeps passwords or security settings; Nimbus does not look into it", w.title));
            }
            let (mut lines, mut items, mut page) = self.read(&w)?;
            // Browsers switch their page's accessibility on only when first asked, so the
            // first look at one shows just its frame: look again once it has woken up.
            for _ in 0..4 {
                if !w.web || page {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
                (lines, items, page) = self.read(&w)?;
            }
            let before = if self.seen_in == w.hwnd {
                std::mem::take(&mut self.seen)
            } else {
                // Another window: its own numbers, from 1.
                self.numbers = Numbers::default();
                HashSet::new()
            };
            let (mut out, _) = render(&w.title, &w.app, &lines, find, &before, &mut self.numbers, brief);
            let thin = lines.iter().filter(|l| l.control && !l.name.is_empty()).count() < super::THIN;
            self.seen = idents(&lines).into_iter().zip(&lines).filter(|(_, l)| l.control).map(|(k, _)| k).collect();
            self.seen_in = w.hwnd;
            // Every control gets its number, also those the list had no room for: a number
            // seen in an earlier look keeps working while its control is there.
            let ids = idents(&lines);
            self.items = items
                .into_iter()
                .enumerate()
                .filter_map(|(i, it)| it.map(|it| (self.numbers.of(&ids[i]), it)))
                .collect();
            self.shot = None;
            if self.pictures && (self.want_picture || thin) {
                match screen::take_window(w.hwnd as u32) {
                    Ok(f) => {
                        out.push_str(
                            "A picture of this window follows. To click something that has no number, use act with action \
click_spot and x, y on a grid from 0,0 (top left) to 1000,1000 (bottom right) of the picture.\n",
                        );
                        self.shot = Some(f);
                    }
                    Err(e) => out.push_str(&format!("(No picture of the window: {e}.)\n")),
                }
            }
            self.win = Some(w);
            Ok(out)
        }

        /// The window's accessibility tree: lines to show, the controls behind them, and
        /// whether a page (a document) was among them.
        #[allow(clippy::type_complexity)]
        fn read(&self, w: &Win) -> Result<(Vec<Line>, Vec<Option<Item>>, bool), String> {
            let e = |e: windows::core::Error| format!("the window could not be read: {}", e.message());
            let mut page = false;
            let (lines, items) = unsafe {
                let root = self.uia.ElementFromHandle(hwnd(w.hwnd)).map_err(e)?;
                let cache = self.uia.CreateCacheRequest().map_err(e)?;
                for p in PROPS {
                    cache.AddProperty(*p).map_err(e)?;
                }
                let cond = self.uia.ControlViewCondition().map_err(e)?;
                let all = root.FindAllBuildCache(TreeScope_Descendants, &cond, &cache).map_err(e)?;
                let n = all.Length().unwrap_or(0);
                // The title bar: the top of the window, as tall as Windows draws it at this size.
                let whole = root.CurrentBoundingRectangle().unwrap_or_default();
                let top = if whole.bottom > whole.top { whole.top } else { i32::MIN };
                let bar = top.saturating_add(64);
                // The keyboard's area is one stretch of the list (descendants come in tree
                // order): from its first descendant to its last.
                let area = self.keyboard_area(&root, whole);
                let same = |a: &IUIAutomationElement, b: &IUIAutomationElement| self.uia.CompareElements(a, b).is_ok_and(|s| s.as_bool());
                let (mut near, mut passed) = (false, false);
                let mut lines = Vec::new();
                let mut items = Vec::new();
                for i in 0..n {
                    let Ok(el) = all.GetElement(i) else { continue };
                    if let Some((first, last)) = area.as_ref().filter(|_| !passed) {
                        if near && i > 0 && all.GetElement(i - 1).is_ok_and(|p| same(&p, last)) {
                            (near, passed) = (false, true);
                        } else if !near && same(&el, first) {
                            near = true;
                        }
                    }
                    if el.CachedIsOffscreen().map(|b| b.as_bool()).unwrap_or(true) {
                        continue;
                    }
                    let rect = el.CachedBoundingRectangle().unwrap_or_default();
                    if rect.right <= rect.left || rect.bottom <= rect.top {
                        continue;
                    }
                    let t = el.CachedControlType().map(|t| t.0).unwrap_or(0);
                    page |= t == 50030;
                    let name = clean(&el.CachedName().map(|n| n.to_string()).unwrap_or_default());
                    if matches!(t, 50000 | 50011) && rect.bottom <= bar && FRAME.contains(&name.to_lowercase().as_str()) {
                        continue;
                    }
                    let invoke = flag(&el, UIA_IsInvokePatternAvailablePropertyId);
                    let value = flag(&el, UIA_IsValuePatternAvailablePropertyId);
                    let toggle = flag(&el, UIA_IsTogglePatternAvailablePropertyId);
                    let expandable = flag(&el, UIA_IsExpandCollapsePatternAvailablePropertyId);
                    let select = flag(&el, UIA_IsSelectionItemPatternAvailablePropertyId);
                    let (role_name, actionable) = role(t).unwrap_or(("control", false));
                    let control = actionable || invoke || toggle || select || (value && !matches!(t, 50030 | 50037));
                    if !control && (name.is_empty() || !matches!(t, 50020 | 50006)) {
                        continue;
                    }
                    let password = el.CachedIsPassword().map(|b| b.as_bool()).unwrap_or(false);
                    let enabled = el.CachedIsEnabled().map(|b| b.as_bool()).unwrap_or(true);
                    let read_only = flag(&el, UIA_ValueIsReadOnlyPropertyId);
                    let scroll = flag(&el, UIA_IsScrollPatternAvailablePropertyId);
                    let expand = expandable.then(|| int(&el, UIA_ExpandCollapseExpandCollapseStatePropertyId)).flatten();
                    let mut states = Vec::new();
                    if !enabled {
                        states.push("greyed out");
                    }
                    if el.CachedHasKeyboardFocus().map(|b| b.as_bool()).unwrap_or(false) {
                        states.push("focused");
                    }
                    if toggle {
                        states.push(match int(&el, UIA_ToggleToggleStatePropertyId) {
                            Some(1) => "on",
                            Some(2) => "partly on",
                            _ => "off",
                        });
                    }
                    match expand {
                        Some(0) => states.push("closed"),
                        Some(1) | Some(2) => states.push("open"),
                        _ => {}
                    }
                    if select && flag(&el, UIA_SelectionItemIsSelectedPropertyId) {
                        states.push("selected");
                    }
                    if password {
                        states.push("password");
                    }
                    let shown_value = if password || !value { None } else { text(&el, UIA_ValueValuePropertyId).map(|v| clean(&v)) };
                    // Nameless groups and boxes cannot be told apart; a picture shows them.
                    if name.is_empty() && shown_value.as_deref().unwrap_or("").is_empty() && says_nothing(role_name) {
                        continue;
                    }
                    lines.push(Line { role: role_name, name: name.clone(), value: shown_value, control, states, near });
                    items.push(control.then(|| Item {
                        el,
                        role: role_name,
                        name,
                        rect,
                        enabled,
                        password,
                        invoke,
                        value,
                        read_only,
                        toggle,
                        expand,
                        select,
                        scroll,
                    }));
                }
                (lines, items)
            };
            Ok((lines, items, page))
        }

        /// A spot in the newest picture, on the desktop. Spots are given on a grid of 1,000
        /// by 1,000 over the picture, whatever its size: the way Qwen-family models point,
        /// and far steadier with Bonsai 2 than asking for pixels (which it mixed up with
        /// the grid in a test).
        fn spot(&self, x: f64, y: f64) -> Result<(i32, i32), String> {
            let f = self.shot.as_ref().ok_or("there is no picture of the window to point in; look with see_window and picture true first")?;
            if !(0.0..=1000.0).contains(&x) || !(0.0..=1000.0).contains(&y) {
                return Err(format!("{x},{y} is off the grid; x and y go from 0 to 1000"));
            }
            Ok(f.to_screen(x / 1000.0 * f.w as f64, y / 1000.0 * f.h as f64))
        }

        /// What the control at a spot in the newest picture is.
        fn at_spot(&self, x: f64, y: f64) -> Result<Target, String> {
            let (sx, sy) = self.spot(x, y)?;
            let w = self.win.as_ref().ok_or("look at a window first")?;
            let el = unsafe { self.uia.ElementFromPoint(POINT { x: sx, y: sy }) }.map_err(|e| e.message().to_string())?;
            let (name, t, password) = unsafe {
                (
                    el.CurrentName().map(|n| clean(&n.to_string())).unwrap_or_default(),
                    el.CurrentControlType().map(|t| t.0).unwrap_or(0),
                    el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false),
                )
            };
            let role = role(t).map(|r| r.0).unwrap_or("control");
            Ok(Target { window: w.title.clone(), app: w.app.clone(), kind: w.kind, role: role.into(), name, password })
        }

        /// The part of the window around the control with the keyboard: the dialog it is
        /// in, else the largest box around it that covers at most half the window (Gmail's
        /// new mail box over the inbox), as its first and last descendant. None when the
        /// keyboard is in another window, or nothing is focused.
        fn keyboard_area(&self, root: &IUIAutomationElement, whole: RECT) -> Option<(IUIAutomationElement, IUIAutomationElement)> {
            let size = |r: &RECT| (r.right - r.left).max(0) as i64 * (r.bottom - r.top).max(0) as i64;
            let half = size(&whole) / 2;
            unsafe {
                let walker = self.uia.ControlViewWalker().ok()?;
                let mut at = self.uia.GetFocusedElement().ok()?;
                let (mut dialog, mut best) = (None, None);
                for _ in 0..40 {
                    let up = walker.GetParentElement(&at).ok()?;
                    if self.uia.CompareElements(&up, root).is_ok_and(|same| same.as_bool()) {
                        let area: IUIAutomationElement = dialog.or(best)?;
                        let first = walker.GetFirstChildElement(&area).ok()?;
                        let mut last = walker.GetLastChildElement(&area).ok()?;
                        while let Ok(deeper) = walker.GetLastChildElement(&last) {
                            last = deeper;
                        }
                        return Some((first, last));
                    }
                    let r = up.CurrentBoundingRectangle().unwrap_or_default();
                    if dialog.is_none() && size(&r) > 0 {
                        let aria = up.CurrentAriaRole().map(|s| s.to_string()).unwrap_or_default();
                        if matches!(aria.as_str(), "dialog" | "alertdialog") || up.CurrentControlType().is_ok_and(|t| t.0 == 50032) {
                            dialog = Some(up.clone());
                        } else if size(&r) <= half {
                            best = Some(up.clone());
                        }
                    }
                    at = up;
                }
                None
            }
        }

        /// After typing into `el`: the name of what has the keyboard instead, if the focus
        /// left the field. A suggestion that shows the typed words (Gmail's address list)
        /// counts as the field.
        fn focus_left(&self, el: &IUIAutomationElement, typed: &str) -> Option<String> {
            unsafe {
                let f = self.uia.GetFocusedElement().ok()?;
                let walker = self.uia.ControlViewWalker().ok()?;
                let mut at = Some(f.clone());
                for _ in 0..12 {
                    let Some(e) = at else { break };
                    if self.uia.CompareElements(&e, el).is_ok_and(|same| same.as_bool()) {
                        return None;
                    }
                    at = walker.GetParentElement(&e).ok();
                }
                let name = clean(&f.CurrentName().map(|n| n.to_string()).unwrap_or_default());
                let shown = typed.trim().to_lowercase();
                if !shown.is_empty() && name.to_lowercase().contains(&shown) {
                    return None;
                }
                Some(if name.is_empty() { "another control".into() } else { name })
            }
        }

        fn item(&self, n: usize) -> Result<&Item, String> {
            self.items.get(&n).ok_or_else(|| format!("number {n} is not in the window now; look at it again"))
        }

        fn front(&self) -> Result<(), String> {
            let w = self.win.as_ref().ok_or("look at a window first")?;
            real_input()?;
            if on_screen(w.hwnd).is_none() {
                return Err(gone(&w.title));
            }
            unsafe {
                if !IsWindow(Some(hwnd(w.hwnd))).as_bool() {
                    return Err(format!("\"{}\" was closed", w.title));
                }
                if GetForegroundWindow() == hwnd(w.hwnd) {
                    return Ok(());
                }
                let _ = SetForegroundWindow(hwnd(w.hwnd));
                if GetForegroundWindow() != hwnd(w.hwnd) {
                    if let Ok(root) = self.uia.ElementFromHandle(hwnd(w.hwnd)) {
                        let _ = root.SetFocus();
                    }
                }
                std::thread::sleep(Duration::from_millis(150));
                if GetForegroundWindow() != hwnd(w.hwnd) {
                    return Err(format!("\"{}\" could not be brought to the front", w.title));
                }
            }
            Ok(())
        }

        /// A real click on a spot, with the pointer put back after.
        fn click_at(&self, x: i32, y: i32, button: Button, times: usize) -> Result<(), String> {
            self.front()?;
            if over_nimbus(x, y) {
                return Err("that spot is covered by Nimbus's own window; Nimbus never clicks itself".into());
            }
            let mut enigo = real_input().and_then(|_| Enigo::new(&Settings::default()).map_err(|e| e.to_string())).map_err(|e| format!("the mouse could not be used: {e}"))?;
            let back = enigo.location().ok();
            enigo.move_mouse(x, y, Coordinate::Abs).map_err(|e| e.to_string())?;
            for _ in 0..times {
                enigo.button(button, Direction::Click).map_err(|e| e.to_string())?;
                std::thread::sleep(Duration::from_millis(60));
            }
            if let Some((bx, by)) = back {
                let _ = enigo.move_mouse(bx, by, Coordinate::Abs);
            }
            Ok(())
        }

        fn act(&mut self, n: usize, action: &str, typed: Option<String>, spot: Option<(f64, f64)>, point: &dyn Fn(i32, i32, bool)) -> Result<String, String> {
            if action == "click_spot" {
                let (x, y) = spot.ok_or("click_spot needs x and y on the 0 to 1000 grid")?;
                let what = self.at_spot(x, y)?;
                if what.password && typed.is_some() {
                    return Err("Nimbus never types into password fields; the person types passwords themselves".into());
                }
                let (sx, sy) = self.spot(x, y)?;
                point(sx, sy, true);
                self.click_at(sx, sy, Button::Left, 1)?;
                let label = if what.name.is_empty() { format!("the {} at {x:.0},{y:.0}", what.role) } else { format!("\"{}\"", what.name) };
                // Typing after the click goes to whatever the click put the keyboard on.
                if let Some(t) = typed {
                    std::thread::sleep(Duration::from_millis(150));
                    let mut enigo = real_input().and_then(|_| Enigo::new(&Settings::default()).map_err(|e| e.to_string())).map_err(|e| format!("the keyboard could not be used: {e}"))?;
                    enigo.text(&t).map_err(|e| e.to_string())?;
                    return Ok(format!("Clicked {label} and typed {} characters.", t.chars().count()));
                }
                return Ok(format!("Clicked {label}."));
            }
            let it = self.item(n)?;
            let label = if it.name.is_empty() { format!("the {}", it.role) } else { format!("\"{}\"", it.name) };
            if !it.enabled && action != "focus" {
                return Err(format!("{label} is greyed out right now"));
            }
            let (cx, cy) = ((it.rect.left + it.rect.right) / 2, (it.rect.top + it.rect.bottom) / 2);
            let e = |e: windows::core::Error| format!("{label} did not respond: {}", e.message());
            let el = it.el.clone();
            let done = unsafe {
                match action {
                    "click" => {
                        point(cx, cy, true);
                        if it.invoke {
                            el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId).and_then(|p| p.Invoke()).map_err(e)?;
                        } else if it.select {
                            el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId).and_then(|p| p.Select()).map_err(e)?;
                        } else if it.toggle {
                            el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId).and_then(|p| p.Toggle()).map_err(e)?;
                        } else if let Some(state) = it.expand {
                            let p = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).map_err(e)?;
                            if state == 0 { p.Expand() } else { p.Collapse() }.map_err(e)?;
                        } else {
                            self.click_at(cx, cy, Button::Left, 1)?;
                        }
                        format!("Clicked {label}.")
                    }
                    "double_click" | "right_click" => {
                        point(cx, cy, true);
                        let (button, times) = if action == "right_click" { (Button::Right, 1) } else { (Button::Left, 2) };
                        self.click_at(cx, cy, button, times)?;
                        format!("{} {label}.", if action == "right_click" { "Right-clicked" } else { "Double-clicked" })
                    }
                    "type" => {
                        if it.password {
                            return Err("Nimbus never types into password fields; the person types passwords themselves".into());
                        }
                        let typed = typed.ok_or("say what to type in text")?;
                        let mut moved = String::new();
                        point(cx, cy, false);
                        let web = self.win.as_ref().is_some_and(|w| w.web);
                        if it.value && !it.read_only && !web {
                            el.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId).and_then(|p| p.SetValue(&BSTR::from(typed.as_str()))).map_err(e)?;
                        } else {
                            // Real typing: the field gets the keyboard, its text is replaced.
                            self.front()?;
                            el.SetFocus().map_err(e)?;
                            std::thread::sleep(Duration::from_millis(80));
                            let mut enigo = real_input().and_then(|_| Enigo::new(&Settings::default()).map_err(|e| e.to_string())).map_err(|e| format!("the keyboard could not be used: {e}"))?;
                            // An empty field is not select-all'ed: in Gmail's "To" box ctrl+a
                            // jumps to the address chips, and the typing is lost.
                            let has_text = el
                                .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                                .and_then(|p| p.CurrentValue())
                                .map(|v| !v.is_empty())
                                .unwrap_or(true);
                            if has_text {
                                super::press(&mut enigo, "ctrl+a")?;
                            }
                            enigo.text(&typed).map_err(|e| e.to_string())?;
                            std::thread::sleep(Duration::from_millis(150));
                            if let Some(other) = self.focus_left(&el, &typed) {
                                moved = format!(
                                    " But the keyboard moved from {label} to \"{other}\" on the way, so the text may not be in {label}: check it in the window below."
                                );
                            }
                        }
                        format!("Typed {} characters into {label}.{moved}", typed.chars().count())
                    }
                    "toggle" => {
                        point(cx, cy, true);
                        el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId).and_then(|p| p.Toggle()).map_err(e)?;
                        format!("Switched {label}.")
                    }
                    "expand" | "collapse" => {
                        point(cx, cy, true);
                        let p = el.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(UIA_ExpandCollapsePatternId).map_err(e)?;
                        if action == "expand" { p.Expand() } else { p.Collapse() }.map_err(e)?;
                        format!("{} {label}.", if action == "expand" { "Opened" } else { "Closed" })
                    }
                    "select" => {
                        point(cx, cy, true);
                        el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId).and_then(|p| p.Select()).map_err(e)?;
                        format!("Chose {label}.")
                    }
                    "scroll_down" | "scroll_up" => {
                        let amount = if action == "scroll_down" { ScrollAmount_LargeIncrement } else { ScrollAmount_LargeDecrement };
                        if it.scroll {
                            el.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId).and_then(|p| p.Scroll(ScrollAmount_NoAmount, amount)).map_err(e)?;
                        } else {
                            point(cx, cy, false);
                            self.front()?;
                            let mut enigo = real_input().and_then(|_| Enigo::new(&Settings::default()).map_err(|e| e.to_string())).map_err(|e| format!("the mouse could not be used: {e}"))?;
                            let back = enigo.location().ok();
                            enigo.move_mouse(cx, cy, Coordinate::Abs).map_err(|e| e.to_string())?;
                            enigo.scroll(if action == "scroll_down" { 5 } else { -5 }, enigo::Axis::Vertical).map_err(|e| e.to_string())?;
                            if let Some((bx, by)) = back {
                                let _ = enigo.move_mouse(bx, by, Coordinate::Abs);
                            }
                        }
                        format!("Scrolled {} in {label}.", if action == "scroll_down" { "down" } else { "up" })
                    }
                    "focus" => {
                        el.SetFocus().map_err(e)?;
                        format!("Put the keyboard on {label}.")
                    }
                    other => return Err(format!("there is no action called \"{other}\"")),
                }
            };
            Ok(done)
        }

        /// Look again at the window of the newest look, if it is still open.
        fn again(&mut self) -> String {
            let Some(w) = self.win.take() else { return String::new() };
            std::thread::sleep(Duration::from_millis(450));
            let alive = unsafe { IsWindow(Some(hwnd(w.hwnd))).as_bool() };
            let next = if alive { windows().into_iter().find(|x| x.hwnd == w.hwnd).unwrap_or(w) } else { return format!("\n\n\"{}\" is closed now.", w.title) };
            match self.look_at(next, None, true) {
                Ok(text) => format!("\n\nThe window now:\n{text}"),
                Err(e) => format!("\n\nThe window could not be read again: {e}."),
            }
        }
    }

    /// Look at a window. With `pictures`, a picture of it comes with the words.
    pub fn look(window: Option<String>, find: Option<String>, pictures: bool, picture: bool) -> Result<(String, Option<Frame>), String> {
        call(move |h| {
            h.pictures = pictures;
            h.want_picture = picture;
            let w = match (window.as_deref(), &h.win) {
                (Some(want), _) => pick(Some(want))?,
                // Without a name: the window this request works in, not whatever came to
                // the front meanwhile (the person may have switched to their own work).
                (None, Some(old)) => on_screen(old.hwnd).ok_or_else(|| gone(&old.title))?,
                (None, None) => pick(None)?,
            };
            let text = h.look_at(w, find.as_deref(), false)?;
            Ok((text, h.shot.clone()))
        })
    }

    /// The window, if it is still open, not minimised and on this desktop.
    fn on_screen(h: isize) -> Option<Win> {
        windows().into_iter().find(|w| w.hwnd == h)
    }

    fn gone(title: &str) -> String {
        format!(
            "\"{title}\" is not on screen any more (closed, minimised or on another desktop). Stop and tell the person; \
do not carry on in another window."
        )
    }

    /// A new request: no window yet.
    pub fn reset() {
        let _ = call(|h| {
            h.win = None;
            h.shot = None;
            Ok(())
        });
    }

    /// What the control at a spot in the newest picture is.
    pub fn spot_target(x: f64, y: f64) -> Option<Target> {
        call(move |h| h.at_spot(x, y)).ok()
    }

    /// What number `n` of the newest look is, in which window.
    pub fn target(n: usize) -> Option<Target> {
        call(move |h| {
            let w = h.win.as_ref().ok_or("no look yet")?;
            let it = h.item(n)?;
            Ok(Target { window: w.title.clone(), app: w.app.clone(), kind: w.kind, role: it.role.into(), name: it.name.clone(), password: it.password })
        })
        .ok()
    }

    /// The window keys would go to: the named one, else the one of the newest look, else
    /// the one in front.
    pub fn window_target(window: Option<String>) -> Option<Target> {
        call(move |h| {
            let w = match (window.as_deref(), &h.win) {
                (Some(want), _) => pick(Some(want))?,
                (None, Some(w)) => Win { hwnd: w.hwnd, title: w.title.clone(), app: w.app.clone(), kind: w.kind, web: w.web },
                (None, None) => pick(None)?,
            };
            Ok(Target { window: w.title, app: w.app, kind: w.kind, ..Default::default() })
        })
        .ok()
    }

    /// Act on number `n` of the newest look; the answer ends with a fresh look.
    pub fn act(n: usize, action: String, typed: Option<String>, spot: Option<(f64, f64)>, pictures: bool, point: Box<dyn Fn(i32, i32, bool) + Send>) -> Result<(String, Option<Frame>), String> {
        call(move |h| {
            h.pictures = pictures;
            // Working by picture (click_spot) goes on with pictures; the rest in words.
            h.want_picture = action == "click_spot";
            let done = h.act(n, &action, typed, spot, &*point)?;
            Ok((format!("{done}{}", h.again()), h.shot.clone()))
        })
    }

    /// Press keys in a window, brought to the front first.
    pub fn keys(combo: String, window: Option<String>, pictures: bool) -> Result<(String, Option<Frame>), String> {
        call(move |h| {
            h.pictures = pictures;
            h.want_picture = false;
            if let Some(want) = window.as_deref() {
                let w = pick(Some(want))?;
                if w.kind == "secret" {
                    return Err(format!("\"{}\" keeps passwords or security settings; Nimbus does not type there", w.title));
                }
                h.win = Some(w);
            } else if h.win.is_none() {
                h.win = Some(pick(None)?);
            }
            h.front()?;
            let mut enigo = real_input().and_then(|_| Enigo::new(&Settings::default()).map_err(|e| e.to_string())).map_err(|e| format!("the keyboard could not be used: {e}"))?;
            super::press(&mut enigo, &combo)?;
            Ok((format!("Pressed {combo}.{}", h.again()), h.shot.clone()))
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Target;
    use crate::agent::screen::Frame;

    const NOT_YET: &str = "clicking by name works on Windows for now";

    pub fn look(_window: Option<String>, _find: Option<String>, _pictures: bool, _picture: bool) -> Result<(String, Option<Frame>), String> {
        Err(NOT_YET.into())
    }
    pub fn spot_target(_x: f64, _y: f64) -> Option<Target> {
        None
    }
    pub fn reset() {}
    pub fn target(_n: usize) -> Option<Target> {
        None
    }
    pub fn window_target(_window: Option<String>) -> Option<Target> {
        None
    }
    pub fn act(_n: usize, _action: String, _typed: Option<String>, _spot: Option<(f64, f64)>, _pictures: bool, _point: Box<dyn Fn(i32, i32, bool) + Send>) -> Result<(String, Option<Frame>), String> {
        Err(NOT_YET.into())
    }
    pub fn keys(_combo: String, _window: Option<String>, _pictures: bool) -> Result<(String, Option<Frame>), String> {
        Err(NOT_YET.into())
    }
}

fn key_named(name: &str) -> Result<enigo::Key, String> {
    use enigo::Key;
    let n = name.trim().to_lowercase();
    Ok(match n.as_str() {
        "ctrl" | "control" | "strg" => Key::Control,
        "alt" | "option" => Key::Alt,
        "shift" => Key::Shift,
        "win" | "windows" | "meta" | "cmd" | "command" | "super" => Key::Meta,
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "esc" | "escape" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" | "entf" => Key::Delete,
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
        // On Windows a letter or digit goes as its real key, so ctrl+a is a shortcut. Sent
        // as a character (Unicode), Chrome took ctrl+a for something else and moved the
        // focus out of Gmail's "To" field.
        #[cfg(windows)]
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_alphanumeric() => Key::Other(s.as_bytes()[0].to_ascii_uppercase() as u32),
        s if s.chars().count() == 1 => Key::Unicode(s.chars().next().unwrap_or(' ')),
        other => return Err(format!("there is no key called \"{other}\"")),
    })
}

/// Press a combination like "ctrl+shift+t": modifiers down, the key, all up again.
fn press(enigo: &mut enigo::Enigo, combo: &str) -> Result<(), String> {
    use enigo::{Direction, Keyboard};
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

#[cfg(test)]
mod tests {
    /// Prints a window's controls (not its text, digits masked): PROBE_WINDOW names it.
    #[test]
    #[ignore]
    fn probe_window() {
        let want = std::env::var("PROBE_WINDOW").unwrap_or_else(|_| "WhatsApp".into());
        let find = std::env::var("PROBE_FIND").ok();
        let (out, _) = super::look(Some(want), find, false, false).expect("look");
        for line in out.lines() {
            let control = ["] button", "] edit", "] tab", "] link", "] checkbox", "] menu", "] combo", "] list "].iter().any(|k| line.contains(k));
            if control || !line.starts_with('[') {
                let masked: String = line.chars().map(|c| if c.is_ascii_digit() && !line.starts_with('[') { '#' } else { c }).collect();
                println!("{}", masked.chars().take(110).collect::<String>());
            } else {
                println!("(text, {} chars)", line.len());
            }
        }
    }

    use super::*;

    fn line(role: &'static str, name: &str, control: bool) -> Line {
        Line { role, name: name.into(), control, ..Default::default() }
    }

    #[test]
    fn numbers_controls_only() {
        let lines = vec![line("text", "Save your work?", false), line("button", "Save", true), line("button", "Cancel", true)];
        let none = HashSet::new();
        let (out, shown) = render("Editor", "notepad", &lines, None, &none, &mut Numbers::default(), false);
        assert!(out.contains("- Save your work?\n[1] button \"Save\"\n[2] button \"Cancel\""), "{out}");
        assert_eq!(shown, vec![(1, 1), (2, 2)]);
        let (out, shown) = render("Editor", "", &lines, Some("canc"), &none, &mut Numbers::default(), false);
        assert!(out.contains("[1] button \"Cancel\""), "{out}");
        assert_eq!(shown, vec![(1, 2)]);
        // No control has all the words: any of them will do.
        let (out, shown) = render("Editor", "", &lines, Some("cancel button"), &none, &mut Numbers::default(), false);
        assert!(out.contains("some of them") && out.contains("[1] button \"Cancel\""), "{out}");
        assert_eq!(shown, vec![(1, 2)]);
        let (out, shown) = render("Editor", "", &lines, Some("zzz"), &none, &mut Numbers::default(), false);
        assert!(out.contains("no numbers"), "{out}");
        assert!(shown.is_empty());
    }

    #[test]
    fn new_controls_first_and_rows_once() {
        let row = "Anna, Lunch on Friday";
        let lines = vec![
            line("button", "Compose", true),
            line("item", row, true),
            line("check box", row, true),
            line("link", row, true),
            line("text field", "To recipients", true),
            line("button", "Send", true),
        ];
        // The first look, before the form opened.
        let mut numbers = Numbers::default();
        let (out, _) = render("Gmail", "", &lines[..4], None, &HashSet::new(), &mut numbers, false);
        assert!(out.contains("[1] button \"Compose\"\n[2] check box"), "{out}");
        let before: HashSet<String> = idents(&lines[..4]).into_iter().collect();
        let (out, shown) = render("Gmail", "", &lines, None, &before, &mut numbers, false);
        // Old controls keep their numbers; the new ones come first, with the next ones.
        assert!(out.contains("New since the last look:\n[4] text field \"To recipients\"\n[5] button \"Send\"\nThe rest of the window:\n[1] button \"Compose\""), "{out}");
        // The row's item is left out next to its tick box and link.
        assert_eq!(shown, vec![(4, 4), (5, 5), (1, 0), (2, 2), (3, 3)]);
        // After a click, only the change: the rest keeps the numbers the model has.
        let (out, shown) = render("Gmail", "", &lines, None, &before, &mut numbers, true);
        assert!(out.contains("[5] button \"Send\"
(The other 3 controls are as before"), "{out}");
        assert!(!out.contains("Compose"), "{out}");
        assert_eq!(shown, vec![(4, 4), (5, 5)]);
        let all: HashSet<String> = idents(&lines).into_iter().collect();
        let (out, shown) = render("Gmail", "", &lines, None, &all, &mut numbers, true);
        assert!(out.contains("Nothing new appeared") && shown.is_empty(), "{out}");
    }

    /// The number a look gave the first line containing `what`.
    fn number(look: &str, what: &str) -> usize {
        let line = look.lines().find(|l| l.contains(what) && l.starts_with('[')).unwrap_or_else(|| panic!("no {what} in:\n{look}"));
        line[1..line.find(']').unwrap()].parse().unwrap()
    }

    /// Needs the test window from the scratchpad's testwin.ps1 open. Uses the controls'
    /// own accessibility actions only: the mouse and keyboard are never touched.
    #[test]
    #[ignore]
    fn drives_a_test_window() {
        let none = || Box::new(|_: i32, _: i32, _: bool| {}) as Box<dyn Fn(i32, i32, bool) + Send>;
        let first = look(Some("nimbus hands test".into()), None, false, false).unwrap().0;
        println!("{first}");
        let act = |n, a: &str, t: Option<String>| act(n, a.into(), t, None, false, none()).map(|r| r.0);
        let after = act(number(&first, "Your name"), "type", Some("Ada".into())).unwrap();
        println!("{after}");
        let after = act(number(&after, "Remember me"), "toggle", None).unwrap();
        assert!(after.lines().any(|l| l.contains("\"Remember me\"") && l.contains(" on)")), "{after}");
        let after = act(number(&after, "Say hello"), "click", None).unwrap();
        println!("{after}");
        assert!(after.contains("Hello Ada (remembered)"), "{after}");
        // The window's own Close button is not offered.
        assert!(!after.contains("button \"Close\""), "{after}");
    }

    /// Prints a look at the window named in NIMBUS_LOOK (words of its title), with the
    /// words in NIMBUS_FIND if set. Reads only.
    #[test]
    #[ignore]
    fn look_at_named_window() {
        let want = std::env::var("NIMBUS_LOOK").expect("NIMBUS_LOOK names the window");
        match look(Some(want), std::env::var("NIMBUS_FIND").ok(), false, false) {
            Ok((text, _)) => println!("{text}"),
            Err(e) => println!("FAILED: {e}"),
        }
    }

    /// Types NIMBUS_TYPE into the first field matching NIMBUS_FIND in the window named by
    /// NIMBUS_LOOK. Real typing: for trying it out on a field chosen by hand.
    #[test]
    #[ignore]
    fn types_into_named_field() {
        let want = std::env::var("NIMBUS_LOOK").expect("NIMBUS_LOOK names the window");
        let find = std::env::var("NIMBUS_FIND").expect("NIMBUS_FIND names the field");
        // Without NIMBUS_TYPE, the first match is clicked instead.
        let text = std::env::var("NIMBUS_TYPE").ok();
        let (seen, _) = look(Some(want), Some(find.clone()), false, false).unwrap();
        println!("{seen}");
        let field = |l: &&str| l.starts_with('[') && (text.is_none() || l.contains("text field") || l.contains("combo box"));
        let exact = format!("\"{}\"", find.to_lowercase());
        let lines: Vec<&str> = seen.lines().collect();
        let pick = lines.iter().find(|l| field(l) && l.to_lowercase().contains(&exact)).or_else(|| lines.iter().find(|l| field(l)));
        let n = pick.map(|l| l[1..l.find(']').unwrap()].parse().unwrap()).expect("a control");
        let none = Box::new(|_: i32, _: i32, _: bool| {}) as Box<dyn Fn(i32, i32, bool) + Send>;
        let action = if text.is_some() { "type" } else { "click" };
        match act(n, action.into(), text, None, false, none) {
            Ok((t, _)) => println!("{}", t.lines().next().unwrap_or("")),
            Err(e) => println!("FAILED: {e}"),
        }
    }

    #[test]
    fn kinds_of_windows() {
        assert_eq!(kind_of("CASCADIA_HOSTING_WINDOW_CLASS", "WindowsTerminal", "PowerShell"), "terminal");
        assert_eq!(kind_of("Chrome_WidgetWin_1", "msedge", "Bing"), "web");
        assert_eq!(kind_of("Chrome_WidgetWin_1", "WhatsApp", "WhatsApp"), "chat");
        assert_eq!(kind_of("", "1Password", "1Password"), "secret");
        assert_eq!(kind_of("Notepad", "Notepad", "notes.txt - Notepad"), "");
        assert_eq!(kind_of("ConsoleWindowClass", "Windows PowerShell", "Windows PowerShell"), "terminal");
        assert_eq!(kind_of("WindowsForms10.Window.8.app.0.1", "Windows PowerShell", "Nimbus hands test"), "");
    }
}
