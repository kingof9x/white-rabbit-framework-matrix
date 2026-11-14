//! The Windows side: one hidden window that owns the tray icon and its menu, the keyboard shortcuts
//! (Ctrl+Alt+Shift + letter, registered with Windows so no keys are watched), and the power messages
//! that say the lid closed, the screen turned off or the PC is going to sleep.
//!
//! Whether the laptop's own screen is in use is asked with QueryDisplayConfig: the built-in screen
//! drops out of the active screens when Windows only uses an external one. That is checked when
//! Windows says the screens changed (WM_DISPLAYCHANGE), after the lid opens or closes, and every few
//! seconds as a safety net.

use std::ptr::{null, null_mut};
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Display::{
    GetDisplayConfigBufferSizes, QueryDisplayConfig, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, QDC_ALL_PATHS,
    QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Power::{RegisterPowerSettingNotification, POWERBROADCAST_SETTING};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::System::SystemServices::{GUID_CONSOLE_DISPLAY_STATE, GUID_LIDSWITCH_STATE_CHANGE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use wr_core::settings::{save_key, Settings};

use crate::monitor::Shared;
use crate::paths;
use crate::power::{builtin_screen_unused, PowerEvent, PowerState};
use wr_core::{info, warn};
use wr_panel::winports::wide;

const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;

// Menu item ids
const ID_ANIMATION: usize = 10;
const ID_IDENTIFY: usize = 11;
const ID_NEXT: usize = 12;
const ID_FREEZE: usize = 13;
const ID_FOLDER: usize = 20;
const ID_LOG: usize = 21;
const ID_RELOAD: usize = 22;
const ID_LID: usize = 30;
const ID_SCREEN_UNUSED: usize = 31;
const ID_QUIT: usize = 40;

/// The shortcuts this monitor handles so far (music mode, M, comes with the music visualizer).
const HOTKEYS: [char; 4] = ['i', 'n', 'f', 'u'];

const PBT_APMSUSPEND: usize = 0x0004;
const PBT_APMRESUMESUSPEND: usize = 0x0007;
const PBT_APMRESUMEAUTOMATIC: usize = 0x0012;
const PBT_POWERSETTINGCHANGE: usize = 0x8013;

// Timers for the built-in screen check: the regular one, and one more shortly after a display or lid
// change, as Windows can finish rearranging the screens later
const TIMER_PERIODIC: usize = 1;
const TIMER_RECHECK: usize = 2;
const DISPLAY_CHECK_MS: u32 = 5000;
const DISPLAY_RECHECK_MS: u32 = 1500;

struct Ui {
    shared: Arc<Shared>,
    power: std::sync::Mutex<PowerState>,
    taskbar_created: u32,
}

static UI: OnceLock<Ui> = OnceLock::new();

/// Takes the White Rabbit single-instance mutex. False when another White Rabbit monitor is running.
pub fn single_instance() -> bool {
    let name = wide(paths::SINGLE_INSTANCE_MUTEX);
    // SAFETY: the handle is kept for the life of the process on purpose.
    unsafe {
        let handle = CreateMutexW(null(), 0, name.as_ptr());
        !(handle.is_null() || GetLastError() == ERROR_ALREADY_EXISTS)
    }
}

pub fn show_error(message: &str) {
    let text = wide(message);
    let title = wide("White Rabbit (Rust test)");
    // SAFETY: plain message box.
    unsafe {
        MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING);
    }
}

fn taskbar_is_light() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("SystemUsesLightTheme");
    let mut data: u32 = 0;
    let mut size = 4u32;
    // SAFETY: a DWORD read into a u32 with its size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut data as *mut u32 as *mut _,
            &mut size,
        )
    };
    r == 0 && data != 0
}

fn make_icon(lit: bool) -> HICON {
    // SAFETY: plain metric read.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as usize;
    let (side, mut rgba) = crate::icon::tray_icon_rgba(size, lit, taskbar_is_light());
    // 1 bit per pixel, each row padded to 2 bytes; all clear, so the alpha channel decides
    let and_mask: Vec<u8> = vec![0; side.div_ceil(16) * 2 * side];
    for p in rgba.chunks_mut(4) {
        p.swap(0, 2); // RGBA -> BGRA
    }
    // SAFETY: both buffers are as big as the icon needs (side x side, 1 bit and 32 bits per pixel).
    unsafe { CreateIcon(null_mut(), side as i32, side as i32, 1, 32, and_mask.as_ptr(), rgba.as_ptr()) }
}

