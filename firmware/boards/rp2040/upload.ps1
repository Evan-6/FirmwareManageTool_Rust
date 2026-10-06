param(
    [string]$Port = "",          # Optional: RPI-RP2 drive root (e.g. E:\). Skips auto enter_bootloader.
    [string]$SketchDir = $PSScriptRoot,
    [switch]$SkipCoreInstall,
    [switch]$SkipBootloader      # Don't send enter_bootloader; expect the board already in BOOTSEL.
)

$ErrorActionPreference = "Stop"

try { chcp 65001 | Out-Null } catch {}
[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)
if ($PSVersionTable.PSVersion.Major -ge 7) { $PSStyle.OutputRendering = "PlainText" }

# arduino-pico (Earle Philhower) core. USB stack must be Adafruit TinyUSB. The
# firmware exposes HID keyboard + Vendor HID only (no CDC / no COM port), so
# flashing goes through the RP2040 ROM UF2 bootloader (RPI-RP2 mass-storage
# drive), reached automatically by sending "enter_bootloader" over Vendor HID.
$Fqbn = "rp2040:rp2040:rpipico:usbstack=tinyusb"
$SketchName = "rp2040.ino"
$CoreId = "rp2040:rp2040@6.2.0"
$CoreIndexUrl = "https://github.com/earlephilhower/arduino-pico/releases/download/global/package_rp2040_index.json"
$BuildPath = Join-Path $env:TEMP "mscv-rp2040-keyboard-build"
$ConfigPath = Join-Path $env:TEMP "mscv-rp2040-keyboard-arduino-cli.yaml"

# Vendor HID identity (matches Firmware.cpp / the AVR build).
$VendorHidVid = [UInt16]0x03F0
$VendorHidPid = [UInt16]0x0024
$VendorHidUsagePage = [UInt16]0xFF60
$VendorHidUsage = [UInt16]0x61
$VendorHidCommandTimeoutMs = 1500
$BootloaderWaitSeconds = 15

# Shared Vendor HID helper (Invoke-VendorHidCommand), same one the AVR build uses.
. (Join-Path $PSScriptRoot "..\..\tools\vendor_hid_bootloader.ps1")

function Write-Step { param([string]$Message) Write-Host ""; Write-Host "==> $Message" -ForegroundColor Cyan }

# --- 新增：自動安裝所需的輔助函數 ---
function Refresh-Path {
    $machinePath = [Environment]::GetEnvironmentVariable("Path", "Machine")
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $env:Path = "$machinePath;$userPath"
}

function Test-CommandExists {
    param([string]$Name)
    return [bool](Get-Command $Name -ErrorAction SilentlyContinue)
}

function Install-ArduinoCliIfMissing {
    if (Test-CommandExists "arduino-cli") {
        return # 已經安裝就略過
    }

    Write-Step "arduino-cli not found, installing via winget..."

    if (-not (Test-CommandExists "winget")) {
        throw @"
arduino-cli not found, and winget is not available.

Please install App Installer from Microsoft Store,
or manually download Arduino CLI from:
https://github.com/arduino/arduino-cli/releases
"@
    }

    & winget install `
        --id ArduinoSA.CLI `
        --exact `
        --accept-source-agreements `
        --accept-package-agreements

    if ($LASTEXITCODE -ne 0) {
        throw "winget failed to install Arduino CLI. ExitCode=$LASTEXITCODE"
    }

    Refresh-Path

    if (-not (Test-CommandExists "arduino-cli")) {
        throw @"
Arduino CLI was installed, but this PowerShell session still cannot find arduino-cli.

Close this PowerShell window, reopen it, then run the script again.
"@
    }

    Write-Host "Arduino CLI installed." -ForegroundColor Green
}
# -----------------------------------

function Invoke-ArduinoCli {
    param([string[]]$Arguments)
    Write-Host "arduino-cli $($Arguments -join ' ')" -ForegroundColor DarkGray
    & arduino-cli --config-file $ConfigPath @Arguments
    if ($LASTEXITCODE -ne 0) { throw "arduino-cli failed. ExitCode=$LASTEXITCODE" }
}

# An RP2040 in BOOTSEL mounts a removable drive containing INFO_UF2.TXT.
function Find-Rp2Drive {
    foreach ($drive in [System.IO.DriveInfo]::GetDrives()) {
        if (-not $drive.IsReady) { continue }
        try {
            $root = $drive.RootDirectory.FullName
            if (Test-Path (Join-Path $root "INFO_UF2.TXT")) { return $root }
        } catch {}
    }
    return $null
}

