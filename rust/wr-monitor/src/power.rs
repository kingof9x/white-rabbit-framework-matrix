//! When the panels turn off: the lid closed, the screen off or the PC going to sleep, and the laptop's
//! own screen not in use (docked with the lid shut, or Win+P "Second screen only"). A port of
//! `PowerState` and `builtin_screen_unused` in `led_mon/power_events.py`. Windows' messages are
//! handled in `ui.rs`; this part is the same everywhere so it can be tested.

/// DISPLAYCONFIG_VIDEO_OUTPUT_TECHNOLOGY values (wingdi.h) for a screen built into the laptop:
/// LVDS, embedded DisplayPort, embedded UDI and "internal".
pub const BUILT_IN_OUTPUTS: [u32; 4] = [6, 11, 13, 0x8000_0000];

/// True when the laptop has a built-in screen and Windows isn't showing anything on it.
///
/// `active`: the output technology of each screen Windows is using now; `all`: of every screen it knows
/// about. False when the built-in screen is in use, when there is no built-in screen (a desktop PC), or
/// when Windows uses no screen at all (lid closed, nothing plugged in: the lid setting handles that).
pub fn builtin_screen_unused(active: &[u32], all: &[u32]) -> bool {
    let built_in = |o: &u32| BUILT_IN_OUTPUTS.contains(o);
    !active.is_empty() && !active.iter().any(built_in) && all.iter().any(built_in)
}

/// A change Windows tells us about.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PowerEvent {
    Suspend,
    Resume,
    /// The lid switch: 0 closed, 1 open.
    Lid(Option<u8>),
    /// The screen: 0 off, 1 on, 2 dimmed.
    Display(Option<u8>),
    /// Whether the laptop's own screen is unused (QueryDisplayConfig).
    BuiltinUnused(bool),
}

/// What Windows last told us.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerState {
    pub lid_closed: bool,
    pub display_off: bool,
    pub suspending: bool,
    pub builtin_unused: bool,
}

impl PowerState {
    /// The lid is closed, the screen is off or the PC is going to sleep ("Turn off when lid closes").
    pub fn should_sleep(&self) -> bool {
        self.lid_closed || self.display_off || self.suspending
    }

    /// Should the panels be off, given the two settings?
    pub fn wants_sleep(&self, with_lid: bool, when_screen_unused: bool) -> bool {
        (with_lid && self.should_sleep()) || (when_screen_unused && self.builtin_unused)
    }

    /// Applies one event. True when `should_sleep` or `builtin_unused` changed.
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

#[cfg(test)]
mod tests {
    use super::*;

    const EDP: u32 = 11;
    const HDMI: u32 = 5;

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
