# Shared helpers for the build scripts. Dot-source this file.
$ErrorActionPreference = "Stop"

$script:Root = Split-Path -Parent $PSScriptRoot

# The exact PawnIOLib.dll we ship (official PawnIO 2.2.0, signed by namazso). The build refuses anything else.
$script:PawnIOLibSha256 = "D71F62627D66983BB9F5B1C269F27BCD8C1B8A46E794377A6330F84C198F4443"

function Get-AppVersion {
    $m = Select-String -Path (Join-Path $script:Root "Cargo.toml") -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $m) { throw "Couldn't read the version from Cargo.toml" }
    return $m.Matches[0].Groups[1].Value
}

function Assert-Redist {
    $dll = Join-Path $script:Root "redist\PawnIOLib.dll"
    if (-not (Test-Path $dll)) {
        throw "redist\PawnIOLib.dll is missing. Copy it from an installed PawnIO (C:\Program Files\PawnIO\PawnIOLib.dll)."
    }
    $hash = (Get-FileHash $dll -Algorithm SHA256).Hash
    if ($hash -ne $script:PawnIOLibSha256) {
        throw "redist\PawnIOLib.dll has an unexpected hash ($hash). Expected the official PawnIO 2.2.0 build."
    }
    $sig = Get-AuthenticodeSignature $dll
    if ($sig.Status -ne "Valid" -or $sig.SignerCertificate.Subject -notmatch "namazso") {
        throw "redist\PawnIOLib.dll is not validly signed by namazso (status: $($sig.Status))."
    }
}

# Builds the release exe (requireAdministrator manifest) and returns its path.
function Build-Exe {
    Push-Location $script:Root
    try {
        # A leftover dev override would bake an unelevated manifest into the release build.
        Remove-Item Env:\TASKBARHWM_ASINVOKER -ErrorAction SilentlyContinue
        Write-Host "==> cargo build --release" -ForegroundColor Cyan
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally { Pop-Location }
    $exe = Join-Path $script:Root "target\release\TaskbarHWM.exe"
    if (-not (Test-Path $exe)) { throw "Build did not produce $exe" }
    return $exe
}

# Copies the exe plus everything it needs into $Dest (one self-contained folder).
function Publish-AppFolder([string]$Exe, [string]$Dest) {
    # A running copy locks its exe and PawnIOLib.dll. It usually runs elevated, so a normal shell can't stop it.
    $running = Get-Process TaskbarHWM -ErrorAction SilentlyContinue
    if ($running -and (Test-Path $Dest)) {
        throw "TaskbarHWM is still running. Exit it first (right-click the widget > Exit, or end it from an administrator shell), then run this script again. The new exe is already built at $Exe."
    }
    if (Test-Path $Dest) { Remove-Item $Dest -Recurse -Force }
    New-Item -ItemType Directory -Force $Dest | Out-Null
    Copy-Item $Exe (Join-Path $Dest "TaskbarHWM.exe")
    Copy-Item (Join-Path $script:Root "redist\PawnIOLib.dll") $Dest
    Copy-Item (Join-Path $script:Root "redist\THIRD-PARTY-NOTICES.txt") $Dest
    Copy-Item (Join-Path $script:Root "redist\COPYING.LGPL-2.1.txt") $Dest
}

function Show-Result([string]$Path) {
    $item = Get-Item $Path
    $size = if ($item.PSIsContainer) { (Get-ChildItem $Path -Recurse -File | Measure-Object Length -Sum).Sum } else { $item.Length }
    Write-Host ("`nDone: {0}  ({1:N0} KB)" -f $Path, ($size / 1KB)) -ForegroundColor Green
}
