//! Downloads the newest keyboard and macropad firmware from the keyboard repository's releases
//! (kingof9x/framework-16-keyboard-customization), for the Firmware page's "Update firmware".

/// Where the keyboard and macropad firmware is released.
#[cfg_attr(not(windows), allow(dead_code))]
pub const REPO: &str = "kingof9x/framework-16-keyboard-customization";

/// The newest release's version and its file called `file` (a device's firmware).
#[cfg(windows)]
pub fn latest_asset(file: &str) -> Result<(String, Vec<u8>), String> {
    use std::time::Duration;
    let release = crate::pages::updates::latest_release(REPO, file)?;
    let url = release.asset_url.ok_or_else(|| format!("Release {} has no {file} in it.", release.version))?;
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
    Ok((release.version, data))
}

#[cfg(not(windows))]
pub fn latest_asset(_file: &str) -> Result<(String, Vec<u8>), String> {
    Err("Firmware is downloaded in the Windows app.".into())
}
