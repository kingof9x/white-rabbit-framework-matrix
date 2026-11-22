//! The audio show's Ctrl+Alt+Shift+M shortcut. It is watched (a low-level keyboard hook) rather than
//! registered as a hotkey, so White Rabbit's own Ctrl+Alt+Shift+M for the LED panels still works too.
//! Both the window and the background part watch it; whichever is driving the keyboard answers.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Turns the shortcut on or off (the "Ctrl+Alt+Shift+M" switch on the Audio show page).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Release);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// Starts watching for the shortcut on a thread of its own. `on_press` runs on a new thread each time,
/// so it may take its time. Does nothing away from Windows.
pub fn watch(on_press: impl Fn() + Send + Sync + 'static) {
    imp::watch(std::sync::Arc::new(on_press));
}

#[cfg(not(windows))]
mod imp {
    pub fn watch(_on_press: std::sync::Arc<dyn Fn() + Send + Sync>) {}
}

#[cfg(windows)]
mod imp {
    use super::enabled;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage,
        HC_ACTION, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    const VK_M: u32 = 0x4D;
    const WM_PRESSED: u32 = WM_APP + 7;
    /// The watching thread, which the hook tells about a press.
    static THREAD: AtomicU32 = AtomicU32::new(0);
    static M_DOWN: AtomicBool = AtomicBool::new(false);

    pub fn watch(on_press: Arc<dyn Fn() + Send + Sync>) {
        let started = std::thread::Builder::new().name("audio-shortcut".into()).spawn(move || {
            // SAFETY: the hook is set on this thread, which runs a message loop for it until the app exits.
            unsafe {
                THREAD.store(GetCurrentThreadId(), Ordering::Release);
                let hook =
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), GetModuleHandleW(std::ptr::null()), 0);
                if hook.is_null() {
                    complain("Couldn't watch for Ctrl+Alt+Shift+M");
                    return;
                }
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.hwnd.is_null() && msg.message == WM_PRESSED {
                        let on_press = on_press.clone();
                        std::thread::spawn(move || on_press());
                        continue;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        });
        if started.is_err() {
            complain("Couldn't start watching for Ctrl+Alt+Shift+M");
        }
    }

    fn complain(message: &str) {
        eprintln!("{message}");
    }

    /// Sees every key press without taking any. Kept short: Windows drops a slow hook.
    unsafe extern "system" fn keyboard_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 && enabled() {
            // SAFETY: Windows passes a KBDLLHOOKSTRUCT with this hook.
            let info = unsafe { &*(lp as *const KBDLLHOOKSTRUCT) };
            if info.vkCode == VK_M {
                let msg = wp as u32;
                if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                    // SAFETY: plain calls.
                    let held = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } < 0;
                    let first = !M_DOWN.swap(true, Ordering::AcqRel);
                    if first && held(VK_CONTROL) && held(VK_MENU) && held(VK_SHIFT) {
                        // SAFETY: posting to our own thread.
                        unsafe { PostThreadMessageW(THREAD.load(Ordering::Acquire), WM_PRESSED, 0, 0) };
                    }
                } else if msg == WM_KEYUP || msg == WM_SYSKEYUP {
                    M_DOWN.store(false, Ordering::Release);
                }
            }
        }
        // SAFETY: passes the key on unchanged.
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wp, lp) }
    }
}
