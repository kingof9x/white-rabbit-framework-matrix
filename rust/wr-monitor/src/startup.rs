//! "Start with Windows": the WhiteRabbit value under HKCU\...\CurrentVersion\Run, the same value the
//! installer and the Python tray (`led_mon/startup.py`) use. Task Manager can disable it under
//! ...\Explorer\StartupApproved\Run (odd first byte), which counts as off.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
};
use wr_panel::winports::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE_NAME: &str = "WhiteRabbit";

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

pub fn is_enabled() -> bool {
    if read(RUN_KEY, RRF_RT_REG_SZ).is_none() {
        return false;
    }
    !matches!(read(APPROVED_KEY, RRF_RT_REG_BINARY), Some(b) if b.first().is_some_and(|x| x % 2 == 1))
}

/// The command Windows runs at login: this exe with --background (monitor and tray icon, no window).
fn command() -> String {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{exe}\" --background")
}

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
