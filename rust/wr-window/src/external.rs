//! Finding and opening Libre Hardware Monitor and Framework Control, for the tray's menu items
//! (--open-lhm, --find-lhm, --open-framework-control) and the layout editor (`led_mon/external_apps.py`).

//!
//! Libre Hardware Monitor (LHM) is the free program that reads the PC's sensors for the monitor; it
//! has no installer, so people unzip it anywhere. Finding it is a search in order of likelihood:
//! a running copy (just bring its window forward), the path saved in settings.json, a Start menu
//! shortcut, then a bounded walk of likely folders (Desktop, Downloads, Documents, Program Files,
//! the user's folder, C:\, the app's own folder). The walk is limited to 3 levels deep and 3000
//! folders per starting folder, so a huge disk can't make it hang. If all that fails the tray item
//! asks the user to pick the exe and saves the answer.
//!
//! Framework Control is found by its shortcut, or else by the port its background service listens
//! on (read from the service's XML config), opened as a local web page.
//!
//! The tray items run in a short-lived window process with no window (main.rs returns after them),
//! so blocking here is fine; message boxes are Windows' own. The Windows calls (process snapshot,
//! window enumeration, known folders) are wrapped in small functions below, each with its safety
//! reasoning; away from Windows they find nothing.
use std::path::{Path, PathBuf};

/// The usual name of Libre Hardware Monitor's program file.
const LHM_EXE: &str = "LibreHardwareMonitor.exe";
/// How a Start menu shortcut to LHM's name starts (lower case), with or without spaces.
const LHM_SHORTCUT_PREFIXES: [&str; 2] = ["libre hardware monitor", "librehardwaremonitor"];
/// The name of Framework Control's shortcut (".lnk" or ".url").
const FC_SHORTCUT_NAME: &str = "Framework Control";
/// Framework Control's folder under Program Files.
const FC_INSTALL_FOLDER: &str = "FrameworkControl";
/// The service config file in that folder that holds its web port.
const FC_SERVICE_CONFIG: &str = "FrameworkControlService.xml";
/// The Start menu's programs folder, under %APPDATA% (this user) or %ProgramData% (everyone).
const START_MENU: &str = r"Microsoft\Windows\Start Menu\Programs";
/// How many folder levels below a search folder's subfolders are looked into.
const MAX_DEPTH: usize = 3;
/// Most folders looked into below one search folder, a cap on the time a search can take.
const MAX_FOLDERS: usize = 3000;

/// What to open, as found by the finders.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// An exe or shortcut to open
    File(PathBuf),
    /// A web page to open in the browser (Framework Control's local page)
    Url(String),
    /// Already running: bring its window forward
    ///
    /// The number is the running program's process id.
    Running(u32),
}

