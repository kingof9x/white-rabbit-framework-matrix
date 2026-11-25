//! The open apps, for the FPS counter's game picker: each visible top-level window's program and title.
//!
//! How: Windows' EnumWindows calls our `each` function once for every top-level window. `each` skips
//! windows a user wouldn't think of as an app (hidden ones, owned pop-ups, tool windows, untitled ones,
//! this window itself, and a few Windows shell programs), finds the program behind each remaining
//! window from its process id, and keeps the first window per program. Called on a worker thread by
//! the System monitor page; takes a few milliseconds.

///
/// Empty away from Windows.
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
        /// What the callback collects into, passed to it through EnumWindows' `lparam` pointer.
        struct Found {
            /// This process's id, so the White Rabbit window itself isn't listed.
            own: u32,
            /// (exe name, title) found so far.
            apps: Vec<(String, String)>,
        }
        /// Called by Windows for each top-level window. Returning 1 means "keep going"; it never stops early.
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: lparam is the Found passed to EnumWindows below, alive for the whole call; the
            // buffers and their lengths go together.
            unsafe {
                let found = &mut *(lparam as *mut Found);
                // Skip hidden windows, windows owned by another (dialogs, pop-ups) and tool windows (palettes
                // and floating toolbars, which have WS_EX_TOOLWINDOW): none of those is a game's main window.
                if IsWindowVisible(hwnd) == 0
                    || !GetWindow(hwnd, GW_OWNER).is_null()
                    || GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0
                {
                    return 1;
                }
                // The title, up to 255 UTF-16 units; GetWindowTextW returns how many it wrote (0 when none).
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
                // The least access that can still read the program's path; it works for most other users'
                // processes too. A process we may not open (some system ones) is skipped.
                let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if process.is_null() {
                    return 1;
                }
                // QueryFullProcessImageNameW takes the buffer size in `len` and sets it to the path's length.
                let mut path = [0u16; 1024];
                let mut len = path.len() as u32;
                let ok = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len) != 0;
                CloseHandle(process);
                if !ok {
                    return 1;
                }
                let full = String::from_utf16_lossy(&path[..len as usize]);
                // Just the file name, e.g. "game.exe" from "C:\Games\game.exe".
                let exe = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
                // Windows' own shell windows (the desktop, the touch keyboard, UWP app frames, Settings).
                let skip = ["explorer.exe", "textinputhost.exe", "applicationframehost.exe", "systemsettings.exe"];
                if skip.contains(&exe.to_lowercase().as_str())
                    || found.apps.iter().any(|(e, _)| e.eq_ignore_ascii_case(&exe))
                {
                    return 1;
                }
                found.apps.push((exe, title));
            }
            1
        }
        let mut found = Found { own: std::process::id(), apps: Vec::new() };
        // SAFETY: the callback only uses found, which outlives the call.
        unsafe {
            // The pointer to `found` is passed as the lparam number and turned back into a reference in `each`;
            // EnumWindows returns only after the last callback, so `found` is alive throughout.
            EnumWindows(Some(each), &mut found as *mut Found as LPARAM);
        }
        found.apps
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}
