fn main() {
    println!("cargo:rerun-if-changed=resources/app.manifest");
    if std::env::var("TARGET").unwrap_or_default() == "x86_64-pc-windows-msvc" {
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
            .join("resources/app.manifest");
        println!("cargo:rustc-link-arg-bin=firmware-manage-tool=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin=firmware-manage-tool=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