/// Lower case with everything but letters and digits removed, so "Libre Hardware Monitor",
/// "LibreHardwareMonitor" and "libre-hardware-monitor" compare the same.
fn compact(name: &str) -> String {
    name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// LibreHardwareMonitor.exe, and variants such as LibreHardwareMonitor_x64.exe.
pub fn is_lhm_exe(file_name: &str) -> bool {
    let lower = file_name.to_lowercase();
    // Strip ".exe" (4 ASCII bytes, so the cut is on a character boundary), then compare letters and digits.
    lower.ends_with(".exe") && compact(&lower[..lower.len() - 4]).starts_with("librehardwaremonitor")
}

/// An environment variable, None when it is missing or empty.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// LHM's exe directly in this folder: the usual name first, else the first variant in name order.
/// None if there is none or the folder can't be read.
fn exe_in(folder: &Path) -> Option<PathBuf> {
    let direct = folder.join(LHM_EXE);
    if direct.is_file() {
        return Some(direct);
    }
    let mut variants: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file() && is_lhm_exe(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .collect();
    // Sorted so the same file is picked every time when there are several.
    variants.sort();
    variants.into_iter().next()
}

/// The folders directly inside this one (none if it can't be read).
fn subdirs(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

/// Depth-first search for the exe in this folder and up to `depth` levels below it. Every folder
/// looked into uses one unit of `budget`; at 0 the search gives up.
fn find_below(folder: &Path, depth: usize, budget: &mut usize) -> Option<PathBuf> {
    // Every folder looked into costs one; stop the whole search when the budget is spent.
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if let Some(found) = exe_in(folder) {
        return Some(found);
    }
    if depth == 0 {
        return None;
    }
    // Stop at the first folder that has it.
    subdirs(folder).iter().find_map(|sub| find_below(sub, depth - 1, budget))
}

/// The exe in root or below it: in every folder (any_folder: Desktop, Downloads, Documents), or only in
/// folders named like "...HardwareMonitor..." (the zip unpacks into such folders).
fn find_exe_under(root: &Path, any_folder: bool) -> Option<PathBuf> {
    if !root.is_dir() {
        return None;
    }
    if let Some(found) = exe_in(root) {
        return Some(found);
    }
    let mut budget = MAX_FOLDERS;
    for sub in subdirs(root) {
        let name = sub.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        // In Program Files and similar, only folders named like the program are entered, so the search
        // doesn't wander through every installed app.
        if !any_folder && !compact(&name).contains("hardwaremonitor") {
            continue;
        }
        if let Some(found) = find_below(&sub, MAX_DEPTH, &mut budget) {
            return Some(found);
        }
    }
    None
}

/// The folders to search, in order, each with whether every subfolder is searched (true for the
/// user's own folders, where an unzipped copy could be in any folder) or only ones named like
/// "...HardwareMonitor..." (Program Files, the drive's root and so on). Duplicates are dropped
/// (comparing paths ignoring case, as Windows does).
fn search_roots() -> Vec<(PathBuf, bool)> {
    // The user's own folders first (where a downloaded zip is usually unpacked), from Windows and from
    // the usual names under the profile and OneDrive.
    let mut personal: Vec<PathBuf> = known_folders();
    if let Some(home) = env("USERPROFILE") {
        for sub in ["Desktop", "Downloads", "Documents"] {
            personal.push(Path::new(&home).join(sub));
        }
    }
    if let Some(od) = env("OneDrive") {
        personal.push(Path::new(&od).join("Desktop"));
        personal.push(Path::new(&od).join("Documents"));
    }
    let mut roots: Vec<(PathBuf, bool)> = personal.into_iter().map(|p| (p, true)).collect();
    // Then the places programs are installed.
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(v) = env(var) {
            roots.push((PathBuf::from(v), false));
        }
    }
    if let Some(local) = env("LOCALAPPDATA") {
        roots.push((Path::new(&local).join("Programs"), false));
        roots.push((PathBuf::from(local), false));
    }
    if let Some(home) = env("USERPROFILE") {
        roots.push((PathBuf::from(&home), false));
        roots.push((Path::new(&home).join("Tools"), false));
        roots.push((Path::new(&home).join("Apps"), false));
    }
    if let Some(drive) = env("SystemDrive") {
        let root = PathBuf::from(format!("{drive}\\"));
        roots.push((root.join("Tools"), false));
        roots.push((root.join("Apps"), false));
        roots.push((root, false));
    }
    // Last, next to White Rabbit itself.
    roots.push((crate::paths::app_dir(), false));
    let mut seen = std::collections::HashSet::new();
    roots.retain(|(p, _)| seen.insert(p.to_string_lossy().to_lowercase()));
    roots
}

/// A Start menu shortcut whose name starts like LHM's, in the Start menu folder itself or one
/// folder below it, for this user first and then for all users.
fn find_lhm_shortcut() -> Option<PathBuf> {
    for base in ["APPDATA", "ProgramData"].iter().filter_map(|v| env(v)) {
        let folder = Path::new(&base).join(START_MENU);
        let mut dirs = vec![folder.clone()];
        dirs.extend(subdirs(&folder));
        for dir in dirs {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let lower = entry.file_name().to_string_lossy().to_lowercase();
                if lower.ends_with(".lnk") && LHM_SHORTCUT_PREFIXES.iter().any(|p| lower.starts_with(p)) {
                    return Some(entry.path());
                }
            }
        }
    }
    None
}

/// Libre Hardware Monitor: already running, the path in settings.json, a Start menu shortcut, or a
/// likely folder. None when not found.
pub fn find_libre_hardware_monitor(configured: &str) -> Option<Target> {
    // Already running: nothing to search for, just bring it forward.
    if let Some(pid) = running_pid(is_lhm_exe) {
        return Some(Target::Running(pid));
    }
    // A path pasted from Explorer's "Copy as path" has quotes around it. A folder is accepted too (the
    // exe in it is used). An Open Hardware Monitor exe (the older program LHM came from) is ignored.
    let configured = configured.trim().trim_matches('"');
    if !configured.is_empty() {
        let mut path = PathBuf::from(configured);
        if path.is_dir() {
            path = exe_in(&path).unwrap_or_else(|| path.join(LHM_EXE));
        }
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if path.is_file() && !compact(&name).starts_with("openhardwaremonitor") {
            return Some(Target::File(path));
        }
    }
    // A Start menu shortcut is opened as it is; Windows follows it to the exe.
    if let Some(shortcut) = find_lhm_shortcut() {
        return Some(Target::File(shortcut));
    }
    let roots = search_roots();
    for (root, any) in &roots {
        if let Some(exe) = find_exe_under(root, *any) {
            return Some(Target::File(exe));
        }
    }
    wr_core::info!(
        "Libre Hardware Monitor not found. Looked in: {}",
        roots.iter().map(|(r, _)| r.display().to_string()).collect::<Vec<_>>().join("; ")
    );
    None
}

///
/// A light text search, not a real XML parser: finds the tag holding FRAMEWORK_CONTROL_PORT and reads
/// its value="..." (either quote kind). None if the file, the name or a valid port (1..65535) isn't
/// there.
/// FRAMEWORK_CONTROL_PORT in the service's FrameworkControlService.xml.
fn read_fc_port(config: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(config).ok()?;
    let at = text.find("FRAMEWORK_CONTROL_PORT")?;
    // The tag around the name: from the "<" before it to the ">" after it.
    let tag_start = text[..at].rfind('<')?;
    let tag_end = at + text[at..].find('>')?;
    let tag = &text[tag_start..tag_end];
    let v = tag.split("value=").nth(1)?;
    // The value is quoted with " or '; read up to the matching quote.
    let quote = v.chars().next()?;
    let value: String = v[1..].chars().take_while(|c| *c != quote).collect();
    value.trim().parse::<u16>().ok().filter(|p| *p > 0)
}

/// Framework Control's web port, from its service config in any Program Files folder.
pub fn framework_control_port() -> Option<u16> {
    ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(|v| env(v))
        .find_map(|base| read_fc_port(&Path::new(&base).join(FC_INSTALL_FOLDER).join(FC_SERVICE_CONFIG)))
}

///
/// Shortcuts are looked for in the Start menu (all users, then this user) and on the desktops.
/// Framework Control: its shortcut, or the service's own web page.
pub fn find_framework_control() -> Option<Target> {
    let mut places = Vec::new();
    for v in ["ProgramData", "ALLUSERSPROFILE", "APPDATA"] {
        if let Some(b) = env(v) {
            places.push(Path::new(&b).join(START_MENU));
        }
    }
    for v in ["PUBLIC", "USERPROFILE"] {
        if let Some(b) = env(v) {
            places.push(Path::new(&b).join("Desktop"));
        }
    }
    for folder in places {
        for ext in [".lnk", ".url"] {
            let shortcut = folder.join(format!("{FC_SHORTCUT_NAME}{ext}"));
            if shortcut.is_file() {
                return Some(Target::File(shortcut));
            }
        }
    }
    // No shortcut: the service's own web page on this PC, if its port can be read.
    framework_control_port().map(|p| Target::Url(format!("http://127.0.0.1:{p}")))
}

///
/// A program already running has its window brought forward; if no visible window with "Hardware
/// Monitor" in its title can be found (it is minimised to its tray icon), the message says so.
/// Opens what the finders returned. Some(message) to show the user when it didn't open.
pub fn open_target(target: Option<Target>, app_name: &str) -> Option<String> {
    match target {
        None => Some(format!("{app_name} was not found.")),
        Some(Target::Running(pid)) => {
            if show_window_of(pid, "Hardware Monitor") {
                None
            } else {
                Some(format!("{app_name} is already running. Open it from its own icon in the system tray."))
            }
        }
        Some(Target::Url(url)) => {
            crate::app::open_url(&url);
            None
        }
        Some(Target::File(path)) => {
            if crate::app::open_path(&path.to_string_lossy()) {
                None
            } else {
                Some(format!("Could not open {app_name} ({}).", path.display()))
            }
        }
    }
}

///
/// Errors are logged and shown in a message box. "lhm" that finds nothing falls through to "find-lhm".
/// The tray's items: "lhm", "find-lhm" (ask where it is first) or "fc".
pub fn open_from_tray(which: &str) {
    let message = match which {
        "find-lhm" => {
            let Some(path) = ask_for_lhm() else { return };
            // Remember the choice in settings.json, so next time it is found straight away.
            crate::settings::set("libre_hardware_monitor_path", serde_json::Value::String(path.clone()));
            open_target(find_libre_hardware_monitor(&path), "Libre Hardware Monitor")
        }
        "lhm" => {
            let configured = crate::settings::get_str("libre_hardware_monitor_path").unwrap_or_default();
            match find_libre_hardware_monitor(&configured) {
                None => {
                    // Not found: ask where it is
                    // Asking may itself fail to open it; that message is shown by the inner call.
                    open_from_tray("find-lhm");
                    return;
                }
                t => open_target(t, "Libre Hardware Monitor"),
            }
        }
        _ => open_target(find_framework_control(), "Framework Control"),
    };
    if let Some(m) = message {
        show_error(&m);
    }
}

/// Ask the user to pick LibreHardwareMonitor.exe.
///
/// The file picker starts in the Desktop (or Downloads / Documents). Returns the chosen path, or None
/// if cancelled (or away from Windows).
pub fn ask_for_lhm() -> Option<String> {
    #[cfg(windows)]
    {
        // Start the file picker in the Desktop (the first known folder that exists).
        let start = known_folders().into_iter().find(|f| f.is_dir());
        let mut dialog = rfd::FileDialog::new()
            .set_title("Where is LibreHardwareMonitor.exe?")
            .add_filter("Programs", &["exe"])
            .add_filter("All files", &["*"]);
        if let Some(s) = start {
            dialog = dialog.set_directory(s);
        }
        let path = dialog.pick_file()?;
        wr_core::info!("Libre Hardware Monitor chosen at {}", path.display());
        Some(path.display().to_string())
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Logs an error and, on Windows, shows it in an error message box.
pub fn show_error(message: &str) {
    wr_core::warn!("{message}");
    #[cfg(windows)]
    {
        let _ = rfd::MessageDialog::new()
            .set_title("White Rabbit")
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

///
/// These can be moved (for example into OneDrive), so asking Windows is more reliable than guessing
/// from %USERPROFILE%. A folder Windows can't name is left out.
/// Where Windows says the Desktop, Downloads and Documents folders really are.
fn known_folders() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::core::GUID;
        use windows_sys::Win32::System::Com::CoTaskMemFree;
        use windows_sys::Win32::UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, SHGetKnownFolderPath,
        };
        let ids: [&GUID; 3] = [&FOLDERID_Desktop, &FOLDERID_Downloads, &FOLDERID_Documents];
        let mut out = Vec::new();
        for id in ids {
            let mut path: *mut u16 = std::ptr::null_mut();
            // SAFETY: id points at a GUID; path receives a string Windows allocates, freed below.
            unsafe {
                if SHGetKnownFolderPath(id, 0, std::ptr::null_mut(), &mut path) == 0 && !path.is_null() {
                    // The string is null-terminated UTF-16; count up to the terminator to make a slice of it.
                    // Windows allocated it with the COM allocator, so CoTaskMemFree is the right way to free
                    // it (even on failure, as the docs ask).
                    let len = (0..).take_while(|&i| *path.add(i) != 0).count();
                    out.push(PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(path, len))));
                }
                if !path.is_null() {
                    CoTaskMemFree(path as _);
                }
            }
        }
        out
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

///
/// Walks a snapshot of all processes (Toolhelp32) and returns the first one whose exe file name
/// `matches` accepts. None if none does or the snapshot fails.
/// A running process whose exe name matches.
fn running_pid(matches: fn(&str) -> bool) -> Option<u32> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        };
        // SAFETY: a snapshot handle, walked with a correctly sized entry and closed at the end.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return None;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            // Windows requires dwSize to be set to the struct's size before the first call.
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut found = None;
            let mut ok = Process32FirstW(snap, &mut entry) != 0;
            while ok {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                // The first match wins; it is enough to know one copy is running.
                if matches(&name) {
                    found = Some(entry.th32ProcessID);
                    break;
                }
                ok = Process32NextW(snap, &mut entry) != 0;
            }
            // Close the snapshot on both paths (found or not); the loop only breaks, never returns early.
            CloseHandle(snap);
            found
        }
    }
    #[cfg(not(windows))]
    {
        let _ = matches;
        None
    }
}

