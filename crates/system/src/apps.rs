//! Installed apps for the launcher: what the Start menu lists under "All apps" on
//! Windows (desktop and Store apps alike), the Applications folders on macOS, and
//! the desktop entries on Linux.

use crate::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct App {
    /// Pass to [`launch`]: the shell's app id on Windows, the bundle on macOS, the desktop file on Linux.
    pub id: String,
    pub name: String,
}

/// An app icon as straight (not premultiplied) RGBA pixels.
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Every launchable app, sorted by name. `want_icon` is asked for each app id;
/// return false for apps whose icon is already cached, so it is not drawn again.
pub fn list(icon_size: u32, want_icon: impl Fn(&str) -> bool + Sync) -> Vec<(App, Option<Icon>)> {
    let mut out = imp::list(icon_size, &want_icon);
    out.sort_by_key(|(a, _)| a.name.to_lowercase());
    out.dedup_by(|a, b| a.0.id == b.0.id);
    out
}

pub fn launch(id: &str) -> Result<()> {
    imp::launch(id)
}

/// Opens a link, file or folder through the running Windows shell (Explorer), as the
/// Start menu would. What opens then belongs to the shell, not to the caller: it stays
/// open when the caller is closed or restarted, even when the caller runs inside a job
/// that ends everything it started (as a development build does). False when there is
/// no shell to ask; the caller then opens it itself.
#[cfg(windows)]
pub fn open_via_shell(target: &str) -> bool {
    imp::via_shell(target, "", "").is_some()
}

/// The same with a command line for the program and the folder it starts in.
#[cfg(windows)]
pub fn run_via_shell(program: &str, args: &str, dir: &str) -> bool {
    imp::via_shell(program, args, dir).is_some()
}

/// Uninstallers, read-me files and web links sit next to apps in the Start menu.
#[cfg_attr(not(windows), allow(dead_code))]
fn launchable(name: &str, id: &str) -> bool {
    const DOCS: [&str; 9] = [".txt", ".pdf", ".chm", ".htm", ".html", ".rtf", ".md", ".ini", ".log"];
    const WORDS: [&str; 8] = ["uninstall", "readme", "read me", "website", "documentation", "release notes", "license", "help and support"];
    let (n, i) = (name.to_lowercase(), id.to_lowercase());
    !WORDS.iter().any(|w| n.contains(w)) && !DOCS.iter().any(|d| i.ends_with(d))
}

