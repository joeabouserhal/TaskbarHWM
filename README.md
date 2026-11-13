# TaskbarHWM

A small hardware monitor that lives at the left edge of the Windows 11 taskbar. CPU, GPU and RAM
readings (usage, clocks, temperature, power, VRAM) in a compact widget; click it for a flyout with
details and settings. Written in Rust with Win32, Direct2D and DirectComposition: no GUI framework,
about 0.7 MB on disk and a few MB of memory.

## Building

Prerequisites: Rust (MSVC toolchain). For the installer also Inno Setup 6.1+
(`winget install JRSoftware.InnoSetup`).

There are two build flows. Both produce a release build whose exe asks for administrator rights
(needed to read CPU temperature and power through the PawnIO driver).

### 1. The app only: `dist\app\`

```powershell
powershell -File scripts\build-app.ps1
```

Produces a self-contained folder:

```
dist\app\
  TaskbarHWM.exe          the app (static C runtime, no Visual C++ redistributable needed)
  PawnIOLib.dll           helper library for the PawnIO driver (official 2.2.0, unmodified)
  THIRD-PARTY-NOTICES.txt
  COPYING.LGPL-2.1.txt
```

Use this when PawnIO is already installed. Copy the folder anywhere and run `TaskbarHWM.exe`.

### 2. The installer: `dist\TaskbarHWM-Setup-<version>.exe`

```powershell
powershell -File scripts\build-installer.ps1
```

Builds the app, then compiles `installer\TaskbarHWM.iss`. The wizard:

1. shows a **System check** page (Windows version, PawnIO driver, NVIDIA driver library, existing install);
2. installs everything into one folder, by default `C:\Program Files\TaskbarHWM`;
3. if the PawnIO driver is missing or older than 2.2.0, downloads the official installer from GitHub,
   checks a pinned SHA-256, and runs it silently;
4. optionally enables **Start with Windows** (a logon task with highest privileges, so no UAC prompt at sign-in).

Uninstalling removes the app and its autostart task. PawnIO is left in place because other programs may use it.

Silent install for scripts: `TaskbarHWM-Setup-0.1.0.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`
(add `/TASKS="!autostart"` to skip autostart). `/DRYRUNPAWNIO` pretends PawnIO is missing and only
downloads and verifies its installer, which is useful for testing.

## Bundled files

`redist\PawnIOLib.dll` must be the official signed PawnIO 2.2.0 build; the scripts check its SHA-256 and
signature. To bump PawnIO, update `scripts\common.ps1`, `installer\TaskbarHWM.iss` and
`src\shell\setup.rs` together (URL, size and hashes), then replace `redist\PawnIOLib.dll` and the modules
in `res\pawnio\`.

`scripts\make-icon.ps1` regenerates `res\app.ico`.

## Development

`cargo run` builds a debug exe that runs unelevated (CPU temperature and power then show as unavailable).
`TaskbarHWM.exe --dump [file]` prints a few seconds of sensor readings.
