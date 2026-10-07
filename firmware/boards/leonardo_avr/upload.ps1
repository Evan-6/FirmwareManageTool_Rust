param(
    [string]$Port = "",
    [string]$SketchDir = $PSScriptRoot,
    [switch]$SkipCoreInstall
)

$ErrorActionPreference = "Stop"

# UTF-8 console setup
try {
    chcp 65001 | Out-Null
} catch {
}

[Console]::InputEncoding  = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$OutputEncoding = [System.Text.UTF8Encoding]::new($false)

if ($PSVersionTable.PSVersion.Major -ge 7) {
    $PSStyle.OutputRendering = "PlainText"
}

$Fqbn = "goosedevil:avr:keyboard"
$SketchName = "leonardo_avr.ino"
$UsbProductName = "HP Keyboard"
$UsbManufacturerName = "HP"
$UsbVid = "0x03F0"
$UsbPid = "0x0024"
$UsbSerial = "HP-KB-0024"
$BuildPath = Join-Path $env:TEMP "mscv-arduino-keyboard-build"
$ArduinoCliUserDir = [System.IO.Path]::GetFullPath($PSScriptRoot)
$ArduinoCliConfigPath = Join-Path $env:TEMP "mscv-hp-keyboard-arduino-cli.yaml"
$VendorHidHelperPath = Join-Path $PSScriptRoot "..\..\tools\vendor_hid_bootloader.ps1"
$VendorHidVid = 0x03F0
$LegacyVendorHidVid = 0x6666
$VendorHidPid = 0x0024
$LegacyVendorHidPid = 0x6666
$VendorHidUsagePage = 0xFF60
$VendorHidUsage = 0x61
$VendorHidCommandTimeoutMs = 1500
$BootloaderWaitSeconds = 10
$BootloaderSettleMilliseconds = 1200

. $VendorHidHelperPath

function Write-Step {
    param([string]$Message)

    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

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
        Write-Host "arduino-cli found."
        return
    }

    Write-Step "arduino-cli not found, installing"

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

Close this PowerShell window, reopen it, then run:

pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\upload.ps1
"@
    }

    Write-Host "Arduino CLI installed." -ForegroundColor Green
}

function Write-ArduinoCliConfig {
    $arduinoDataDir = Join-Path $env:LOCALAPPDATA "Arduino15"
    $arduinoDownloadsDir = Join-Path $arduinoDataDir "staging"

    @"
directories:
  data: $arduinoDataDir
  downloads: $arduinoDownloadsDir
  user: $ArduinoCliUserDir
"@ | Set-Content -LiteralPath $script:ArduinoCliConfigPath -Encoding ASCII
}
function Invoke-ArduinoCli {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    Write-Host "arduino-cli $($Arguments -join ' ')" -ForegroundColor DarkGray

    & arduino-cli --config-file $script:ArduinoCliConfigPath @Arguments

    if ($LASTEXITCODE -ne 0) {
        throw "arduino-cli failed. ExitCode=$LASTEXITCODE"
    }
}

function Initialize-ArduinoCliConfig {
    Write-Step "Checking Arduino CLI config"

    $dumpOutput = & arduino-cli config dump 2>&1
    $dumpExitCode = $LASTEXITCODE

    if ($dumpExitCode -eq 0) {
        Write-Host "Arduino CLI config is OK."
        return
    }

    Write-Host "Arduino CLI config not found, creating config..." -ForegroundColor Yellow

    $initOutput = & arduino-cli config init 2>&1
    $initExitCode = $LASTEXITCODE

    $initOutput | ForEach-Object {
        Write-Host $_
    }

    if ($initExitCode -eq 0) {
        Write-Host "Arduino CLI config created."
        return
    }

    $joinedOutput = $initOutput -join "`n"

    if ($joinedOutput -match "already exists|�w�s�b|�]�w�ɤw�s�b") {
        Write-Host "Arduino CLI config already exists, skipping init." -ForegroundColor Yellow
        return
    }

    throw "arduino-cli config init failed. ExitCode=$initExitCode"
}

