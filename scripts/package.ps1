param([string]$BinaryPath = '')
$ErrorActionPreference = 'Stop'
if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT) {
    throw 'Windows EXE/ZIP packaging must run on Windows using a natively built MSVC executable.'
}
$projectRoot = Split-Path $PSScriptRoot -Parent
if (-not $BinaryPath) { $BinaryPath = Join-Path $projectRoot 'target\x86_64-pc-windows-msvc\release\firmware-manage-tool.exe' }
if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) { throw 'Build the Windows MSVC release executable first.' }
Push-Location $projectRoot
try {
    $metadataText = & cargo metadata --locked --format-version 1
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed.' }
    $metadata = ($metadataText -join "`n") | ConvertFrom-Json
    $project = $metadata.packages | Where-Object { $_.name -eq 'firmware-manage-tool' }
    $packageName = "FirmwareManageTool_Rust-$($project.version)-windows-x64"
    $dist = Join-Path $projectRoot 'dist'
    $stage = Join-Path $dist $packageName
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath $BinaryPath -Destination (Join-Path $stage 'FirmwareManageTool.exe')
    foreach ($name in @('firmware', 'shared', 'docs', 'README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md')) {
        Copy-Item -LiteralPath (Join-Path $projectRoot $name) -Destination $stage -Recurse
    }
    $licenses = Join-Path $stage 'ThirdPartyLicenses'
    New-Item -ItemType Directory -Force -Path $licenses | Out-Null
    foreach ($package in $metadata.packages) {
        if ($package.name -eq 'firmware-manage-tool') { continue }
        $source = Split-Path $package.manifest_path -Parent
        $files = Get-ChildItem -LiteralPath $source | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([-._].*)?$' }
        if ($files) {
            $destination = Join-Path $licenses "$($package.name)-$($package.version)"
            New-Item -ItemType Directory -Force -Path $destination | Out-Null
            foreach ($file in $files) { Copy-Item -LiteralPath $file.FullName -Destination $destination -Recurse }
        }
    }
    $hidapi = $metadata.packages | Where-Object { $_.name -eq 'hidapi' }
    if ($hidapi) {
        $cLicenses = Join-Path (Split-Path $hidapi.manifest_path -Parent) 'etc/hidapi'
        $destination = Join-Path $licenses 'hidapi-c'
        New-Item -ItemType Directory -Force -Path $destination | Out-Null
        Get-ChildItem -LiteralPath $cLicenses -Filter 'LICENSE*' -File | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $destination }
    }
    $metadata.packages | Select-Object name, version, license, repository, source | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $licenses 'dependencies.json') -Encoding UTF8
    $archive = Join-Path $dist "$packageName.zip"
    if (Test-Path -LiteralPath $archive) { Remove-Item -LiteralPath $archive -Force }
    Get-ChildItem -LiteralPath $stage -Recurse -File | Where-Object { $_.LastWriteTimeUtc.Year -lt 1980 } | ForEach-Object { $_.LastWriteTimeUtc = [datetime]'1980-01-01T00:00:00Z' }
    Compress-Archive -LiteralPath $stage -DestinationPath $archive -CompressionLevel Optimal
    Get-FileHash -Algorithm SHA256 -LiteralPath $archive | Format-List
    Write-Host "Portable package: $archive"
} finally { Pop-Location }