#[cfg(windows)]
mod imp {
    use super::{App, Icon, launchable};
    use crate::{Error, Result};
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP, ReleaseDC,
    };
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IDispatch, IServiceProvider,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{
        BHID_EnumItems, CSIDL_DESKTOP, FOLDERID_AppsFolder, IEnumShellItems, IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellItem,
        IShellItemImageFactory, IShellWindows, KF_FLAG_DEFAULT, SHGetKnownFolderItem, SID_STopLevelBrowser, SIGDN, SIGDN_NORMALDISPLAY,
        SIGDN_PARENTRELATIVEPARSING, SIIGBF, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY, SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH,
        ShellExecuteW, ShellWindows,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{BSTR, HSTRING, Interface, PCWSTR, w};

    /// Shell objects want a single-threaded apartment, so each call gets a thread of its own.
    fn on_sta_thread<T: Send>(f: impl FnOnce() -> T + Send) -> Option<T> {
        std::thread::scope(|s| {
            s.spawn(|| unsafe {
                let inited = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
                let out = f();
                if inited {
                    CoUninitialize();
                }
                out
            })
            .join()
            .ok()
        })
    }

    pub fn list(size: u32, want_icon: &(dyn Fn(&str) -> bool + Sync)) -> Vec<(App, Option<Icon>)> {
        on_sta_thread(|| unsafe {
            let mut out = Vec::new();
            let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else { return out };
            let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return out };
            loop {
                let mut one = [None];
                let mut got = 0u32;
                if items.Next(&mut one, Some(&mut got)).is_err() || got == 0 {
                    break;
                }
                let Some(item) = one[0].take() else { break };
                let (Some(name), Some(id)) = (display(&item, SIGDN_NORMALDISPLAY), display(&item, SIGDN_PARENTRELATIVEPARSING))
                else {
                    continue;
                };
                if !launchable(&name, &id) {
                    continue;
                }
                let icon = if want_icon(&id) { icon(&item, size) } else { None };
                out.push((App { id, name }, icon));
            }
            out
        })
        .unwrap_or_default()
    }

    unsafe fn display(item: &IShellItem, kind: SIGDN) -> Option<String> {
        unsafe {
            let p = item.GetDisplayName(kind).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.filter(|s| !s.is_empty())
        }
    }

    unsafe fn icon(item: &IShellItem, size: u32) -> Option<Icon> {
        unsafe {
            let factory: IShellItemImageFactory = item.cast().ok()?;
            let flags = SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_BIGGERSIZEOK.0);
            let bmp = factory.GetImage(SIZE { cx: size as i32, cy: size as i32 }, flags).ok()?;
            let out = pixels(bmp);
            let _ = DeleteObject(bmp.into());
            out
        }
    }

    unsafe fn pixels(bmp: HBITMAP) -> Option<Icon> {
        unsafe {
            let mut bm = BITMAP::default();
            if GetObjectW(bmp.into(), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
                return None;
            }
            let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
            if w <= 0 || h <= 0 {
                return None;
            }
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut px = vec![0u8; (w * h * 4) as usize];
            let dc = GetDC(None);
            let lines = GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
            ReleaseDC(None, dc);
            if lines == 0 {
                return None;
            }
            // Old icons carry no alpha at all; treat them as opaque.
            let has_alpha = px.chunks_exact(4).any(|p| p[3] != 0);
            for p in px.chunks_exact_mut(4) {
                let a = if has_alpha { p[3] } else { 255 };
                // The shell hands back premultiplied BGRA; PNG wants straight RGBA.
                let un = |c: u8| match a {
                    0 => 0,
                    255 => c,
                    _ => ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8,
                };
                let (b, g, r) = (p[0], p[1], p[2]);
                p[0] = un(r);
                p[1] = un(g);
                p[2] = un(b);
                p[3] = a;
            }
            Some(Icon { width: w as u32, height: h as u32, rgba: px })
        }
    }

    /// Asks Explorer's desktop window to open `target` (the way Raymond Chen describes for
    /// starting a program as the shell): Explorer starts it, so it is Explorer's child.
    pub fn via_shell(target: &str, args: &str, dir: &str) -> Option<()> {
        match on_sta_thread(|| unsafe { shell_execute(target, args, dir) }) {
            Some(Ok(())) => Some(()),
            Some(Err(e)) => {
                eprintln!("[nimbus] the shell did not open it: {e}");
                None
            }
            None => None,
        }
    }

    unsafe fn shell_execute(target: &str, args: &str, dir: &str) -> std::result::Result<(), String> {
        let step = |what: &'static str| move |e: windows::core::Error| format!("{what}: {e}");
        unsafe {
            let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER).map_err(step("ShellWindows"))?;
            let desktop = VARIANT::from(CSIDL_DESKTOP as i32);
            let none = VARIANT::default();
            let mut hwnd = 0i32;
            let found = windows.FindWindowSW(&desktop, &none, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH).map_err(step("FindWindowSW"))?;
            let browser: IShellBrowser = found.cast::<IServiceProvider>().map_err(step("IServiceProvider"))?.QueryService(&SID_STopLevelBrowser).map_err(step("QueryService"))?;
            let view: IShellFolderViewDual =
                browser.QueryActiveShellView().map_err(step("QueryActiveShellView"))?.GetItemObject::<IDispatch>(SVGIO_BACKGROUND)
                .map_err(step("GetItemObject"))?
                .cast()
                .map_err(step("IShellFolderViewDual"))?;
            let shell: IShellDispatch2 = view.Application().map_err(step("Application"))?.cast().map_err(step("IShellDispatch2"))?;
            let open = VARIANT::from(BSTR::from("open"));
            let show = VARIANT::from(SW_SHOWNORMAL.0);
            let text = |t: &str| if t.is_empty() { VARIANT::default() } else { VARIANT::from(BSTR::from(t)) };
            shell.ShellExecute(&BSTR::from(target), &text(args), &text(dir), &open, &show).map_err(step("ShellExecute"))
        }
    }

    pub fn launch(id: &str) -> Result<()> {
        // Through the shell first, so the app does not end with Nimbus (see `via_shell`).
        if via_shell(&format!("shell:AppsFolder\\{id}"), "", "").is_some() {
            return Ok(());
        }
        // The AppsFolder path opens desktop and Store apps alike, the way Start does.
        let target = HSTRING::from(format!("shell:AppsFolder\\{id}"));
        let code = on_sta_thread(|| unsafe {
            ShellExecuteW(None, w!("open"), &target, PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL).0 as isize
        })
        .unwrap_or(0);
        if code > 32 { Ok(()) } else { Err(Error::Failed(format!("Windows could not open the app (code {code})"))) }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{App, Icon};
    use crate::Result;
    use crate::cmd::run;

    pub fn list(_size: u32, _want_icon: &(dyn Fn(&str) -> bool + Sync)) -> Vec<(App, Option<Icon>)> {
        let home = std::env::var("HOME").unwrap_or_default();
        let dirs = [
            "/Applications".to_string(),
            "/Applications/Utilities".into(),
            "/System/Applications".into(),
            "/System/Applications/Utilities".into(),
            format!("{home}/Applications"),
        ];
        let mut out = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for p in entries.flatten().map(|e| e.path()) {
                if p.extension().is_some_and(|x| x == "app") {
                    let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    out.push((App { id: p.to_string_lossy().into_owned(), name }, None));
                }
            }
        }
        out
    }

    pub fn launch(id: &str) -> Result<()> {
        run("open", &[id]).map(|_| ())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::{App, Icon};
    use crate::Result;
    use crate::cmd::run;

    /// Name of a desktop entry, or `None` when it should not be listed.
    fn entry(text: &str) -> Option<String> {
        let mut in_entry = false;
        let (mut name, mut app) = (None, false);
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
                continue;
            }
            if !in_entry {
                continue;
            }
            match line.split_once('=') {
                Some(("Name", v)) if name.is_none() => name = Some(v.trim().to_string()),
                Some(("Type", v)) => app = v.trim() == "Application",
                Some(("NoDisplay" | "Hidden", v)) if v.trim() == "true" => return None,
                _ => {}
            }
        }
        name.filter(|_| app)
    }

    pub fn list(_size: u32, _want_icon: &(dyn Fn(&str) -> bool + Sync)) -> Vec<(App, Option<Icon>)> {
        let home = std::env::var("HOME").unwrap_or_default();
        let dirs = [
            "/usr/share/applications".to_string(),
            "/usr/local/share/applications".into(),
            "/var/lib/flatpak/exports/share/applications".into(),
            format!("{home}/.local/share/applications"),
            format!("{home}/.local/share/flatpak/exports/share/applications"),
        ];
        let mut out = Vec::new();
        for dir in dirs {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for p in entries.flatten().map(|e| e.path()) {
                if p.extension().is_some_and(|x| x == "desktop") {
                    if let Some(name) = std::fs::read_to_string(&p).ok().as_deref().and_then(entry) {
                        out.push((App { id: p.to_string_lossy().into_owned(), name }, None));
                    }
                }
            }
        }
        out
    }

    pub fn launch(id: &str) -> Result<()> {
        run("gio", &["launch", id]).map(|_| ())
    }
}

#[cfg(all(test, windows))]
mod tests {
    /// Starts Character Map through the shell and checks Explorer is its parent, not this
    /// test. Leaves it open: the caller closes it.
    #[test]
    #[ignore]
    fn opens_outside_the_caller() {
        assert!(super::open_via_shell(r"C:\Windows\System32\charmap.exe"));
        std::thread::sleep(std::time::Duration::from_secs(2));
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-c", "$p = Get-CimInstance Win32_Process -Filter \"Name='charmap.exe'\" | Select -First 1; (Get-CimInstance Win32_Process -Filter \"ProcessId=$($p.ParentProcessId)\").Name"])
            .output()
            .unwrap();
        let parent = String::from_utf8_lossy(&out.stdout).trim().to_lowercase();
        println!("charmap's parent: {parent}");
        assert_eq!(parent, "explorer.exe");
    }
}
