//! Following Windows' look: light or dark mode, the accent colour, "Make text bigger" and high contrast,
//! the way the Windows 11 Settings app does (`led_mon/theme.py`). Read again every two seconds, so a
//! change in Windows' settings shows straight away.
//!
//! The choice (system, light, dark or classic) is saved as "app_theme" in settings.json. Classic is the
//! plain Windows look, and is used whenever high contrast is on, so its colours are followed exactly.
//!
//! Away from Windows (screenshots, tests): WHITE_RABBIT_APPEARANCE=light|dark,
//! WHITE_RABBIT_ACCENT=#light,#dark, WHITE_RABBIT_TEXT_SCALE=1.25 and WHITE_RABBIT_HIGH_CONTRAST=1.

use slint::Color;

pub const SETTING: &str = "app_theme";
pub const CHOICES: [&str; 4] = ["system", "light", "dark", "classic"];
/// Windows' default blue accent: (light mode, dark mode)
pub const DEFAULT_ACCENT: (u32, u32) = (0x005fb8, 0x60cdff);

#[derive(Clone, Debug, PartialEq)]
pub struct Windows {
    pub dark: bool,
    /// (light mode, dark mode) shades, 0xRRGGBB
    pub accent: (u32, u32),
    pub high_contrast: bool,
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

pub fn rgb(c: u32) -> Color {
    Color::from_rgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

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
    let color = |i: usize| (data[i * 4] as u32) << 16 | (data[i * 4 + 1] as u32) << 8 | data[i * 4 + 2] as u32;
    Some((color(4), color(1)))
}

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
    (((h / 6.0) % 1.0 + 1.0) % 1.0, l, s)
}

#[cfg_attr(not(windows), allow(dead_code))]
fn hls_to_rgb(h: f64, l: f64, s: f64) -> (f64, f64, f64) {
    if s == 0.0 {
        return (l, l, l);
    }
    let m2 = if l <= 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let m1 = 2.0 * l - m2;
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

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Graphics::Gdi::GetSysColor;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RRF_RT_REG_DWORD};
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

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

    pub fn high_contrast() -> bool {
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
        (c & 0xff) << 16 | (c & 0xff00) | (c >> 16) & 0xff
    }
}

/// Windows' look settings now.
pub fn read_windows() -> Windows {
    let mut found = Windows::default();
    #[cfg(windows)]
    {
        found.dark = win::dword(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme")
            .is_some_and(|v| v == 0);
        found.accent = win::binary(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentPalette")
            .and_then(|d| accent_from_palette(&d))
            .or_else(|| win::dword(r"Software\Microsoft\Windows\DWM", "AccentColor").map(accent_from_color))
            .unwrap_or(DEFAULT_ACCENT);
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

pub fn load_choice() -> String {
    crate::settings::get_str(SETTING).filter(|c| CHOICES.contains(&c.as_str())).unwrap_or_else(|| "system".into())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_palette() {
        let mut data = vec![0u8; 32];
        data[4..7].copy_from_slice(&[0x99, 0xeb, 0xff]);
        data[16..19].copy_from_slice(&[0x00, 0x5a, 0x9e]);
        assert_eq!(accent_from_palette(&data), Some((0x005a9e, 0x99ebff)));
        assert_eq!(accent_from_palette(&[1, 2]), None);
    }

    #[test]
    fn accent_color_matches_python() {
        // Python: accent_from_color(0xffd77800) -> ("#006cc1", "#65bbff")
        let (light, dark) = accent_from_color(0xffd7_7800);
        assert_eq!(light, 0x006cc1);
        assert_eq!(dark, 0x65bbff);
    }

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
