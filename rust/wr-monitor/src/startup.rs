//! "Start with Windows": the WhiteRabbit value under HKCU\...\CurrentVersion\Run, the same value the
//! installer and the Python tray (`led_mon/startup.py`) use. Task Manager can disable it under
//! ...\Explorer\StartupApproved\Run (odd first byte), which counts as off.
//!
//! Used by the tray menu's "Start with Windows" check item (`ui.rs`), on the tray thread. Only the
//! current user's part of the registry (HKEY_CURRENT_USER) is touched, so no admin rights are needed.
//! Registry strings are UTF-16 with a closing zero, which `wide` makes.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
};
use wr_panel::winports::wide;

/// The registry key whose values Windows runs at login.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager's Startup tab stores enabled/disabled marks (a binary value per Run entry).
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// The name of our value under both keys.
const VALUE_NAME: &str = "WhiteRabbit";

/// Reads our value under `key` in HKEY_CURRENT_USER as raw bytes; `flags` limits the accepted type
/// (string or binary). None when the value is missing, has another type, or can't be read.
fn read(key: &str, flags: u32) -> Option<Vec<u8>> {
    let (k, v) = (wide(key), wide(VALUE_NAME));
    let mut size = 0u32;
    // SAFETY: first call asks for the size, the second fills a buffer of that size.
    unsafe {
        if RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), flags, null_mut(), null_mut(), &mut size)
            != ERROR_SUCCESS
        {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            v.as_ptr(),
            flags,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut size,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        buf.truncate(size as usize);
        Some(buf)
    }
}

/// True when the Run entry exists and Task Manager has not marked it disabled.
pub fn is_enabled() -> bool {
    if read(RUN_KEY, RRF_RT_REG_SZ).is_none() {
        return false;
    }
    // Task Manager writes 02 (enabled) or 03 (disabled) as the first byte, so an odd byte means off.
    !matches!(read(APPROVED_KEY, RRF_RT_REG_BINARY), Some(b) if b.first().is_some_and(|x| x % 2 == 1))
}

/// The command Windows runs at login: this exe with --background (monitor and tray icon, no window).
fn command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\" --background")
}

/// Turns "Start with Windows" on (writes the Run value pointing at this exe) or off (deletes it).
/// Registry errors are ignored: the tray menu reads `is_enabled` again, so a failure just shows as
/// the item not changing.
pub fn set_enabled(enabled: bool) {
    let name = wide(VALUE_NAME);
    // SAFETY: plain registry calls with null-terminated strings that outlive them.
    unsafe {
        if enabled {
            let key = wide(RUN_KEY);
            let data = wide(&command());
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                // The size is in bytes: UTF-16 uses 2 bytes per unit, including the closing zero.
                (data.len() * 2) as u32,
            );
        } else {
            let key = wide(RUN_KEY);
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
        }
        // Clear Task Manager's enabled/disabled mark so ours is the one that counts
        let approved = wide(APPROVED_KEY);
        RegDeleteKeyValueW(HKEY_CURRENT_USER, approved.as_ptr(), name.as_ptr());
    }
}
