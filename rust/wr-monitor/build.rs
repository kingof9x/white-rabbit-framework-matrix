//! Build script for WhiteRabbit.exe (the tray monitor). Cargo runs it before compiling the crate.
//!
//! Its only job is to give the Windows exe its icon and file details (version, product name,
//! copyright), which Windows shows in the file's Properties, in Task Manager and in Settings' list of
//! tray icons. The shared code is in `rust/windows_resources.rs`, pulled in below with `include!` so
//! that the monitor and the settings window are labelled the same way. On Linux (tests and CI) the
//! function does nothing.
include!("../windows_resources.rs");

/// Called by Cargo at build time: adds the icon and version details to `WhiteRabbit.exe`.
fn main() {
    windows_resources("WhiteRabbit");
}
