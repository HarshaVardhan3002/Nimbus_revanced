//! Small Windows helpers shared by the modules: process paths and friendly app names.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::core::{HSTRING, PWSTR, w};

/// Full path of a running process's program file.
pub fn exe_path(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The name a program gives itself ("Google Chrome", "Visual Studio Code"), falling
/// back to its file name. Cached, since version resources are read from disk.
pub fn app_name(exe: &str) -> String {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(n) = cache.lock().ok().and_then(|c| c.get(exe).cloned()) {
        return n;
    }
    let stem = std::path::Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = describe(exe).filter(|d| !d.is_empty() && d.chars().count() <= 40).unwrap_or(stem);
    if let Ok(mut c) = cache.lock() {
        c.insert(exe.to_string(), name.clone());
    }
    name
}

/// FileDescription from the program's version resource.
fn describe(exe: &str) -> Option<String> {
    unsafe {
        let path = HSTRING::from(exe);
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr() as *mut _).ok()?;
        let block = data.as_ptr() as *const _;
        let (mut ptr, mut len) = (std::ptr::null_mut(), 0u32);
        // The first language and code page the resource lists, US English as the fallback.
        let (lang, cp) = if VerQueryValueW(block, w!("\\VarFileInfo\\Translation"), &mut ptr, &mut len).as_bool() && len >= 4 {
            let t = ptr as *const u16;
            (*t, *t.add(1))
        } else {
            (0x0409, 0x04b0)
        };
        let key = HSTRING::from(format!("\\StringFileInfo\\{lang:04x}{cp:04x}\\FileDescription"));
        if !VerQueryValueW(block, &key, &mut ptr, &mut len).as_bool() || len < 2 {
            return None;
        }
        let text = std::slice::from_raw_parts(ptr as *const u16, len as usize - 1);
        Some(String::from_utf16_lossy(text).trim().to_string())
    }
}
