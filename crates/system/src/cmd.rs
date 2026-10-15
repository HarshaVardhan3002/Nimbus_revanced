//! Small helper for the macOS and Linux backends, which drive standard CLI tools.

use crate::{Error, Result};
use std::process::Command;

/// Run a program and return its trimmed stdout, or an error naming the program.
pub fn run(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::Failed(format!("{program}: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(Error::Failed(format!("{program}: {}", err.trim())));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
pub fn osascript(script: &str) -> Result<String> {
    run("osascript", &["-e", script])
}
