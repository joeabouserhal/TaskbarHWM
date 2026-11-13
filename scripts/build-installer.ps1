<#
.SYNOPSIS
  Flow 2: build the installer wizard (dist\TaskbarHWM-Setup-<version>.exe).

.DESCRIPTION
  Builds the release exe, stages dist\app\ (exe + PawnIOLib.dll + notices), then compiles
  installer\TaskbarHWM.iss with Inno Setup. The wizard checks the PC, installs everything into one
  folder (default C:\Program Files\TaskbarHWM), and silently installs the PawnIO driver if it's missing.

  Requires Inno Setup 6.1+ (winget install JRSoftware.InnoSetup).

  Usage:  powershell -File scripts\build-installer.ps1
#>
. "$PSScriptRoot\common.ps1"

function Find-Iscc {
    $cmd = Get-Command iscc -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $candidates = @(
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) { if (Test-Path $c) { return $c } }
    throw "Inno Setup (ISCC.exe) not found. Install it with:  winget install JRSoftware.InnoSetup"
}

$iscc = Find-Iscc
Assert-Redist
$version = Get-AppVersion
$exe     = Build-Exe
$stage   = Join-Path $script:Root "dist\app"
Publish-AppFolder -Exe $exe -Dest $stage

Write-Host "==> compiling installer ($iscc)" -ForegroundColor Cyan
& $iscc "/DAppVersion=$version" "/DSourceDir=$stage" "/Qp" (Join-Path $script:Root "installer\TaskbarHWM.iss")
if ($LASTEXITCODE -ne 0) { throw "Inno Setup compilation failed" }

Show-Result (Join-Path $script:Root "dist\TaskbarHWM-Setup-$version.exe")
