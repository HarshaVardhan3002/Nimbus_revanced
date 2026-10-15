//! Large downloads (the model server, model files): resumed where they broke off,
//! checked against the publisher's SHA-256, and stoppable.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent("Nimbus")
        .build()
        .into()
}

/// A small JSON answer (release lists, file lists).
pub fn json(url: &str) -> Result<serde_json::Value, String> {
    let mut r = agent().get(url).header("Accept", "application/json").call().map_err(|e| e.to_string())?;
    if r.status() != 200 {
        return Err(format!("{url} answered {}", r.status()));
    }
    let text = r.body_mut().with_config().limit(8 << 20).read_to_string().map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn part_of(dest: &Path) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect())
}

/// Download `url` to `dest`. A `.part` file left by an earlier try is continued, not
/// started over. `on` hears (bytes so far, whole size) a few times a second.
pub fn download(url: &str, dest: &Path, sha256: Option<&str>, stop: &AtomicBool, mut on: impl FnMut(u64, u64)) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let part = part_of(dest);
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = agent().get(url);
    if have > 0 {
        req = req.header("Range", &format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let length = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let mut out = match status {
        206 => OpenOptions::new().append(true).open(&part),
        200 => {
            have = 0;
            File::create(&part)
        }
        // Asked for bytes past the end: the earlier try already had everything.
        416 if have > 0 => return finish(&part, dest, sha256),
        s => return Err(format!("the server answered {s}")),
    }
    .map_err(|e| e.to_string())?;
    let total = length.map(|l| l + have).unwrap_or(0);
    let mut body = resp.into_body();
    let mut reader = body.as_reader();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = have;
    let mut told = Instant::now();
    on(done, total);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("stopped".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("the download broke off ({e}); it continues from here next time"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if told.elapsed() >= Duration::from_millis(250) {
            on(done, total);
            told = Instant::now();
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    on(done, total);
    if total > 0 && done < total {
        return Err("the download broke off; it continues from here next time".into());
    }
    finish(&part, dest, sha256)
}

fn finish(part: &Path, dest: &Path, sha256: Option<&str>) -> Result<(), String> {
    if let Some(want) = sha256 {
        let got = sha256_of(part).map_err(|e| e.to_string())?;
        if !got.eq_ignore_ascii_case(want) {
            let _ = fs::remove_file(part);
            return Err("the downloaded file was damaged (its checksum did not match); try again".into());
        }
    }
    let _ = fs::remove_file(dest);
    fs::rename(part, dest).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum() {
        let dir = std::env::temp_dir().join(format!("nimbus-fetch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.part");
        fs::write(&f, b"abc").unwrap();
        assert_eq!(sha256_of(&f).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert!(finish(&f, &dir.join("a"), Some("00")).is_err());
        assert!(!f.exists(), "a damaged download is thrown away");
        fs::write(&f, b"abc").unwrap();
        finish(&f, &dir.join("a"), Some("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD")).unwrap();
        assert_eq!(fs::read(dir.join("a")).unwrap(), b"abc");
        fs::remove_dir_all(dir).unwrap();
    }
}
