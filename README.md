# TaskbarHWM

A small hardware monitor that lives at the left edge of the Windows 11 taskbar. It shows live CPU, GPU,
memory, disk and network readings in a compact widget; click it for a flyout with details, and open
**Settings** (from the flyout's gear, or by right-clicking the widget) to choose what is shown.

Written in Rust with plain Win32, Direct2D and DirectComposition: no GUI framework, no .NET, no background
service. The app is under 1 MB on disk and typically uses 10 to 30 MB of memory.

## What it shows

| Section | Readings |
|---|---|
| CPU | usage, clock, temperature, power |
| GPU | usage, temperature, power, core clock, memory clock, VRAM, fan. Several GPUs are supported (for example an NVIDIA card plus an AMD or Intel integrated GPU): pick which appear on the taskbar; with more than one they are labelled GPU1, GPU2 |
| Memory | usage, used / total |
| Disks | every local drive: used / total, % used or free space, as one combined row or one row per drive |
| Network | download and upload speed, as one row or split into two. Count all adapters as a total, or only the ones you pick; show MB/s or Mbps |

Everything is optional. For each reading you choose whether it appears on the taskbar, in the flyout, or both.
The widget's rows and columns can be reordered by dragging, and text size, spacing, labels (icon, text or both),
badges and flyout transparency are adjustable.

Units ("GHz", "%", "GB") are drawn smaller and muted so the number stands out.

## Requirements

* Windows 11 (64-bit). Windows 10 works for the sensors but the widget is designed for the Windows 11 taskbar.
* Administrator rights. The app always runs elevated: reading CPU temperature and power needs a kernel driver.
* For GPU temperature, power and clocks, the GPU vendor's own driver: NVIDIA (NVML) or AMD (ADLX). Nothing is
  installed for these; the app uses the library the graphics driver already put in `System32`. Without them you
  still get GPU usage and VRAM.

## Installing

### With the installer

Run `TaskbarHWM-Setup-<version>.exe`. It is a normal wizard and asks for administrator rights.

1. **System check**: shows your Windows version, whether the PawnIO driver is installed, and whether an NVIDIA
   driver library was found. Nothing to do on this page.
2. **Install folder**: `C:\Program Files\TaskbarHWM` by default.
3. **Options**: *Start TaskbarHWM when I sign in* (see below).
4. **Install**: copies the files, then installs the PawnIO driver if it is missing or older than 2.2.0.
5. **Finish**: tells you whether the driver is active or needs a restart, and offers to launch the app.

On first launch the Settings page opens once so you can see what can be changed.

### Without the installer

Copy the `dist\app` folder anywhere and run `TaskbarHWM.exe`. If the PawnIO driver is not installed, the app
shows a card in **Settings > PawnIO driver** with an **Install driver** button; everything except CPU temperature
and power works without it.

## What is installed, and where

| What | Where | Notes |
|---|---|---|
| The app and its files | `C:\Program Files\TaskbarHWM\` | `TaskbarHWM.exe`, `PawnIOLib.dll`, licence notices, and the uninstaller |
| Start menu shortcut | Start menu > Programs > TaskbarHWM | |
| PawnIO driver (only if missing or older than 2.2.0) | Driver: `C:\Windows\System32\DriverStore\...\PawnIO.sys`, registered as the `PawnIO` service. Tools: `C:\Program Files\PawnIO\` | Installed by PawnIO's own installer, downloaded from its official GitHub release |
| Start-with-Windows entry (optional) | A Task Scheduler task named `TaskbarHWM`, set to run at sign-in with highest privileges | A scheduled task rather than a Run key, so there is no UAC prompt at sign-in |
| Your settings | `%APPDATA%\TaskbarHWM\config.toml` | Created on first run. If the file is ever unreadable it is kept as `config.toml.bad` before defaults are used |
| Crash report | `%LOCALAPPDATA%\TaskbarHWM\crash.log` | Only written if the app crashes |

About PawnIO: it is a small, signed, open-source driver that lets programs read CPU model-specific registers
safely. TaskbarHWM uses it for CPU temperature and power on AMD (Zen) and Intel CPUs. The installer downloads
PawnIO's official installer, checks it against a pinned SHA-256 (and size), copies it into the admin-only install
folder, checks it again, runs it silently, and deletes it. If PawnIO reports that a restart is needed, the wizard
says so; CPU temperature and power start working after the restart, and the rest works immediately.

The PawnIO modules the app needs (`AMDFamily17.bin`, `IntelMSR.bin`) are compiled into `TaskbarHWM.exe`; only
`PawnIOLib.dll` sits beside it.

### Where the readings come from

| Reading | Source |
|---|---|
| CPU usage and clock | Windows performance counters |
| CPU temperature and power | PawnIO |
| GPU usage and VRAM (any vendor) | Windows performance counters and DXGI |
| NVIDIA GPU details | `nvml.dll` from the NVIDIA driver |
| AMD GPU details | `amdadlx64.dll` from the Radeon driver |
| GPU temperature (fallback for any vendor) | the Windows graphics kernel (D3DKMT) |
| Memory | `GlobalMemoryStatusEx` |
| Disks | drive letters of fixed and removable local drives (network and optical drives are skipped on purpose) |
| Network | the interface table's byte counters, physical adapters only (VPNs and virtual switches would be counted twice) |

Integrated AMD GPUs do not report their own power (the driver gives whole-chip power), so it is left blank
rather than shown wrongly.

## Uninstalling

Use *Apps > Installed apps*, or the uninstaller in the install folder. It closes the app, removes the files, the
shortcut and the scheduled task, and asks whether to delete your settings. **PawnIO is left installed** because
other programs may use it; remove it separately from *Apps > Installed apps* if you want it gone.

## Building

Prerequisites: Rust (MSVC toolchain). For the installer also Inno Setup 6.1 or newer
(`winget install JRSoftware.InnoSetup`).

There are two build flows. Both produce a release build whose exe asks for administrator rights.

### 1. The app only: `dist\app\`

```powershell
powershell -File scripts\build-app.ps1
```

```
dist\app\
  TaskbarHWM.exe          the app (static C runtime, no Visual C++ redistributable needed)
  PawnIOLib.dll           helper library for the PawnIO driver (official 2.2.0, unmodified)
  THIRD-PARTY-NOTICES.txt
  COPYING.LGPL-2.1.txt
```

Stop the running app first: the script refuses to overwrite files that are in use.

### 2. The installer: `dist\TaskbarHWM-Setup-<version>.exe`

```powershell
powershell -File scripts\build-installer.ps1
```

Builds the app, then compiles `installer\TaskbarHWM.iss`.

Silent install for scripts: `TaskbarHWM-Setup-0.1.0.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`
(add `/TASKS="!autostart"` to skip autostart). `/DRYRUNPAWNIO` pretends PawnIO is missing and only downloads and
verifies its installer, which is useful for testing.

## Development

`cargo run` builds a debug exe that runs unelevated (CPU temperature and power then show as unavailable).
To get an unelevated *release* build for automated tests, set `TASKBARHWM_ASINVOKER=1` while building.

```
TaskbarHWM.exe --dump [file]        print a few seconds of sensor readings (CPUs, GPUs, memory, network)
TaskbarHWM.exe --autostart on|off   create or remove the sign-in task
TaskbarHWM.exe --verify-installer   check the PawnIO download and hash steps without running the installer
cargo test                          unit tests (sensor decoding, value formatting, network totals)
```

Environment switches for development: `HWM_KEEP_OPEN=1` keeps the flyout open when it loses focus;
`HWM_FAKE_PAWN=active|missing|restart|failed` previews the PawnIO card's states.

Source layout: `src\sensors` (CPU, GPU, memory, disk, network samplers), `src\metrics.rs` (the catalogue of
readings and how they are formatted and laid out), `src\ui` (the widget, the flyout and its drawing code),
`src\shell` (autostart and the driver setup), `installer` and `scripts` (the two build flows).

### Bumping PawnIO

`redist\PawnIOLib.dll` must be the official signed PawnIO build; the scripts check its SHA-256 and signature.
To bump PawnIO, update `scripts\common.ps1`, `installer\TaskbarHWM.iss` and `src\shell\setup.rs` together (URL,
size and hashes), then replace `redist\PawnIOLib.dll` and the modules in `res\pawnio\`.
`scripts\make-icon.ps1` regenerates `res\app.ico`.

## Licences

TaskbarHWM bundles the PawnIO helper library (LGPL-2.1) and uses PawnIO's official modules; see
`THIRD-PARTY-NOTICES.txt` and `COPYING.LGPL-2.1.txt`. The AMD ADLX and NVIDIA NVML interfaces are bound by hand
against the vendors' public headers, and their libraries come from the graphics driver, not from this project.
