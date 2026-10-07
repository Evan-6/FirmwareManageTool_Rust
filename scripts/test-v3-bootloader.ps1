$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path $PSScriptRoot -Parent
if (-not $IsWindows) { function chcp { param($CodePage) } }

foreach ($relative in @('firmware/tools/vendor_hid_bootloader.ps1', 'firmware/boards/leonardo_avr/upload.ps1', 'firmware/boards/rp2040/upload.ps1')) {
    $tokens = $null
    $parseErrors = $null
    [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $projectRoot $relative), [ref]$tokens, [ref]$parseErrors) | Out-Null
    if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
}
. (Join-Path $projectRoot 'firmware/tools/vendor_hid_bootloader.ps1')
Add-VendorHidUploadType
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Threading;
using System.Threading.Tasks;
public sealed class BootloaderTestStream : MemoryStream
{
    public readonly Queue<byte[]> Replies = new Queue<byte[]>();
    public readonly List<byte[]> Writes = new List<byte[]>();
    public override Task WriteAsync(byte[] buffer, int offset, int count, CancellationToken token)
    {
        byte[] copy = new byte[count]; Array.Copy(buffer, offset, copy, 0, count);
        Writes.Add(copy); return Task.CompletedTask;
    }
    public override Task<int> ReadAsync(byte[] buffer, int offset, int count, CancellationToken token)
    {
        if (Replies.Count == 0) return new TaskCompletionSource<int>().Task;
        byte[] reply = Replies.Dequeue(); Array.Copy(reply, 0, buffer, offset, reply.Length);
        return Task.FromResult(reply.Length);
    }
}
'@
$control = [VendorHidUpload].GetMethod('Control', [Reflection.BindingFlags]'Static,NonPublic')
function New-Reply {
    param([byte]$Op, [uint32]$Session = 42, [uint32]$Sequence = 1, [byte]$Code = 0)
    $r = [byte[]]::new(64)
    $r[0] = 13; $r[1] = 3; $r[2] = $Op; $r[3] = 1; $r[13] = $Code
    [BitConverter]::GetBytes($Session).CopyTo($r, 5)
    [BitConverter]::GetBytes($Sequence).CopyTo($r, 9)
    return ,$r
}
function Invoke-ControlTest {
    param($Stream, [byte]$Op = 1, [uint32]$Sequence = 1)
    $control.Invoke($null, @($Stream, $Op, [uint32]42, $Sequence, [Diagnostics.Stopwatch]::StartNew(), [int]30)) | Out-Null
}
function Expect-ControlFailure {
    param($Stream, [byte]$Op = 1, [uint32]$Sequence = 1)
    $failed = $false
    try { Invoke-ControlTest $Stream $Op $Sequence } catch { $failed = $true }
    if (-not $failed) { throw 'Invalid bootloader response was accepted' }
}
$stream = [BootloaderTestStream]::new()
$stream.Replies.Enqueue((New-Reply -Op 127 -Session 43 -Code 5))
$stream.Replies.Enqueue((New-Reply -Op 129))
Invoke-ControlTest $stream
$expected = [byte[]]::new(64)
$expected[0] = 12; $expected[1] = 3; $expected[2] = 1; $expected[5] = 42; $expected[9] = 1
if ([Convert]::ToHexString($stream.Writes[0]) -ne [Convert]::ToHexString($expected)) { throw 'OPEN wire mismatch' }
$stream.Replies.Enqueue((New-Reply -Op 134 -Sequence 2))
Invoke-ControlTest $stream -Op 6 -Sequence 2
if ($stream.Writes[1][2] -ne 6 -or $stream.Writes[1][9] -ne 2) { throw 'BOOTLOADER wire mismatch' }
$stream.Dispose()

foreach ($case in @('busy', 'own-fault', 'padding', 'short', 'wrong-sequence', 'timeout')) {
    $stream = [BootloaderTestStream]::new()
    $reply = New-Reply -Op 129
    switch ($case) {
        'busy' { $reply[13] = 2 }
        'own-fault' { $reply[2] = 127; $reply[13] = 5 }
        'padding' { $reply[63] = 1; $reply[5] = 43 }
        'short' { $reply = [byte[]]$reply[0..62] }
        'wrong-sequence' { $reply[9] = 2 }
    }
    if ($case -ne 'timeout') { $stream.Replies.Enqueue($reply) }
    Expect-ControlFailure $stream
    $stream.Dispose()
}
Write-Host 'PowerShell v3 bootloader: parser/C# compilation, OPEN/BOOT wire, foreign sessions, faults, padding, short replies, sequence and timeout passed'