function Wait-Rp2Drive {
    param([int]$TimeoutSeconds = 15)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $found = Find-Rp2Drive
        if ($found) { return $found }
        Start-Sleep -Milliseconds 300
    }
    return $null
}

# Ask the running firmware to reboot into the bootloader over Vendor HID.
# Returns $true if the command was delivered (device was running our firmware).
function Send-EnterBootloader {
    try {
        $response = Invoke-VendorHidCommand `
            -Vid $VendorHidVid -ProductId $VendorHidPid `
            -UsagePage $VendorHidUsagePage -Usage $VendorHidUsage `
            -Command "enter_bootloader" -TimeoutMs $VendorHidCommandTimeoutMs
        Write-Host "Vendor HID response: $response"
        return $true
    }
    catch {
        # "device not found" => not running our firmware (maybe already in BOOTSEL).
        # A post-send exception => device likely already rebooting; treat as delivered.
        Write-Host "Vendor HID enter_bootloader: $($_.Exception.Message)" -ForegroundColor DarkYellow
        return $false
    }
}

try {
    # 取代原本遇到錯誤就退出的邏輯，改呼叫自動安裝函數
    Install-ArduinoCliIfMissing

    Write-Step "Preparing arduino-cli config"
    if (-not (Test-Path $ConfigPath)) {
        & arduino-cli --config-file $ConfigPath config init | Out-Null
    }
    & arduino-cli --config-file $ConfigPath config set board_manager.additional_urls $CoreIndexUrl | Out-Null

    if (-not $SkipCoreInstall) {
        Write-Step "Installing/updating $CoreId core (bundles Adafruit TinyUSB)"
        Invoke-ArduinoCli @("core", "update-index")
        Invoke-ArduinoCli @("core", "install", $CoreId)
    }

    $sketchPath = Join-Path $SketchDir $SketchName
    if (-not (Test-Path $sketchPath)) { throw "Cannot find $SketchName in $SketchDir." }

    Write-Step "Compiling"
    Invoke-ArduinoCli @("compile", "--fqbn", $Fqbn, "--clean", "--build-path", $BuildPath, $SketchDir)

    $uf2 = Get-ChildItem -Path $BuildPath -Filter *.uf2 -File | Select-Object -First 1
    if (-not $uf2) { throw "Compile produced no .uf2 in $BuildPath." }
    Write-Host "Firmware image: $($uf2.FullName)"

    # Resolve the target RPI-RP2 drive.
    $targetDrive = ""
    if (-not [string]::IsNullOrWhiteSpace($Port)) {
        $targetDrive = $Port
        Write-Step "Using specified drive: $targetDrive"
    }
    else {
        $targetDrive = Find-Rp2Drive
        if ($targetDrive) {
            Write-Step "Board already in BOOTSEL: $targetDrive"
        }
        elseif ($SkipBootloader) {
            throw "No RPI-RP2 drive found. Hold BOOTSEL while plugging in, then re-run."
        }
        else {
            Write-Step "Requesting bootloader via Vendor HID (enter_bootloader)"
            [void](Send-EnterBootloader)

            Write-Step "Waiting for RPI-RP2 drive (up to ${BootloaderWaitSeconds}s)"
            $targetDrive = Wait-Rp2Drive -TimeoutSeconds $BootloaderWaitSeconds
            if (-not $targetDrive) {
                throw "RPI-RP2 drive did not appear. If the firmware is not running yet, hold BOOTSEL while plugging in, then re-run (or pass -Port <drive>)."
            }
            Write-Host "Bootloader drive: $targetDrive"
        }
    }

    Write-Step "Flashing (copying .uf2 to $targetDrive)"
    try {
        Copy-Item -LiteralPath $uf2.FullName -Destination $targetDrive -Force
    }
    catch {
        # The drive disappears the moment the RP2040 finishes flashing and reboots;
        # a copy error at that point usually still means a successful flash.
        Write-Host "Copy reported: $($_.Exception.Message) (board likely rebooted after flashing)" -ForegroundColor DarkYellow
    }

    Write-Host ""
    Write-Host "Done: flashed $SketchName to RP2040 via $targetDrive" -ForegroundColor Green
}
catch {
    Write-Host ""
    Write-Host "Upload failed: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}