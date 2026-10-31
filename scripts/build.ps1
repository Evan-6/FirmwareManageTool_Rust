param([switch]$SkipChecks)
$ErrorActionPreference = 'Stop'
if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw 'Windows EXE/ZIP builds must run on Windows with the native MSVC toolchain.'
}
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
try {
    # Build for the host instead of passing --target: cargo splits host-only units
    # from target units, so naming the host triple explicitly builds every
    # dependency twice (target/<profile> plus target/<triple>/<profile>).
    $hostTriple = (& rustc -vV | Select-String '^host:').ToString().Split(' ')[1]
    if ($hostTriple -ne 'x86_64-pc-windows-msvc') {
        throw "The default Rust toolchain must target x86_64-pc-windows-msvc, got $hostTriple."
    }
    # The Tauri shell embeds `ui/dist` at compile time, so the frontend has to be
    # built before any cargo command that touches src-tauri.
    if (-not (Test-Path (Join-Path $projectRoot 'ui\node_modules'))) {
        & npm --prefix ui ci
        if ($LASTEXITCODE -ne 0) { throw 'npm ci failed.' }
    }
    & npm --prefix ui run build
    if ($LASTEXITCODE -ne 0) { throw 'UI build failed.' }
    if (-not $SkipChecks) {
        & cargo fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
        & cargo clippy --locked --workspace --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
        & cargo test --locked --workspace --exclude firmware-manage-tool-app --all-targets
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
        & node scripts/check-ipc.mjs
        if ($LASTEXITCODE -ne 0) { throw 'IPC contract check failed.' }
        # firmware/app is its own workspace (no_std plus a thumbv6m binary).
        & cargo test --locked --manifest-path firmware/app/Cargo.toml
        if ($LASTEXITCODE -ne 0) { throw 'Firmware tests failed.' }
    }
    & cargo build --locked --release -p firmware-manage-tool-app
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    & (Join-Path $PSScriptRoot 'package.ps1')
} finally { Pop-Location }
