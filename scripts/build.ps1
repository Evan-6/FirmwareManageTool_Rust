param([switch]$SkipChecks)
$ErrorActionPreference = 'Stop'
if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw 'Windows EXE/ZIP builds must run on Windows with the native MSVC toolchain.'
}
$projectRoot = Split-Path $PSScriptRoot -Parent
Push-Location $projectRoot
try {
    & rustup target add x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Installing the Rust MSVC target failed.' }
    if (-not $SkipChecks) {
        & cargo fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
        & cargo clippy --locked --workspace --target x86_64-pc-windows-msvc --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
        & cargo test --locked --workspace --target x86_64-pc-windows-msvc --all-targets
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
    }
    & cargo build --locked --release --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    & (Join-Path $PSScriptRoot 'package.ps1')
} finally { Pop-Location }
