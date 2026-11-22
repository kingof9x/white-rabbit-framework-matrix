//! The keyboard part's links to Windows from the window: the file pickers, and telling White Rabbit's
//! background part (the monitor) about the "Open an app" keys and the audio show's shortcut.
//!
//! "Open an app" keys send Ctrl+Shift+Alt+F13 to F24 (see `keycodes::app_code`). The programs are kept
//! in the keyboard settings, which the monitor reads: it registers those keys and starts the
//! programs, so they work with the window closed too.

/// What the window tells the background part.
pub struct Background;

impl Background {
    pub fn start() -> Self {
        Background
    }
    /// The "Open an app" programs changed. They go into the keyboard settings, and the background part
    /// notices the file changed within a second and registers the keys again.
    pub fn set_apps(&self, _apps: Vec<String>) {}
    pub fn set_tray(&self, _on: bool) {}
    /// Ctrl+Alt+Shift+M for the audio show: watched here while the window is open, and by the
    /// background part (which reads the setting from the keyboard settings) while it is closed.
    pub fn set_audio_hotkey(&self, on: bool) {
        wr_keyboard::shortcut::set_enabled(on);
    }
    pub fn close(&self) {}
}

pub fn set_autostart(_on: bool) {}

/// Windows' Open dialog, for picking the program an "Open an app" key starts.
pub fn browse_for_app() -> Option<String> {
    crate::dialogs::open_file(
        "Pick the program this key opens",
        &[("Programs", &["exe", "lnk", "bat", "cmd"]), ("All files", &["*"])],
    )
    .map(|p| p.display().to_string())
}

pub fn browse_for_firmware() -> Option<String> {
    crate::dialogs::open_file("Pick the firmware file", &[("Firmware (.uf2)", &["uf2"]), ("All files", &["*"])])
        .map(|p| p.display().to_string())
}
