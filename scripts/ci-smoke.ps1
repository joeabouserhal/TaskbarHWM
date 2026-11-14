<#
.SYNOPSIS
  Simple runtime checks for a built TaskbarHWM.exe. No hardware sensors are tested.

.DESCRIPTION
  Runs the exe (an unelevated test build: build with TASKBARHWM_ASINVOKER=1) against throw-away settings folders and checks that
  it starts and stays up, writes its config, leaves no crash log, refuses a second copy, answers --claude off, and shuts down
  cleanly when asked to close.

  Usage:  powershell -File scripts\ci-smoke.ps1 -Exe target\ci\release\TaskbarHWM.exe
#>
param([Parameter(Mandatory = $true)][string]$Exe)
$ErrorActionPreference = "Stop"
$Exe = (Resolve-Path $Exe).Path

Add-Type -TypeDefinition @"
using System; using System.Text; using System.Runtime.InteropServices;
public static class Smoke {
  delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc p, IntPtr l);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  // A window of the given class owned by the given process (hidden ones included), or zero.
  public static IntPtr Find(uint pid, string cls) {
    IntPtr found = IntPtr.Zero;
    EnumWindows((h, l) => {
      uint p; GetWindowThreadProcessId(h, out p);
      var sb = new StringBuilder(128); GetClassName(h, sb, 128);
      if (p == pid && sb.ToString() == cls) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@

$failed = @()
function Check([string]$name, [bool]$ok, [string]$detail = "") {
    if ($ok) { Write-Host "  ok    $name" } else { Write-Host "  FAIL  $name $detail" -ForegroundColor Red; $script:failed += $name }
}

# Everything the app writes goes to a scratch folder, never the real settings.
$root = Join-Path ([IO.Path]::GetTempPath()) ("hwm-smoke-" + [Guid]::NewGuid().ToString("N").Substring(0, 8))
$env:APPDATA = Join-Path $root "roaming"; $env:LOCALAPPDATA = Join-Path $root "local"
$env:HWM_DATA_DIR = Join-Path $root "data"; $env:HWM_CLAUDE_DIR = Join-Path $root "claude"
foreach ($d in $env:APPDATA, $env:LOCALAPPDATA, $env:HWM_DATA_DIR, $env:HWM_CLAUDE_DIR) { New-Item -ItemType Directory -Force $d | Out-Null }
$dll = Join-Path (Split-Path $Exe) "PawnIOLib.dll"
if (-not (Test-Path $dll)) { Copy-Item (Join-Path $PSScriptRoot "..\redist\PawnIOLib.dll") $dll }

Write-Host "Starting $Exe"
$app = Start-Process $Exe -PassThru
try {
    Start-Sleep -Seconds 6
    Check "still running after 6 s" (-not $app.HasExited) "(exit code $($app.ExitCode))"

    $cfg = Join-Path $env:APPDATA "TaskbarHWM\config.toml"
    Check "wrote its config file" ((Test-Path $cfg) -and (Get-Item $cfg).Length -gt 0)

    $crash = Join-Path $env:LOCALAPPDATA "TaskbarHWM\crash.log"
    Check "no crash log" (-not (Test-Path $crash)) $(if (Test-Path $crash) { Get-Content $crash -Raw })

    $second = Start-Process $Exe -PassThru
    Check "second copy exits straight away" ($second.WaitForExit(8000) -and $second.ExitCode -eq 0)

    $claude = Start-Process $Exe -ArgumentList "--claude", "off" -PassThru
    Check "--claude off exits with 0" ($claude.WaitForExit(15000) -and $claude.ExitCode -eq 0)

    $win = [Smoke]::Find([uint32]$app.Id, "TaskbarHWM.Widget")
    Check "its window exists (message loop is running)" ($win -ne [IntPtr]::Zero)
    if ($win -ne [IntPtr]::Zero -and -not $app.HasExited) {
        [void][Smoke]::PostMessage($win, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)   # WM_CLOSE
        Check "closes cleanly when asked" ($app.WaitForExit(10000) -and $app.ExitCode -eq 0) "(exit code $($app.ExitCode))"
    }
} finally {
    if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue }
    Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue
}

if ($failed.Count -gt 0) { Write-Host "`n$($failed.Count) check(s) failed." -ForegroundColor Red; exit 1 }
Write-Host "`nAll runtime checks passed."
