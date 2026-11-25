//! The keyboard part's links to Windows from the window: the file pickers, and telling White Rabbit's
//! background part (the monitor) about the "Open an app" keys and the audio show's shortcut.
//!
//! "Open an app" keys send Ctrl+Shift+Alt+F13 to F24 (see `keycodes::app_code`). The programs are kept
//! in the keyboard settings, which the monitor reads: it registers those keys and starts the
//! programs, so they work with the window closed too.
//!
//! Who calls this: the keyboard pages (`keyboard/mod.rs` and `keyboard/keyedit.rs`) on the UI thread.
//! Most of `Background` is deliberately empty. These calls come from the stand-alone Keyboard Control
//! app, which ran its own hotkey thread and tray; in White Rabbit the monitor process does that job
//! by watching the keyboard settings file, so the
//! window only has to save the settings (done elsewhere, in `keyedit.rs`) and the methods here stay
//! as no-ops that keep the callers' shape. The one live call is the audio show shortcut switch, which
//! flips a process-wide flag in `wr_keyboard::shortcut`.
//!
//! The two `browse_for_*` functions open Windows' Open dialog through `crate::dialogs::open_file`.
//! Away from Windows (tests, screenshots) that dialog picks nothing and they return None.

/// What the window tells the background part.
///
/// It holds no data: it is a handle the keyboard state keeps (`bg` field) so the settings code can
/// call "tell the background" in one place. See the file header for why most methods do nothing.
pub struct Background;

impl Background {
    /// Makes the handle. Starts no thread and touches nothing outside the process.
    pub fn start() -> Self {
        Background
    }
    /// The "Open an app" programs changed. They go into the keyboard settings, and the background part
    /// notices the file changed within a second and registers the keys again.
    ///
    /// Nothing to do here: `_apps` is ignored because the caller has already put the list in the
    /// settings it saves. Kept so `KeyState::sync_background` reads the same as when this did work.
    pub fn set_apps(&self, _apps: Vec<String>) {}
    /// Whether a tray icon should show. A no-op: White Rabbit's own tray is the only one (`HAS_TRAY`).
    pub fn set_tray(&self, _on: bool) {}
    /// Ctrl+Alt+Shift+M for the audio show: watched here while the window is open, and by the
    /// background part (which reads the setting from the keyboard settings) while it is closed.
    pub fn set_audio_hotkey(&self, on: bool) {
        // A shared atomic flag; the low-level keyboard hook checks it on every key press, so the change
        // takes effect immediately with no message passing.
        wr_keyboard::shortcut::set_enabled(on);
    }
    /// Called when the window closes. Nothing to stop, since `start` started nothing.
    pub fn close(&self) {}
}

/// Whether Windows should start the keyboard app at sign-in. A no-op: in White Rabbit the keyboard
/// part has no tray of its own (`HAS_TRAY` is false), so the caller always passes false, and only
/// when the wanted value changes (it remembers the last one in `last_autostart`).
pub fn set_autostart(_on: bool) {}

/// Windows' Open dialog, for picking the program an "Open an app" key starts.
///
/// Offers programs, shortcuts and batch files (.exe, .lnk, .bat, .cmd) first, with "All files" as a
/// fallback. Returns the full path as text, or None if the person cancelled (or away from Windows).
pub fn browse_for_app() -> Option<String> {
    crate::dialogs::open_file(
        "Pick the program this key opens",
        &[("Programs", &["exe", "lnk", "bat", "cmd"]), ("All files", &["*"])],
    )
    .map(|p| p.display().to_string())
}

/// Windows' Open dialog for the Firmware page's "Install a file…": picks a .uf2 firmware file.
/// Returns its full path, or None when cancelled. The file is checked later by `flash::check_uf2`
/// before anything is sent to a device, so picking a wrong file here is harmless.
pub fn browse_for_firmware() -> Option<String> {
    crate::dialogs::open_file("Pick the firmware file", &[("Firmware (.uf2)", &["uf2"]), ("All files", &["*"])])
        .map(|p| p.display().to_string())
}