fn tooltip(paused: bool) -> String {
    if paused {
        "White Rabbit (Rust test) - stopped".into()
    } else {
        "White Rabbit (Rust test)".into()
    }
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    // SAFETY: an all-zero NOTIFYICONDATAW is valid; the size is set.
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = TRAY_ID;
    data
}

fn update_tray(hwnd: HWND, message: u32) {
    let ui = UI.get().unwrap();
    let paused = ui.shared.paused.load(Ordering::SeqCst);
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    data.hIcon = make_icon(!paused);
    let tip = wide(&tooltip(paused));
    data.szTip[..tip.len().min(127)].copy_from_slice(&tip[..tip.len().min(127)]);
    // SAFETY: the struct is filled in above.
    unsafe {
        if Shell_NotifyIconW(message, &data) == 0 && message == NIM_ADD {
            warn!("Windows didn't take the tray icon");
        }
        DestroyIcon(data.hIcon);
    }
}

fn open(path: &std::path::Path) {
    let op = wide("open");
    let file = wide(&path.display().to_string());
    // SAFETY: plain shell call.
    unsafe {
        ShellExecuteW(null_mut(), op.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

fn show_menu(hwnd: HWND) {
    let ui = UI.get().unwrap();
    let shared = &ui.shared;
    let paused = shared.paused.load(Ordering::SeqCst);
    let frozen = shared.frozen.load(Ordering::SeqCst);
    let (lid, screen_unused) = {
        let settings = shared.settings.lock().unwrap();
        (settings.turn_off_with_lid, settings.turn_off_when_screen_unused)
    };
    // SAFETY: the menu is created and destroyed here; strings live until the menu is shown.
    unsafe {
        let menu = CreatePopupMenu();
        let mut texts = Vec::new();
        let mut add = |flags: u32, id: usize, text: &str| {
            let t = wide(text);
            AppendMenuW(menu, flags, id, t.as_ptr());
            texts.push(t);
        };
        add(MF_STRING | MF_GRAYED, 0, "White Rabbit, Rust test build");
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, ID_ANIMATION, if paused { "Start animation" } else { "Stop animation" });
        add(MF_STRING, ID_IDENTIFY, "Identify panels");
        add(MF_STRING, ID_NEXT, "Next app");
        add(MF_STRING | if frozen { MF_CHECKED } else { 0 }, ID_FREEZE, "Freeze app switching");
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, ID_FOLDER, "Open config folder");
        add(MF_STRING, ID_LOG, "Open log");
        add(MF_STRING, ID_RELOAD, "Reload config");
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING | if lid { MF_CHECKED } else { 0 }, ID_LID, "Turn off when lid closes");
        add(
            MF_STRING | if screen_unused { MF_CHECKED } else { 0 },
            ID_SCREEN_UNUSED,
            "Turn off when laptop screen isn't used",
        );
        add(MF_SEPARATOR, 0, "");
        add(MF_STRING, ID_QUIT, "Quit");
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // The menu closes when clicking elsewhere only if the window is in front
        SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        drop(texts);
        if chosen > 0 {
            command(hwnd, chosen as usize);
        }
    }
}

fn command(hwnd: HWND, id: usize) {
    let shared = &UI.get().unwrap().shared;
    match id {
        ID_ANIMATION => {
            let paused = !shared.paused.load(Ordering::SeqCst);
            shared.paused.store(paused, Ordering::SeqCst);
            info!("Animation {}", if paused { "stopped" } else { "started" });
            update_tray(hwnd, NIM_MODIFY);
        }
        ID_IDENTIFY => shared.identify(3.0),
        ID_NEXT => shared.next_app.store(true, Ordering::SeqCst),
        ID_FREEZE => {
            let frozen = !shared.frozen.load(Ordering::SeqCst);
            shared.frozen.store(frozen, Ordering::SeqCst);
        }
        ID_FOLDER => {
            let _ = std::fs::create_dir_all(paths::config_dir());
            open(&paths::config_dir());
        }
        ID_LOG => open(&paths::log_file()),
        ID_RELOAD => shared.reload.store(true, Ordering::SeqCst),
        ID_LID => {
            let enabled = !shared.settings.lock().unwrap().turn_off_with_lid;
            if let Err(e) = save_key(&paths::settings_file(), "turn_off_with_lid", serde_json::Value::Bool(enabled)) {
                warn!("Couldn't save the lid setting: {e}");
            }
            shared.settings.lock().unwrap().turn_off_with_lid = enabled;
            info!("Turn off with lid {}", if enabled { "on" } else { "off" });
            apply_power();
        }
        ID_SCREEN_UNUSED => {
            let enabled = !shared.settings.lock().unwrap().turn_off_when_screen_unused;
            let key = "turn_off_when_screen_unused";
            if let Err(e) = save_key(&paths::settings_file(), key, serde_json::Value::Bool(enabled)) {
                warn!("Couldn't save the laptop screen setting: {e}");
            }
            shared.settings.lock().unwrap().turn_off_when_screen_unused = enabled;
            info!("Turn off when laptop screen isn't used {}", if enabled { "on" } else { "off" });
            apply_power();
        }
        ID_QUIT => {
            shared.quit.store(true, Ordering::SeqCst);
            // SAFETY: closes our own window, which ends the message loop.
            unsafe {
                DestroyWindow(hwnd);
            }
        }
        _ => {}
    }
}

