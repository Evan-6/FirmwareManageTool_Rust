//! Tauri build hook. Also carries the DPI manifest that used to live in the root
//! package's build script, plus the app's own commit for the About line.

fn git_commit() -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    println!("cargo:rustc-env=APP_COMMIT={}", git_commit());
    println!("cargo:rerun-if-changed=../.git/logs/HEAD");

    // Tauri already embeds its own application manifest, and a second
    // /MANIFESTINPUT collides with it (CVT1100). DPI awareness is therefore set at
    // runtime in `platform::enable_dpi_awareness`.
    tauri_build::build()
}