function Test-SketchLayout {
    Write-Step "Checking sketch"

    $resolvedSketchDir = Resolve-Path $SketchDir
    $script:SketchDir = $resolvedSketchDir.Path

    $sketchPath = Join-Path $script:SketchDir $SketchName

    if (-not (Test-Path $sketchPath)) {
        throw @"
Cannot find $SketchName.

Expected layout:

leonardo_avr/
  leonardo_avr.ino
  upload.ps1

Current SketchDir:
$($script:SketchDir)
"@
    }

    $folderName = Split-Path $script:SketchDir -Leaf
    $inoBaseName = [System.IO.Path]::GetFileNameWithoutExtension($SketchName)

    if ($folderName -ne $inoBaseName) {
        Write-Host "Warning: Arduino sketch folder name should match ino filename." -ForegroundColor Yellow
        Write-Host "Folder: $folderName" -ForegroundColor Yellow
        Write-Host "INO:    $inoBaseName" -ForegroundColor Yellow
    }

    Write-Host "SketchDir: $($script:SketchDir)"
    Write-Host "Sketch:    $sketchPath"
    Write-Host "FQBN:      $Fqbn"
    Write-Host "Board dir: $ArduinoCliUserDir\hardware"
}

function Install-ArduinoAvrCoreIfMissing {
    if ($SkipCoreInstall) {
        Write-Step "Skipping Arduino AVR core install"
        return
    }

    Write-Step "Installing Arduino AVR core"
    Invoke-ArduinoCli @("core", "update-index")
    Invoke-ArduinoCli @("core", "install", "arduino:avr@1.8.8")
}

function Find-ComPortInText {
    param([string]$Text)

    if ($Text -match "COM\d+") {
        return $Matches[0]
    }

    return $null
}
function Find-LeonardoBootloaderPort {
    $raw = & arduino-cli --config-file $script:ArduinoCliConfigPath board list 2>&1
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    foreach ($line in ($raw | ForEach-Object { $_.ToString() })) {
        if ($line -match "arduino:avr:leonardo") {
            $port = Find-ComPortInText $line
            if ($port) {
                return $port
            }
        }
    }

    return $null
}

function Wait-LeonardoBootloaderPort {
    param([int]$TimeoutSeconds = 10)

    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    do {
        $port = Find-LeonardoBootloaderPort
        if ($port) {
            return $port
        }

        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)

    return $null
}

function Invoke-VendorHidBootloader {
    Write-Step "Trying Vendor HID bootloader trigger"

    $identities = @(
        @{ Name = "HP Keyboard"; Vid = $VendorHidVid; Pid = $VendorHidPid },
        @{ Name = "Legacy GooseDevil Keyboard"; Vid = $LegacyVendorHidVid; Pid = $LegacyVendorHidPid }
    )

    foreach ($identity in $identities) {
        try {
            $identityName = [string]$identity.Name
            $identityVid = [UInt16]$identity.Vid
            $identityPid = [UInt16]$identity.Pid
            Write-Host (("Trying {0} VID/PID: 0x{1:X4} / 0x{2:X4}") -f $identityName, $identityVid, $identityPid)

            $response = Invoke-VendorHidCommand `
                -Vid $identityVid `
                -ProductId $identityPid `
                -UsagePage ([UInt16]$VendorHidUsagePage) `
                -Usage ([UInt16]$VendorHidUsage) `
                -Command "enter_bootloader" `
                -TimeoutMs $VendorHidCommandTimeoutMs

            Write-Host "Vendor HID response: $response"

            $bootloaderPort = Wait-LeonardoBootloaderPort -TimeoutSeconds $BootloaderWaitSeconds
            if ($bootloaderPort) {
                Write-Host "Bootloader port: $bootloaderPort"
                Start-Sleep -Milliseconds $BootloaderSettleMilliseconds
                return $bootloaderPort
            }

            Write-Host "Vendor HID trigger succeeded, but bootloader COM port did not appear." -ForegroundColor Yellow
        }
        catch {
            Write-Host "Vendor HID bootloader trigger failed for $($identity.Name): $($_.Exception.Message)" -ForegroundColor Yellow
        }
    }

    return $null
}

