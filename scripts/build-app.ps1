<#
.SYNOPSIS
  Flow 1: build the app directly into a ready-to-run folder (no installer).

.DESCRIPTION
  Produces dist\app\ containing TaskbarHWM.exe, PawnIOLib.dll and the license notices. Use this when you
  already have PawnIO installed and just want the app: copy the folder anywhere and run TaskbarHWM.exe
  (it asks for administrator rights, which CPU temperature and power need).

  Usage:  powershell -File scripts\build-app.ps1
#>
. "$PSScriptRoot\common.ps1"

Assert-Redist
$exe  = Build-Exe
$dest = Join-Path $script:Root "dist\app"
Publish-AppFolder -Exe $exe -Dest $dest
Get-ChildItem $dest | ForEach-Object { Write-Host ("   {0,-28} {1,10:N0} bytes" -f $_.Name, $_.Length) }
Show-Result $dest
