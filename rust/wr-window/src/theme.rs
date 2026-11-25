//! Following Windows' look: light or dark mode, the accent colour, "Make text bigger" and high contrast,
//! the way the Windows 11 Settings app does (`led_mon/theme.py`). Read again every two seconds, so a
//! change in Windows' settings shows straight away.
//!
//! The choice (system, light, dark or classic) is saved as "app_theme" in settings.json. Classic is the
//! plain Windows look, and is used whenever high contrast is on, so its colours are followed exactly.
//!
//! Away from Windows (screenshots, tests): WHITE_RABBIT_APPEARANCE=light|dark,
//! WHITE_RABBIT_ACCENT=#light,#dark, WHITE_RABBIT_TEXT_SCALE=1.25 and WHITE_RABBIT_HIGH_CONTRAST=1.

//!
//! How it fits together: `read_windows` reads Windows' settings (registry values under
//! HKEY_CURRENT_USER plus two system calls) into a `Windows` value; `resolve` combines that with the
//! user's choice into a mode and an accent colour; `apply` sets them on the Slint `Theme` global,
//! which every page's colours are bound to, so the whole window restyles at once. `app.rs` runs
//! `read_windows` every 2 seconds on the UI thread (a few registry reads, well under a millisecond)
//! and calls `apply` only when something changed. Colours are passed around as 0xRRGGBB numbers.
use slint::Color;

/// The settings.json key that holds the user's choice.
pub const SETTING: &str = "app_theme";
/// The choices in the order of the Settings page's list ("system" follows Windows' light/dark).
pub const CHOICES: [&str; 4] = ["system", "light", "dark", "classic"];
/// Windows' default blue accent: (light mode, dark mode)
pub const DEFAULT_ACCENT: (u32, u32) = (0x005fb8, 0x60cdff);

/// Windows' look settings, as read by `read_windows`. Compared as a whole to notice a change.
#[derive(Clone, Debug, PartialEq)]
pub struct Windows {
    /// Apps should use dark mode (Settings > Personalisation > Colours)
    pub dark: bool,
    /// (light mode, dark mode) shades, 0xRRGGBB
    pub accent: (u32, u32),
    /// A high-contrast theme is on: the window then uses the classic look with Windows' own colours
    pub high_contrast: bool,
    /// Windows' "Text size" as a factor, 1.0 to 2.25 (100% to 225%)
    pub text_scale: f32,
    /// Classic colours: window, text, face, shadow, grey text
    pub classic: [u32; 5],
}

impl Default for Windows {
    fn default() -> Self {
        Windows {
            dark: false,
            accent: DEFAULT_ACCENT,
            high_contrast: false,
            text_scale: 1.0,
            classic: [0xffffff, 0x000000, 0xf0f0f0, 0xa0a0a0, 0x6d6d6d],
        }
    }
}

/// A 0xRRGGBB number as a Slint colour (opaque).
pub fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// "#60cdff" or "60cdff" as a number; None if it isn't hex.
fn parse_hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches('#'), 16).ok()
}

/// Windows' AccentPalette (8 RGBA colours, lightest first) -> (light mode accent, dark mode accent).
/// Windows apps use the "Dark 1" shade on light backgrounds and "Light 2" on dark ones.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn accent_from_palette(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 32 {
        return None;
    }
    // Each palette entry is 4 bytes R, G, B, A; index 4 is "Dark 1", index 1 is "Light 2".
    let color = |i: usize| (data[i * 4] as u32) << 16 | (data[i * 4 + 1] as u32) << 8 | data[i * 4 + 2] as u32;
    Some((color(4), color(1)))
}

/// Red, green, blue (each 0..1) to hue, lightness, saturation (each 0..1): a copy of Python's
/// `colorsys.rgb_to_hls`, so the shades come out exactly as the Python app's did.
#[cfg_attr(not(windows), allow(dead_code))]
fn rgb_to_hls(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, l, 0.0);
    }
    let d = max - min;
    let s = if l <= 0.5 { d / (max + min) } else { d / (2.0 - max - min) };
    let rc = (max - r) / d;
    let gc = (max - g) / d;
    let bc = (max - b) / d;
    let h = if r == max {
        bc - gc
    } else if g == max {
        2.0 + rc - bc
    } else {
        4.0 + gc - rc
    };
    // h is in sixths of the colour wheel (and may be negative); turn it into 0..1.
    (((h / 6.0) % 1.0 + 1.0) % 1.0, l, s)
}

