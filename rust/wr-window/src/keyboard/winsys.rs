//! The keyboard part's links to Windows from the window: the file pickers, opening a page or program,
//! and telling White Rabbit's background part (the monitor) about the "Open an app" keys.
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
    /// The "Open an app" programs changed: they are already saved in the keyboard settings; the
    /// monitor is asked to read them again.
    pub fn set_apps(&self, _apps: Vec<String>) {
        tell_monitor();
    }
    pub fn set_tray(&self, _on: bool) {}
    pub fn set_audio_hotkey(&self, _on: bool) {
        tell_monitor();
    }
    pub fn close(&self) {}
}

/// Asks the monitor to read the keyboard settings again (in the background: the window never waits).
fn tell_monitor() {
    std::thread::spawn(|| {
        crate::link::request(serde_json::json!({"cmd": "keyboard"}), std::time::Duration::from_secs(3));
    });
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
