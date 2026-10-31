# Reclaims the build trees that are pure duplication, and reports what is left.
#
# Two things bloat this repo:
#   1. Dev artifacts. target/debug holds a full dependency graph plus incremental
#      caches and grows to several GB between prunes.
#   2. Naming the host triple. On Windows `cargo build --target x86_64-pc-windows-msvc`
#      keeps host-only units in target/<profile> and target units in
#      target/x86_64-pc-windows-msvc/<profile>, i.e. it builds everything twice.
#      The scripts no longer pass --target; this also cleans up trees left behind
#      by older runs.
#
# Never touches target/release (the executable we ship) or
# firmware/app/target/thumbv6m-none-eabi (the firmware we flash).
param([switch]$Apply)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Get-SizeBytes([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return 0 }
    $sum = (Get-ChildItem -LiteralPath $Path -Recurse -File -Force -ErrorAction SilentlyContinue |
        Measure-Object -Property Length -Sum).Sum
    if ($null -eq $sum) { return 0 }
    return [int64]$sum
}

function Format-Size([int64]$Bytes) {
    if ($Bytes -ge 1GB) { return ('{0:N1} GB' -f ($Bytes / 1GB)) }
    if ($Bytes -ge 1MB) { return ('{0:N0} MB' -f ($Bytes / 1MB)) }
    return ('{0:N0} KB' -f ($Bytes / 1KB))
}

$disposable = @(
    'target/debug'
    'target/tmp'
    'target/x86_64-pc-windows-msvc'
    'firmware/app/target/debug'
    'firmware/app/target/release'
    'firmware/app/target/tmp'
)
$keep = @(
    'target/release'
    'target/thumbv6m-none-eabi'
    'firmware/app/target/thumbv6m-none-eabi'
)

$rows = foreach ($relative in $disposable) {
    $full = Join-Path $root $relative
    [pscustomobject]@{ Path = $relative; Size = (Get-SizeBytes $full); Exists = (Test-Path -LiteralPath $full) }
}

$before = Get-SizeBytes $root
$total = ($rows | Measure-Object -Property Size -Sum).Sum
if ($null -eq $total) { $total = 0 }

Write-Host ('{0,-34} {1,10}' -f 'PATH', 'SIZE')
foreach ($row in $rows) {
    $mark = if ($row.Exists) { '' } else { '  (absent)' }
    Write-Host ('{0,-34} {1,10}{2}' -f $row.Path, (Format-Size $row.Size), $mark)
}
Write-Host ''
Write-Host ('Disposable: {0}   Repository total: {1}' -f (Format-Size $total), (Format-Size $before))

if (-not $Apply) {
    Write-Host ''
    Write-Host 'Dry run. Re-run with -Apply to delete the rows above.'
    return
}

$freed = [int64]0
foreach ($row in $rows) {
    if (-not $row.Exists) { continue }
    $full = Join-Path $root $row.Path
    Remove-Item -LiteralPath $full -Recurse -Force -ErrorAction Stop
    $freed += [int64]$row.Size
    Write-Host ('removed {0} ({1})' -f $row.Path, (Format-Size $row.Size))
}

$after = Get-SizeBytes $root
Write-Host ''
Write-Host ('Freed {0}. Repository now {1} (was {2}).' -f (Format-Size $freed), (Format-Size $after), (Format-Size $before))
Write-Host ('Kept: {0}' -f ($keep -join ', '))