/// The reverse of `rgb_to_hls`, a copy of Python's `colorsys.hls_to_rgb`.
#[cfg_attr(not(windows), allow(dead_code))]
fn hls_to_rgb(h: f64, l: f64, s: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        return (l, l, l);
    }
    let m2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let m1 = 2.0 * l - m2;
    // One colour channel for a hue: rises over the first sixth, full for the next two, falls over
    // the next sixth, low for the last third. Red, green and blue are this curve shifted by a third.
    let v = |hue: f64| {
        let hue = hue.rem_euclid(1.0);
        if hue < 1.0 / 6.0 {
            m1 + (m2 - m1) * hue * 6.0
        } else if hue < 0.5 {
            m2
        } else if hue < 2.0 / 3.0 {
            m1 + (m2 - m1) * (2.0 / 3.0 - hue) * 6.0
        } else {
            m1
        }
    };
    (v(h + 1.0 / 3.0), v(h), v(h - 1.0 / 3.0))
}

///
/// Used when Windows has no AccentPalette. Keeps the hue and saturation and changes the lightness:
/// at most 0.38 for light mode (dark enough to read on white), at least 0.7 for dark mode.
/// A single accent colour (DWM AccentColor, 0xAABBGGRR) -> (light mode, dark mode) shades.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn accent_from_color(abgr: u32) -> (u32, u32) {
    let (r, g, b) = ((abgr & 0xff) as f64, ((abgr >> 8) & 0xff) as f64, ((abgr >> 16) & 0xff) as f64);
    let (h, l, s) = rgb_to_hls(r / 255.0, g / 255.0, b / 255.0);
    let shade = |lum: f64| {
        let (r, g, b) = hls_to_rgb(h, lum, s);
        let c = |v: f64| (v * 255.0) as u32;
        c(r) << 16 | c(g) << 8 | c(b)
    };
    (shade(l.min(0.38)), shade(l.max(0.7)))
}

/// The Windows calls: registry values under HKEY_CURRENT_USER, the high-contrast flag and system
/// colours. Each returns None / false / a default when Windows says no, so a missing value is
/// never an error.
#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Graphics::Gdi::GetSysColor;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD};
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

    /// A Rust string as null-terminated UTF-16 for the Win32 "W" functions.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A 32-bit number from the registry; None if the key or value is missing or isn't a DWORD
    /// (RRF_RT_REG_DWORD makes Windows check the type).
    pub fn dword(key: &str, name: &str) -> Option<u32> {
        let (k, n) = (wide(key), wide(name));
        let mut value = 0u32;
        let mut size = 4u32;
        // SAFETY: the strings are null-terminated and value/size describe a 4-byte buffer.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        };
        (r == ERROR_SUCCESS).then_some(value)
    }

    /// Binary data (up to 64 bytes) from the registry; None if missing, of another type or longer.
    pub fn binary(key: &str, name: &str) -> Option<Vec<u8>> {
        let (k, n) = (wide(key), wide(name));
        let mut buf = vec![0u8; 64];
        let mut size = buf.len() as u32;
        // SAFETY: the strings are null-terminated and buf/size describe the buffer.
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                k.as_ptr(),
                n.as_ptr(),
                RRF_RT_REG_BINARY,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (r == ERROR_SUCCESS).then(|| {
            buf.truncate(size as usize);
            buf
        })
    }

    /// Whether a high-contrast theme is on, from SystemParametersInfo.
    pub fn high_contrast() -> bool {
        // SAFETY: HIGHCONTRASTW is a plain C struct of numbers and a pointer, for which all zeros is valid.
        let mut hc: HIGHCONTRASTW = unsafe { std::mem::zeroed() };
        hc.cbSize = std::mem::size_of::<HIGHCONTRASTW>() as u32;
        // SAFETY: hc is a HIGHCONTRASTW with its size set.
        let ok =
            unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&mut hc as *mut HIGHCONTRASTW).cast(), 0) };
        ok != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
    }

    /// A Windows system colour (COLORREF 0x00BBGGRR) as 0xRRGGBB.
    pub fn sys_color(index: i32) -> u32 {
        // SAFETY: plain call.
        let c = unsafe { GetSysColor(index) };
        // Swap the red and blue bytes: COLORREF keeps red lowest.
        (c & 0xff) << 16 | (c & 0xff00) | (c >> 16) & 0xff
    }
}

