//! When the panels turn off: the lid closed, the screen off or the PC going to sleep, and the laptop's
//! own screen not in use (docked with the lid shut, or Win+P "Second screen only"). A port of
//! `PowerState` and `builtin_screen_unused` in `led_mon/power_events.py`. Windows' messages are
//! handled in `ui.rs`; this part is the same everywhere so it can be tested.
//!
//! How it fits together: the tray window (`ui.rs`, main thread) registers for Windows power
//! notifications (WM_POWERBROADCAST with lid-switch and console-display GUIDs) and display changes.
//! Each message is turned into a `PowerEvent` and fed to `PowerState::handle`; when the answer
//! changes, `wants_sleep` (with the two user settings) decides whether the main loop should darken the
//! panels. Nothing here calls Windows or touches other threads.

/// DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY values (wingdi.h) for a screen built into the laptop:
/// LVDS, embedded DisplayPort, embedded UDI and "internal".
///
/// Values: 6 = LVDS, 11 = DISPLAYPORT_EMBEDDED, 13 = UDI_EMBEDDED, 0x8000_0000 = INTERNAL.
pub const BUILT_IN_OUTPUTS: [u32; 4] = [6, 11, 13, 0x8000_0000];

/// True when the laptop has a built-in screen and Windows isn't showing anything on it.
///
/// `active`: the output technology of each screen Windows is using now; `all`: of every screen it knows
/// about. False when the built-in screen is in use, when there is no built-in screen (a desktop PC), or
/// when Windows uses no screen at all (lid closed, nothing plugged in: the lid setting handles that).
pub fn builtin_screen_unused(active: &[u32], all: &[u32]) -> bool {
    let built_in = |o: &u32| BUILT_IN_OUTPUTS.contains(o);
    // Three conditions: something is shown, none of it on a built-in screen, and a built-in screen exists.
    !active.is_empty() && !active.iter().any(built_in) && all.iter().any(built_in)
}

/// A change Windows tells us about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PowerEvent {
    /// The PC is about to sleep or hibernate (PBT_APMSUSPEND).
    Suspend,
    /// The PC woke up again (PBT_APMRESUMEAUTOMATIC / PBT_APMRESUMESUSPEND).
    Resume,
    /// The lid switch: 0 closed, 1 open.
    /// `None` when Windows sent a value we could not read; that counts as open.
    Lid(Option<u8>),
    /// The screen: 0 off, 1 on, 2 dimmed.
    /// `None` (unreadable) counts as on.
    Display(Option<u8>),
    /// Whether the laptop's own screen is unused (QueryDisplayConfig).
    BuiltinUnused(bool),
}

/// What Windows last told us.
///
/// Kept by the tray window; all fields start false (lid open, screen on, awake).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerState {
    /// The lid switch last reported closed.
    pub lid_closed: bool,
    /// The console display last reported off (dimmed counts as on).
    pub display_off: bool,
    /// Between a suspend and the following resume.
    pub suspending: bool,
    /// The laptop's own screen is not shown on, while another screen is.
    pub builtin_unused: bool,
}

impl PowerState {
    /// The lid is closed, the screen is off or the PC is going to sleep ("Turn off when lid closes").
    pub fn should_sleep(&self) -> bool {
        self.lid_closed || self.display_off || self.suspending
    }

    /// Should the panels be off, given the two settings?
    ///
    /// `with_lid`: the "turn off with the lid/screen" setting; `when_screen_unused`: the "turn off when
    /// the laptop screen isn't used" setting. Each setting only reacts to its own reasons.
    pub fn wants_sleep(&self, with_lid: bool, when_screen_unused: bool) -> bool {
        (with_lid && self.should_sleep()) || (when_screen_unused && self.builtin_unused)
    }

    /// Applies one event. True when `should_sleep` or `builtin_unused` changed.
    /// The caller uses the return value to act (and log) only on a real change, since Windows often
    /// repeats the same notification.
    pub fn handle(&mut self, event: PowerEvent) -> bool {
        let before = (self.should_sleep(), self.builtin_unused);
        match event {
            PowerEvent::Suspend => self.suspending = true,
            PowerEvent::Resume => self.suspending = false,
            PowerEvent::Lid(v) => self.lid_closed = v == Some(0),
            // Dimmed (2) still counts as on: the screen is still showing something
            PowerEvent::Display(v) => self.display_off = v == Some(0),
            PowerEvent::BuiltinUnused(unused) => self.builtin_unused = unused,
        }
        (self.should_sleep(), self.builtin_unused) != before
    }

    /// A short English description of the current state for the log, e.g. "lid closed, screen off".
    pub fn describe(&self) -> String {
        let reasons: Vec<&str> = [
            ("lid closed", self.lid_closed),
            ("screen off", self.display_off),
            ("going to sleep", self.suspending),
            ("laptop screen not in use", self.builtin_unused),
        ]
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect();
        if reasons.is_empty() {
            "lid open and screen on".into()
        } else {
            reasons.join(", ")
        }
    }
}

/// Tests for the platform-neutral power logic.
#[cfg(test)]
mod tests {
    use super::*;

    /// Embedded DisplayPort: a typical built-in laptop screen.
    const EDP: u32 = 11;
    /// HDMI: a typical external screen.
    const HDMI: u32 = 5;

    /// "Laptop screen unused" needs an external screen in use and a built-in screen present but idle.
    #[test]
    fn the_built_in_screen_is_unused_only_when_another_screen_is_used_instead() {
        assert!(builtin_screen_unused(&[HDMI], &[HDMI, EDP]));
        assert!(!builtin_screen_unused(&[EDP, HDMI], &[HDMI, EDP]));
        // A desktop PC, no built-in screen
        assert!(!builtin_screen_unused(&[HDMI], &[HDMI]));
        // No screen in use at all: the lid handles that
        assert!(!builtin_screen_unused(&[], &[EDP]));
        assert!(builtin_screen_unused(&[HDMI], &[0x8000_0000]));
    }

    /// Each setting reacts only to its own reasons, `handle` reports only real changes, dimming does
    /// not count as off, and `describe` lists the active reasons.
    #[test]
    fn each_setting_turns_off_for_its_own_reason() {
        let mut p = PowerState::default();
        assert!(p.handle(PowerEvent::BuiltinUnused(true)));
        assert!(!p.handle(PowerEvent::BuiltinUnused(true)));
        assert!(p.wants_sleep(false, true));
        assert!(!p.wants_sleep(true, false));
        assert!(p.handle(PowerEvent::Lid(Some(0))));
        assert!(p.wants_sleep(true, false));
        assert!(!p.handle(PowerEvent::Display(Some(2))));
        assert_eq!(p.describe(), "lid closed, laptop screen not in use");
        p.handle(PowerEvent::Lid(Some(1)));
        p.handle(PowerEvent::BuiltinUnused(false));
        assert!(!p.wants_sleep(true, true));
        assert_eq!(p.describe(), "lid open and screen on");
        assert!(p.handle(PowerEvent::Suspend));
        assert!(p.handle(PowerEvent::Resume));
    }
}