///
/// Looks at every top-level window of process `pid` whose title contains `title_part` (ignoring
/// case), shows and restores it and asks Windows to bring it to the front. True as soon as one is
/// visible afterwards.
/// Restores and brings forward a running program's main window.
#[allow(unused_variables)]
fn show_window_of(pid: u32, title_part: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, ShowWindow,
            SW_RESTORE, SW_SHOW,
        };
        /// What the window callback looks for, and the windows it found.
        struct Search {
            pid: u32,
            title: String,
            found: Vec<HWND>,
        }
        /// Called by EnumWindows for each top-level window; returns 1 to keep going.
        unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> i32 {
            // SAFETY: lparam is the Search passed to EnumWindows below, alive for the whole call.
            let search = unsafe { &mut *(lparam as *mut Search) };
            let mut window_pid = 0u32;
            let mut buf = [0u16; 256];
            // SAFETY: hwnd comes from EnumWindows; the buffer and its length go together.
            unsafe {
                // Keep only this process's windows whose title has the wanted text (LHM's main window, not its
                // hidden helper windows).
                GetWindowThreadProcessId(hwnd, &mut window_pid);
                if window_pid == search.pid {
                    let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
                    let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]).to_lowercase();
                    if title.contains(&search.title) {
                        search.found.push(hwnd);
                    }
                }
            }
            1
        }
        let mut search = Search { pid, title: title_part.to_lowercase(), found: Vec::new() };
        // SAFETY: the callback only uses search, which outlives the call.
        unsafe {
            EnumWindows(Some(each), &mut search as *mut Search as LPARAM);
            for hwnd in search.found {
                // SW_SHOW for a hidden window, SW_RESTORE for a minimised one. SetForegroundWindow may be refused
                // by Windows' focus rules, in which case the window just flashes on the taskbar.
                ShowWindow(hwnd, SW_SHOW);
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
                if IsWindowVisible(hwnd) != 0 {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Tests for finding Libre Hardware Monitor and Framework Control.
#[cfg(test)]
mod tests {
    use super::*;

    /// LHM exe names are recognised in any case and with suffixes; other programs and files aren't.
    #[test]
    fn lhm_names() {
        assert!(is_lhm_exe("LibreHardwareMonitor.exe"));
        assert!(is_lhm_exe("LibreHardwareMonitor_x64.EXE"));
        assert!(!is_lhm_exe("OpenHardwareMonitor.exe"));
        assert!(!is_lhm_exe("LibreHardwareMonitor.dll"));
    }

    /// The port is read from the service's XML, and a file without it gives None.
    #[test]
    fn framework_control_port_from_xml() {
        let dir = std::env::temp_dir().join(format!("wr-fc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("FrameworkControlService.xml");
        std::fs::write(&file, r#"<service><env name="FRAMEWORK_CONTROL_PORT" value="8090"/></service>"#).unwrap();
        assert_eq!(read_fc_port(&file), Some(8090));
        std::fs::write(&file, r#"<service><env name="OTHER" value="1"/></service>"#).unwrap();
        assert_eq!(read_fc_port(&file), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An unzipped copy two folders deep in a "...HardwareMonitor..." folder is found.
    #[test]
    fn finds_lhm_in_a_hardware_monitor_folder() {
        let dir = std::env::temp_dir().join(format!("wr-lhm-{}", std::process::id()));
        let deep = dir.join("LibreHardwareMonitor-net472").join("LibreHardwareMonitor");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join(LHM_EXE), b"").unwrap();
        assert_eq!(find_exe_under(&dir, false), Some(deep.join(LHM_EXE)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