///
/// On Windows from the registry and system calls; the WHITE_RABBIT_* variables listed at the top of
/// this file then override parts (anywhere), so screenshots can show any look.
/// Windows' look settings now.
pub fn read_windows() -> Windows {
    let mut found = Windows::default();
    #[cfg(windows)]
    {
        found.dark = win::dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme")
            // AppsUseLightTheme = 0 means dark mode; missing means light.
            .is_some_and(|v| v == 0);
        // The palette Windows made from the accent colour; if missing, work shades out from the plain
        // accent colour; if that's missing too, Windows' default blue.
        found.accent = win::binary(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentPalette")
            .and_then(|d| accent_from_palette(&d))
            .or_else(|| win::dword(r"Software\Microsoft\Windows\DWM", "AccentColor").map(accent_from_color))
            .unwrap_or(DEFAULT_ACCENT);
        // Percent, 100..225.
        let scale = win::dword(r"Software\Microsoft\Accessibility", "TextScaleFactor").unwrap_or(100);
        found.text_scale = (scale as f32 / 100.0).clamp(1.0, 2.25);
        found.high_contrast = win::high_contrast();
        // COLOR_WINDOW 5, COLOR_WINDOWTEXT 8, COLOR_BTNFACE 15, COLOR_BTNSHADOW 16, COLOR_GRAYTEXT 17
        found.classic =
            [win::sys_color(5), win::sys_color(8), win::sys_color(15), win::sys_color(16), win::sys_color(17)];
    }
    if let Ok(v) = std::env::var("WHITE_RABBIT_APPEARANCE") {
        if v == "light" || v == "dark" {
            found.dark = v == "dark";
        }
    }
    if let Ok(v) = std::env::var("WHITE_RABBIT_ACCENT") {
        if let Some((a, b)) = v.split_once(',') {
            if let (Some(a), Some(b)) = (parse_hex(a), parse_hex(b)) {
                found.accent = (a, b);
            }
        }
    }
    if let Some(v) = std::env::var("WHITE_RABBIT_TEXT_SCALE").ok().and_then(|v| v.parse().ok()) {
        found.text_scale = v;
    }
    if let Ok(v) = std::env::var("WHITE_RABBIT_HIGH_CONTRAST") {
        found.high_contrast = v == "1";
    }
    found
}

///
/// High contrast always wins (classic), then an explicit light or dark choice, then "system"
/// follows Windows' dark setting. The accent is the shade for that mode.
/// (mode, accent) for a choice: mode is light, dark or classic.
pub fn resolve(choice: &str, windows: &Windows) -> (&'static str, u32) {
    let mode = if windows.high_contrast || choice == "classic" {
        "classic"
    } else if choice == "light" {
        "light"
    } else if choice == "dark" || windows.dark {
        "dark"
    } else {
        "light"
    };
    let accent = if mode == "dark" { windows.accent.1 } else { windows.accent.0 };
    (mode, accent)
}

/// The saved choice; "system" if none is saved or it isn't one of `CHOICES`.
pub fn load_choice() -> String {
    crate::settings::get_str(SETTING).filter(|c| CHOICES.contains(&c.as_str())).unwrap_or_else(|| "system".into())
}

///
/// In dark mode the accent is always the light shade, so text on it is black. Otherwise the accent's
/// brightness (luma, 0..255, with the usual 0.299/0.587/0.114 weights) decides: above 160, black.
/// Text on the accent colour: dark on a light accent, white on a dark one.
pub fn on_accent(mode: &str, accent: u32) -> u32 {
    if mode == "dark" {
        return 0x000000;
    }
    let (r, g, b) = ((accent >> 16) & 0xff, (accent >> 8) & 0xff, accent & 0xff);
    let luma = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    if luma > 160.0 {
        0x000000
    } else {
        0xffffff
    }
}

///
/// On the UI thread. In high contrast the accent becomes the high-contrast text colour, so nothing
/// on screen uses a colour the user didn't pick.
/// Puts the look into the window.
pub fn apply(app: &crate::AppWindow, choice: &str, windows: &Windows) {
    use slint::ComponentHandle;
    let (mode, accent) = resolve(choice, windows);
    let t = app.global::<crate::Theme>();
    t.set_mode(mode.into());
    t.set_accent(rgb(if mode == "classic" && windows.high_contrast { windows.classic[1] } else { accent }));
    t.set_on_accent(rgb(on_accent(mode, accent)));
    t.set_text_scale(windows.text_scale);
    t.set_classic_window(rgb(windows.classic[0]));
    t.set_classic_text(rgb(windows.classic[1]));
    t.set_classic_face(rgb(windows.classic[2]));
    t.set_classic_shadow(rgb(windows.classic[3]));
    t.set_classic_gray_text(rgb(windows.classic[4]));
    title_bar(app, mode == "dark");
}

///
/// The title bar is drawn by Windows, not Slint, so it needs this call; attribute 20 is the documented
/// value of DWMWA_USE_IMMERSIVE_DARK_MODE on Windows 11 and later Windows 10 builds. Older Windows
/// just ignores it. Does nothing before the window has a native handle.
/// A dark title bar in dark mode (DWMWA_USE_IMMERSIVE_DARK_MODE).
#[allow(unused_variables)]
pub fn title_bar(app: &crate::AppWindow, dark: bool) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use slint::ComponentHandle;
        let wh = app.window().window_handle();
        let Ok(handle) = wh.window_handle() else { return };
        if let RawWindowHandle::Win32(h) = handle.as_raw() {
            let value: i32 = dark as i32;
            // SAFETY: the window handle is live and value is a 4-byte BOOL.
            unsafe {
                windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                    h.hwnd.get() as _,
                    20, // DWMWA_USE_IMMERSIVE_DARK_MODE
                    (&value as *const i32).cast(),
                    4,
                );
            }
        }
    }
}

