//! Downloads the newest keyboard and macropad firmware from the keyboard repository
//! (kingof9x/framework-16-keyboard-customization), for the Firmware page's "Update firmware".
//!
//! The keyboard repository rebuilds its firmware after every merge and puts it in a pre-release
//! called `firmware-latest`, so this gets the firmware built from the newest code without anyone
//! making a numbered release. If that pre-release isn't there, the newest numbered release is used.

#[cfg(windows)]
use serde_json::Value;
use wr_keyboard::protocol::FirmwareBuild;

/// Where the keyboard and macropad firmware is built.
#[cfg_attr(not(windows), allow(dead_code))]
pub const REPO: &str = "kingof9x/framework-16-keyboard-customization";

/// The pre-release that always holds the firmware built from the newest code.
#[cfg_attr(not(windows), allow(dead_code))]
pub const NEWEST_TAG: &str = "firmware-latest";

/// From a GitHub release: what to call it ("version 2.0", or the newest build's title) and the
/// download link of its file called `file`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn pick(release: &serde_json::Value, file: &str) -> Result<(String, String), String> {
    let tag = release.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let name = release.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
    let label = if tag == NEWEST_TAG {
        if name.is_empty() {
            "the newest firmware".to_string()
        } else {
            name.to_lowercase().replacen("newest", "the newest", 1)
        }
    } else {
        format!("version {}", tag.trim_start_matches('v'))
    };
    let url = release
        .get("assets")
        .and_then(|a| a.as_array())
        .and_then(|a| a.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some(file)))
        .and_then(|a| a.get("browser_download_url").and_then(|u| u.as_str()))
        .map(String::from)
        .ok_or_else(|| format!("GitHub has no {file} in {label}."))?;
    Ok((label, url))
}

/// The release at `releases/<which>` on GitHub, or None when there isn't one.
#[cfg(windows)]
fn release(which: &str) -> Result<Option<Value>, String> {
    use std::time::Duration;
    let url = format!("https://api.github.com/repos/{REPO}/releases/{which}");
    match crate::pages::updates::agent(Duration::from_secs(15))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "WhiteRabbit")
        .call()
    {
        Ok(mut reply) => {
            reply.body_mut().read_json().map(Some).map_err(|e| format!("Couldn't read GitHub's answer: {e}"))
        }
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(format!("Couldn't reach GitHub: {e}")),
    }
}

/// What the newest firmware is called ("the newest firmware (2026-11-23, abc1234)" or
/// "version 2.0") and its file called `file` (a device's firmware).
#[cfg(windows)]
pub fn latest_asset(file: &str) -> Result<(String, Vec<u8>), String> {
    use std::time::Duration;
    let found = match release(&format!("tags/{NEWEST_TAG}"))? {
        Some(r) => r,
        None => release("latest")?.ok_or("No keyboard firmware has been built on GitHub yet.")?,
    };
    let (label, url) = pick(&found, file)?;
    let mut reply = crate::pages::updates::agent(Duration::from_secs(60))
        .get(&url)
        .header("User-Agent", "WhiteRabbit")
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let data = reply
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| format!("Download failed: {e}"))?;
    Ok((label, data))
}

/// The newest firmware for a device on GitHub: what it is called, and its build number when the
/// file has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Newest {
    pub label: String,
    pub build: Option<FirmwareBuild>,
}

/// Looks up the newest firmware for `kind` (downloads its small .uf2 file to read the build).
pub fn newest(kind: wr_keyboard::protocol::DeviceKind) -> Result<Newest, String> {
    let (label, data) = latest_asset(super::flash::release_file(kind))?;
    Ok(Newest { label, build: super::flash::uf2_build(&data) })
}

/// What the Firmware page says about a device after a check, and whether the newest firmware is
/// newer than what it has. `installed` is None for a device without this app's firmware.
pub fn compare(installed: Option<FirmwareBuild>, newest: &Newest) -> (String, bool) {
    use FirmwareBuild::*;
    match (installed, newest.build) {
        (Some(Build(n)), Some(Build(m))) if n >= m => (format!("Up to date: build {n} is the newest."), false),
        (Some(Build(n)), Some(Build(m))) => (format!("Build {m} is available (you have build {n})."), true),
        (Some(Test), Some(Build(m))) => {
            (format!("You have a test build. The newest is build {m}: update to go back to it."), false)
        }
        (_, Some(Build(m))) => (format!("Build {m} is available."), true),
        _ => (format!("The newest firmware on GitHub is {}.", newest.label), false),
    }
}

#[cfg(not(windows))]
pub fn latest_asset(_file: &str) -> Result<(String, Vec<u8>), String> {
    Err("Firmware is downloaded in the Windows app.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn newest_build_is_named_by_its_title() {
        let r = json!({"tag_name": "firmware-latest", "name": "Newest firmware (2026-11-23, abc1234)",
            "assets": [{"name": "framework_ansi_f16custom.uf2", "browser_download_url": "https://e/a.uf2"}]});
        let (label, url) = pick(&r, "framework_ansi_f16custom.uf2").unwrap();
        assert_eq!(label, "the newest firmware (2026-11-23, abc1234)");
        assert_eq!(url, "https://e/a.uf2");
    }

    #[test]
    fn compares_builds() {
        let newest = |b| Newest { label: "x".into(), build: b };
        let b14 = newest(Some(FirmwareBuild::Build(14)));
        assert!(compare(Some(FirmwareBuild::Build(12)), &b14).1);
        assert!(!compare(Some(FirmwareBuild::Build(14)), &b14).1);
        assert!(!compare(Some(FirmwareBuild::Build(15)), &b14).1);
        assert!(compare(Some(FirmwareBuild::Older), &b14).1);
        assert!(compare(None, &b14).1, "Framework's own firmware: offer ours");
        assert!(!compare(Some(FirmwareBuild::Test), &b14).1, "a test build is left alone");
        assert!(!compare(Some(FirmwareBuild::Older), &newest(None)).1, "no number: can't tell");
        assert_eq!(compare(Some(FirmwareBuild::Build(12)), &b14).0, "Build 14 is available (you have build 12).");
    }

    #[test]
    fn numbered_release_is_named_by_its_version() {
        let r = json!({"tag_name": "v2.0", "assets": [
            {"name": "framework_macropad_f16custom.uf2", "browser_download_url": "https://e/m.uf2"}]});
        assert_eq!(pick(&r, "framework_macropad_f16custom.uf2").unwrap().0, "version 2.0");
        assert!(pick(&r, "framework_ansi_f16custom.uf2").is_err(), "only the exact file counts");
    }
}
