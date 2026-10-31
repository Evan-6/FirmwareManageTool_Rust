//! Places `memory.x` where the RP2040 linker script (`link-rp.x`) finds it.
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

/// First four bytes of the HEAD commit, or 0 when git is unavailable.
fn git_hash() -> u32 {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok());
    let Some(text) = output else { return 0 };
    let text = text.trim();
    if text.len() < 8 {
        return 0;
    }
    u32::from_str_radix(&text[..8], 16).unwrap_or(0)
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // Build identity for Feature 14 (status byte 56..60). The host compares it with
    // its own commit so the GUI can say "this board is running an older image".
    let hash = git_hash();
    // Emitted as hex so the const parser and `git rev-parse --short=8` can be
    // compared byte for byte.
    println!("cargo:rustc-env=BUILD_HASH={hash:08x}");
    // HEAD points at the branch, so an amended commit does not touch it; the
    // reflog does change, which keeps the hash honest after a rewrite.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo:rustc-link-arg-bins=--nmagic");
    println!("cargo:rustc-link-arg-bins=-Tlink.x");
    println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");
}