function Find-LeonardoPort {
    Write-Step "Finding serial upload port"

    $raw = & arduino-cli --config-file $script:ArduinoCliConfigPath board list 2>&1
    $exitCode = $LASTEXITCODE

    $lines = $raw | ForEach-Object { $_.ToString() }

    if ($exitCode -ne 0) {
        throw "Failed to list Arduino boards:`n$($lines -join "`n")"
    }

    Write-Host "Detected boards:"
    $lines | ForEach-Object {
        Write-Host $_
    }

    foreach ($line in $lines) {
        if ($line -match "arduino:avr:leonardo") {
            $port = Find-ComPortInText $line
            if ($port) {
                return $port
            }
        }
    }

    foreach ($line in $lines) {
        if ($line -match "HP Keyboard|GooseDevil|Leonardo") {
            $port = Find-ComPortInText $line
            if ($port) {
                return $port
            }
        }
    }

    Write-Step "Checking Windows PnP serial devices"
    $pnpMatches = @(Get-CimInstance -ClassName Win32_PnPEntity | Where-Object {
        $_.Name -match "HP Keyboard|GooseDevil|Arduino Leonardo|Leonardo" -or
        $_.DeviceID -match "VID_03F0&PID_0024|VID_6666&PID_6666|VID_2341&PID_8036|VID_2A03&PID_8036"
    })

    foreach ($device in $pnpMatches) {
        $port = Find-ComPortInText $device.Name
        if ($port) {
            Write-Host "Detected PnP keyboard port: $($device.Name)"
            return $port
        }
    }

    throw @"
Cannot find bootloader or serial upload port.

Please check:
1. The keyboard is connected by USB.
2. Device Manager can see the COM port.
3. USB cable is not charge-only.
4. Try:

   arduino-cli board list

Or specify port manually:

   pwsh -NoLogo -NoProfile -ExecutionPolicy Bypass -File .\upload.ps1 -Port COM3
"@
}

try {
    Test-SketchLayout

    Install-ArduinoCliIfMissing
    Write-ArduinoCliConfig

    Write-Step "Arduino CLI version"
    Invoke-ArduinoCli @("version")

    Initialize-ArduinoCliConfig

    Install-ArduinoAvrCoreIfMissing

    $manualPortSpecified = -not [string]::IsNullOrWhiteSpace($Port)
    $usingHidBootloaderPort = $false

    Write-Step "USB identity"
    Write-Host "USB product: $UsbProductName"
    Write-Host "USB manufacturer: $UsbManufacturerName"
    Write-Host "Runtime VID/PID: $UsbVid / $UsbPid"
    Write-Host "USB serial: $UsbSerial"

    Write-Step "Compiling"
    Invoke-ArduinoCli @(
        "compile",
        "--fqbn", $Fqbn,
        "--clean",
        "--build-path", $BuildPath,
        $script:SketchDir
    )

    if ($manualPortSpecified) {
        Write-Step "Using specified upload port: $Port"
    }
    else {
        $hidBootloaderPort = Invoke-VendorHidBootloader
        if ($hidBootloaderPort) {
            $Port = $hidBootloaderPort
            $usingHidBootloaderPort = $true
        }
        else {
            Write-Step "Falling back to serial upload port"
            $Port = Find-LeonardoPort
        }
    }

    Write-Step "Using upload port: $Port"

    Write-Step "Uploading"
    $uploadArgs = @(
        "upload",
        "-p", $Port,
        "--fqbn", $Fqbn,
        "--build-path", $BuildPath,
        "--verify"
    )

    if ($usingHidBootloaderPort) {
        $uploadArgs += @(
            "--upload-property", "upload.use_1200bps_touch=false",
            "--upload-property", "upload.wait_for_upload_port=false"
        )
    }

    $uploadArgs += $script:SketchDir
    Invoke-ArduinoCli $uploadArgs

    Write-Host ""
    Write-Host "Done: uploaded $SketchName to Arduino Leonardo ($Port)" -ForegroundColor Green
}
catch {
    Write-Host ""
    Write-Host "FAILED:" -ForegroundColor Red
    Write-Host $_.Exception.Message -ForegroundColor Red
    exit 1
}