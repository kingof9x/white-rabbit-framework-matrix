// The exe's icon and file details (shown in its Properties, Task Manager and Settings' list of tray
// icons). An exe without them looks more suspicious to antivirus scanners. Shared by the build.rs of
// wr-monitor and wr-window with include!; it only does something when building on Windows for Windows.

/// The version: WHITE_RABBIT_VERSION (set by the release build), else the one in pyproject.toml.
fn app_version() -> String {
    println!("cargo:rerun-if-env-changed=WHITE_RABBIT_VERSION");
    if let Ok(v) = std::env::var("WHITE_RABBIT_VERSION") {
        if !v.trim().is_empty() {
            return v.trim().to_string();
        }
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let pyproject = manifest.join("../../pyproject.toml");
    println!("cargo:rerun-if-changed={}", pyproject.display());
    std::fs::read_to_string(pyproject)
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                let v = l.trim().strip_prefix("version")?.trim().strip_prefix('=')?.trim();
                Some(v.trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "0.0.0".into())
}

/// The numbers in the version (1.5.0-beta is 1.5.0.0), packed as Windows wants them.
fn version_numbers(version: &str) -> u64 {
    let mut parts = [0u64; 4];
    let numbers = version.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty());
    for (i, n) in numbers.take(4).enumerate() {
        parts[i] = n.parse::<u64>().unwrap_or(0).min(0xFFFF);
    }
    (parts[0] << 48) | (parts[1] << 32) | (parts[2] << 16) | parts[3]
}

fn windows_resources(exe_name: &str) {
    let version = app_version();
    let for_windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !for_windows || !cfg!(windows) {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let icon = manifest.join("../../res/WhiteRabbit.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let numbers = version_numbers(&version);
    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon.display().to_string())
        .set("CompanyName", "kingof9x")
        .set("FileDescription", "White Rabbit - Framework 16 LED Control")
        .set("FileVersion", &version)
        .set("InternalName", exe_name)
        .set("LegalCopyright", "MIT License")
        .set("OriginalFilename", &format!("{exe_name}.exe"))
        .set("ProductName", "White Rabbit - Framework 16 LED Control")
        .set("ProductVersion", &version)
        .set_version_info(winresource::VersionInfo::FILEVERSION, numbers)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, numbers);
    res.compile().expect("couldn't add the icon and file details to the exe");
}