/// Tests for following Windows' look.
#[cfg(test)]
mod tests {
    use super::*;

    /// The light and dark accents are taken from the right palette entries; a short palette gives None.
    #[test]
    fn accent_palette() {
        let mut data = vec![0u8; 32];
        data[4..7].copy_from_slice(&[0x99, 0xeb, 0xff]);
        data[16..19].copy_from_slice(&[0x00, 0x5a, 0x9e]);
        assert_eq!(accent_from_palette(&data), Some((0x005a9e, 0x99ebff)));
        assert_eq!(accent_from_palette(&[1, 2]), None);
    }

    /// Shades worked out from a single accent colour match the Python app's.
    #[test]
    fn accent_color_matches_python() {
        // Python: accent_from_color(0xffd77800) -> ("#006cc1", "#65bbff")
        let (light, dark) = accent_from_color(0xffd7_7800);
        assert_eq!(light, 0x006cc1);
        assert_eq!(dark, 0x65bbff);
    }

    /// System follows Windows' dark setting, an explicit choice wins over it, high contrast wins over all.
    #[test]
    fn choices() {
        let mut w = Windows::default();
        assert_eq!(resolve("system", &w), ("light", 0x005fb8));
        w.dark = true;
        assert_eq!(resolve("system", &w), ("dark", 0x60cdff));
        assert_eq!(resolve("light", &w).0, "light");
        w.high_contrast = true;
        assert_eq!(resolve("dark", &w).0, "classic");
    }
}
