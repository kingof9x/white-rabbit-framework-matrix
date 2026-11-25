//! Build script for WhiteRabbitWindow.exe (the settings window). Cargo runs it before compiling.
//!
//! It does two things:
//! 1. Compiles the window's Slint markup (`ui/app.slint` and the files it imports) into Rust code.
//!    The generated code defines `AppWindow` and its properties and callbacks, which `src/main.rs`
//!    pulls in with `slint::include_modules!()`. A mistake in a .slint file fails the build here.
//! 2. Adds the icon and file details to the Windows exe (shared code in `rust/windows_resources.rs`).
include!("../windows_resources.rs");

/// Called by Cargo at build time.
fn main() {
    // The Windows 11 look; colours and our own controls follow the Windows theme (see ui/theme.slint)
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("the window's .slint files have an error");
    windows_resources("WhiteRabbitWindow");
}