/// Turns the panels off or back on from what Windows last said and the two settings.
fn apply_power() {
    let ui = UI.get().unwrap();
    let (with_lid, when_screen_unused) = {
        let settings = ui.shared.settings.lock().unwrap();
        (settings.turn_off_with_lid, settings.turn_off_when_screen_unused)
    };
    let sleep = ui.power.lock().unwrap().wants_sleep(with_lid, when_screen_unused);
    ui.shared.power_sleep.store(sleep, Ordering::SeqCst);
}

fn power_event(event: PowerEvent) {
    let ui = UI.get().unwrap();
    let (changed, text) = {
        let mut p = ui.power.lock().unwrap();
        (p.handle(event), p.describe())
    };
    if changed {
        info!("Power: {text}");
        // The window may have changed the settings since the last time they were read
        *ui.shared.settings.lock().unwrap() = Settings::load(&paths::settings_file());
        apply_power();
    }
}

/// The output technology of each screen QueryDisplayConfig lists with these flags, or None on failure.
fn display_outputs(flags: u32) -> Option<Vec<u32>> {
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    // The screens can change between the two calls
    for _ in 0..3 {
        let (mut paths, mut modes) = (0u32, 0u32);
        // SAFETY: two counts written by Windows.
        if unsafe { GetDisplayConfigBufferSizes(flags, &mut paths, &mut modes) } != 0 {
            return None;
        }
        // SAFETY: plain C structs, valid when zeroed.
        let mut path_array: Vec<DISPLAYCONFIG_PATH_INFO> = vec![unsafe { std::mem::zeroed() }; paths.max(1) as usize];
        let mut mode_array: Vec<DISPLAYCONFIG_MODE_INFO> = vec![unsafe { std::mem::zeroed() }; modes.max(1) as usize];
        // SAFETY: the arrays are as long as the counts say.
        let result = unsafe {
            QueryDisplayConfig(
                flags,
                &mut paths,
                path_array.as_mut_ptr(),
                &mut modes,
                mode_array.as_mut_ptr(),
                null_mut(),
            )
        };
        if result == 0 {
            return Some(path_array[..paths as usize].iter().map(|p| p.targetInfo.outputTechnology as u32).collect());
        }
        if result != ERROR_INSUFFICIENT_BUFFER {
            return None;
        }
    }
    None
}

/// Asks Windows whether the laptop's own screen is in use. "Can't tell" counts as in use.
fn check_builtin_screen() {
    let unused = match (display_outputs(QDC_ONLY_ACTIVE_PATHS), display_outputs(QDC_ALL_PATHS)) {
        (Some(active), Some(all)) => builtin_screen_unused(&active, &all),
        _ => false,
    };
    power_event(PowerEvent::BuiltinUnused(unused));
}

/// One more check of the built-in screen shortly, after a display or lid change.
fn recheck_soon(hwnd: HWND) {
    // SAFETY: a timer on our own window.
    unsafe {
        SetTimer(hwnd, TIMER_RECHECK, DISPLAY_RECHECK_MS, None);
    }
}

