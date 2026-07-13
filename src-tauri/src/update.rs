//! Self-update for the portable exe: check the latest GitHub release, and on
//! install download the new exe next to the current one, then hand off to a
//! detached cmd script that waits for this process to exit, swaps the files
//! and relaunches. No installer needed.

use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

const RELEASES_API: &str = "https://api.github.com/repos/yigitbozyaka/claude-pulse/releases/latest";

#[derive(Serialize, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

fn semver(v: &str) -> (u32, u32, u32) {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let mut n = || it.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (n(), n(), n())
}

pub fn check(current: &str) -> Option<UpdateInfo> {
    let resp = ureq::get(RELEASES_API)
        .set("User-Agent", "claude-pulse")
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()
        .ok()?;
    let body: Value = resp.into_json().ok()?;
    let tag = body["tag_name"].as_str()?;
    if semver(tag) <= semver(current) {
        return None;
    }
    let url = body["assets"]
        .as_array()?
        .iter()
        .find_map(|a| {
            let name = a["name"].as_str()?;
            name.ends_with(".exe")
                .then(|| a["browser_download_url"].as_str())
                .flatten()
        })?
        .to_string();
    Some(UpdateInfo { version: tag.trim_start_matches('v').to_string(), url })
}

/// Download the release exe and spawn the swap script. On success the caller
/// should exit the app; the script replaces the exe and relaunches it.
pub fn install(url: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let new = exe.with_extension("exe.update");

    let resp = ureq::get(url)
        .set("User-Agent", "claude-pulse")
        .timeout(Duration::from_secs(300))
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut bytes = Vec::new();
    resp.into_reader()
        .take(100 * 1024 * 1024)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("download failed: {e}"))?;
    if bytes.len() < 1024 * 1024 {
        return Err("downloaded file looks too small".into());
    }
    std::fs::write(&new, &bytes).map_err(|e| format!("write failed: {e}"))?;

    let script = std::env::temp_dir().join("claude-pulse-update.cmd");
    std::fs::write(
        &script,
        format!(
            "@echo off\r\n\
             :loop\r\n\
             timeout /t 1 /nobreak >nul\r\n\
             move /y \"{new}\" \"{exe}\" >nul 2>&1\r\n\
             if errorlevel 1 goto loop\r\n\
             start \"\" \"{exe}\"\r\n\
             del \"%~f0\"\r\n",
            new = new.display(),
            exe = exe.display()
        ),
    )
    .map_err(|e| format!("script write failed: {e}"))?;

    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/c").arg(&script);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    Ok(())
}
