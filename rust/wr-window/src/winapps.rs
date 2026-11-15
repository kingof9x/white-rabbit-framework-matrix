//! The open apps, for the FPS counter's game picker: each visible top-level window's program and title.

/// [(exe file name, window title)], one per program, in the order Windows lists the windows.
pub fn running_apps() -> Vec<(String, String)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindow, GetWindowLongW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
            GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
        };
        struct Found {
            own: u32,
            apps: Vec<(String, String)>,
        }
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: lparam is the Found passed to EnumWindows below, alive for the whole call; the
            // buffers and their lengths go together.
            unsafe {
                let found = &mut *(lparam as *mut Found);
                if IsWindowVisible(hwnd) == 0
                    || !GetWindow(hwnd, GW_OWNER).is_null()
                    || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0
                {
                    return 1;
                }
                let mut buf = [0u16; 256];
                let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]).trim().to_string();
                if title.is_empty() {
                    return 1;
                }
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid == found.own {
                    return 1;
                }
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if process.is_null() {
                    return 1;
                }
                let mut path = [0u16; 1024];
                let mut len = path.len() as u32;
                let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) != 0;
                CloseHandle(process);
                if !ok {
                    return 1;
                }
                let full = String::from_utf16_lossy(&path[..len as usize]);
                let exe = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
                let skip = ["explorer.exe", "textinputhost.exe", "applicationframehost.exe", "systemsettings.exe"];
                if skip.contains(&exe.to_lowercase().as_str()) || found.apps.iter().any(|(e, _)| e.eq_ignore_ascii_case(&exe)) {
                    return 1;
                }
                found.apps.push((exe, title));
            }
            1
        }
        let mut found = Found { own: std::process::id(), apps: Vec::new() };
        // SAFETY: the callback only uses found, which outlives the call.
        unsafe {
            EnumWindows(Some(each), &mut found as *mut Found as LPARAM);
        }
        found.apps
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