fn hotkey(letter: char) {
    let shared = &UI.get().unwrap().shared;
    match letter {
        'i' => shared.identify(3.0),
        'n' => shared.next_app.store(true, Ordering::SeqCst),
        'f' => shared.frozen.store(true, Ordering::SeqCst),
        'u' => shared.frozen.store(false, Ordering::SeqCst),
        _ => {}
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(ui) = UI.get() else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_TRAY => {
            let event = (lparam & 0xFFFF) as u32;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP || event == WM_CONTEXTMENU {
                show_menu(hwnd);
            }
            0
        }
        WM_HOTKEY => {
            if let Some(letter) = HOTKEYS.get(wparam.wrapping_sub(1)) {
                hotkey(*letter);
            }
            0
        }
        WM_POWERBROADCAST => {
            match wparam {
                PBT_POWERSETTINGCHANGE if lparam != 0 => {
                    // SAFETY: Windows passes a POWERBROADCAST_SETTING with this message.
                    let setting = &*(lparam as *const POWERBROADCAST_SETTING);
                    let value = (setting.DataLength >= 1).then(|| setting.Data[0]);
                    if same_guid(&setting.PowerSetting, &GUID_LIDSWITCH_STATE_CHANGE) {
                        power_event(PowerEvent::Lid(value));
                        check_builtin_screen();
                        recheck_soon(hwnd);
                    } else if same_guid(&setting.PowerSetting, &GUID_CONSOLE_DISPLAY_STATE) {
                        power_event(PowerEvent::Display(value));
                    }
                }
                PBT_APMSUSPEND => power_event(PowerEvent::Suspend),
                PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => power_event(PowerEvent::Resume),
                _ => {}
            }
            1
        }
        WM_DISPLAYCHANGE => {
            check_builtin_screen();
            recheck_soon(hwnd);
            0
        }
        WM_TIMER => {
            if wparam == TIMER_RECHECK {
                KillTimer(hwnd, TIMER_RECHECK);
            }
            check_builtin_screen();
            // The window may have changed either setting in settings.json (the main loop reads it again
            // every second), so apply them even when Windows said nothing new
            apply_power();
            0
        }
        WM_DESTROY => {
            KillTimer(hwnd, TIMER_PERIODIC);
            KillTimer(hwnd, TIMER_RECHECK);
            let data = tray_data(hwnd);
            Shell_NotifyIconW(NIM_DELETE, &data);
            ui.shared.quit.store(true, Ordering::SeqCst);
            PostQuitMessage(0);
            0
        }
        _ if msg == ui.taskbar_created && msg != 0 => {
            // Explorer restarted: put the icon back
            update_tray(hwnd, NIM_ADD);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// Shows the tray icon and runs the window's messages until Quit.
pub fn run(shared: Arc<Shared>) {
    let taskbar_created = {
        let name = wide("TaskbarCreated");
        // SAFETY: plain call.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    };
    let _ = UI.set(Ui { shared: shared.clone(), power: std::sync::Mutex::new(PowerState::default()), taskbar_created });
    let class = wide("WhiteRabbitRustMonitor");
    // SAFETY: standard window class and window creation; the window lives until Quit.
    unsafe {
        let instance = GetModuleHandleW(null());
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(window_proc);
        wc.hInstance = instance;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        let title = wide("White Rabbit");
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            warn!("Couldn't make the hidden window; no tray icon, shortcuts or lid events");
            return;
        }
        update_tray(hwnd, NIM_ADD);

        for guid in [GUID_LIDSWITCH_STATE_CHANGE, GUID_CONSOLE_DISPLAY_STATE] {
            if RegisterPowerSettingNotification(hwnd as HANDLE, &guid, 0) == 0 {
                warn!("Couldn't listen for the lid / screen turning off");
            }
        }
        check_builtin_screen();
        SetTimer(hwnd, TIMER_PERIODIC, DISPLAY_CHECK_MS, None);

        let settings = shared.settings.lock().unwrap().clone();
        let mut registered = Vec::new();
        for (i, letter) in HOTKEYS.iter().enumerate() {
            if !settings.shortcut_on(*letter) {
                continue;
            }
            let key = letter.to_ascii_uppercase() as u32;
            if RegisterHotKey(hwnd, i as i32 + 1, MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, key) != 0 {
                registered.push(letter.to_ascii_uppercase().to_string());
            } else {
                warn!(
                    "Couldn't register Ctrl+Alt+Shift+{}; another program may be using it",
                    letter.to_ascii_uppercase()
                );
            }
        }
        info!(
            "Keyboard shortcuts: Ctrl+Alt+Shift + {}",
            if registered.is_empty() { "none".into() } else { registered.join(", ") }
        );

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
