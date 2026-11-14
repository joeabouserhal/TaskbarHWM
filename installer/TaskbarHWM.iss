; TaskbarHWM installer (Inno Setup 6.1 or newer).
; Build it with:  powershell -File scripts\build-installer.ps1
;
; What the wizard does:
;   * shows a "System check" page (Windows, PawnIO driver, NVIDIA driver library, existing install)
;   * installs TaskbarHWM.exe, PawnIOLib.dll and the license notices into ONE folder (default: Program Files)
;   * if the PawnIO driver is missing (or older than the version we ship against), downloads the official
;     installer, verifies a pinned SHA-256, and runs it silently
;   * optionally enables "Start with Windows" (a logon task that runs with highest privileges)
; Uninstall removes the app and its autostart task. PawnIO is left alone: other tools may use it.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\app"
#endif

#define AppName "TaskbarHWM"
#define AppExe "TaskbarHWM.exe"
#define AppIdGuid "{C6F48C74-FEBC-4624-9743-A24DB08D936C}"

; Official PawnIO installer, pinned. Keep in sync with src\shell\setup.rs.
#define PawnIOUrl "https://github.com/namazso/PawnIO.Setup/releases/download/2.2.0/PawnIO_setup.exe"
#define PawnIOSha256 "1f519a22e47187f70a1379a48ca604981c4fcf694f4e65b734aaa74a9fba3032"
#define PawnIOMinVersion "2.2.0.0"

[Setup]
AppId={{C6F48C74-FEBC-4624-9743-A24DB08D936C}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
WizardStyle=modern
SetupIconFile=..\res\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename=TaskbarHWM-Setup-{#AppVersion}
Compression=lzma2/ultra
SolidCompression=yes
RestartIfNeededByRun=no
CloseApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in"; GroupDescription: "Options:"

[Files]
Source: "{#SourceDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\PawnIOLib.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\COPYING.LGPL-2.1.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"

[Run]
; The installer is already elevated, so the app can create its logon task without another prompt.
Filename: "{app}\{#AppExe}"; Parameters: "--autostart on"; Flags: runhidden waituntilterminated; Tasks: autostart; StatusMsg: "Enabling start with Windows..."
; "Launch" entries run as the original (unelevated) user by default, which fails with error 740 because the app
; requires administrator rights. runascurrentuser starts it with Setup's own elevated token, so no second prompt.
Filename: "{app}\{#AppExe}"; Description: "Launch {#AppName}"; Flags: nowait postinstall skipifsilent runascurrentuser

[UninstallRun]
Filename: "{app}\{#AppExe}"; Parameters: "--autostart off"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveAutostart"
; If Claude usage was connected, take TaskbarHWM out of Claude Code's status line so it isn't left pointing at a script that is gone.
Filename: "{app}\{#AppExe}"; Parameters: "--claude off"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveClaudeStatusLine"

[Code]
var
  CheckPage: TOutputMsgMemoWizardPage;
  PawnNeeded: Boolean;        { install or update the PawnIO driver }
  PawnDryRun: Boolean;        { /DRYRUNPAWNIO: pretend it's missing, download + verify, but don't run it }
  PawnDownloaded: Boolean;
  PawnNeedsRestart: Boolean;
  PawnSummary: String;        { one line shown on the finish page }

{ ---------- helpers ---------- }

function HasParam(const Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), Name) = 0 then
    begin
      Result := True;
      Exit;
    end;
end;

function PawnIOInstalledVersion(var Version: String): Boolean;
begin
  Result := RegQueryStringValue(HKLM64, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\PawnIO', 'DisplayVersion', Version);
end;

function PawnIODriverPresent: Boolean;
begin
  Result := RegKeyExists(HKLM64, 'SYSTEM\CurrentControlSet\Services\PawnIO');
end;

function VersionAtLeast(const Version, Minimum: String): Boolean;
var
  A, B: Int64;
begin
  Result := StrToVersion(Version, A) and StrToVersion(Minimum, B) and (ComparePackedVersion(A, B) >= 0);
end;

{ Decides whether the PawnIO driver has to be installed, and returns a human-readable status line. }
function EvaluatePawnIO: String;
var
  V: String;
begin
  PawnDryRun := HasParam('/DRYRUNPAWNIO');
  PawnNeeded := False;
  if PawnDryRun then
  begin
    PawnNeeded := True;
    Result := 'Dry run (/DRYRUNPAWNIO): pretending the driver is missing. The installer will be downloaded and verified but NOT run.';
    Exit;
  end;
  if PawnIODriverPresent then
  begin
    if PawnIOInstalledVersion(V) then
    begin
      if VersionAtLeast(V, '{#PawnIOMinVersion}') then
        Result := 'Installed (version ' + V + '). Setup will leave it as it is.'
      else
      begin
        PawnNeeded := True;
        Result := 'Version ' + V + ' is older than the required {#PawnIOMinVersion}. Setup will update it (about 3.3 MB download, silent).';
      end;
    end
    else
      Result := 'Installed. Setup will leave it as it is.';
  end
  else
  begin
    PawnNeeded := True;
    Result := 'Not installed. Setup will download the official installer (about 3.3 MB), check its SHA-256, and install it silently.';
  end;
end;

function BuildReport: String;
var
  Win: TWindowsVersion;
  Prev, Line: String;
begin
  GetWindowsVersionEx(Win);
  { Windows 11 still reports version 10.0; the build number (22000+) tells them apart. }
  if Win.Build >= 22000 then
  begin
    Line := 'Windows 11 (build ' + IntToStr(Win.Build) + '), 64-bit';
    Result := '[OK]  ' + Line + #13#10;
  end
  else
  begin
    Line := 'Windows 10 (build ' + IntToStr(Win.Build) + '), 64-bit';
    Result := '[!]   ' + Line + '. TaskbarHWM is designed for the Windows 11 taskbar; it may not line up on Windows 10.' + #13#10;
  end;

  Result := Result + '[OK]  Administrator rights: granted' + #13#10;
  Result := Result + #13#10;

  Result := Result + 'PawnIO driver (CPU temperature and power):' + #13#10 + '      ' + EvaluatePawnIO + #13#10 + #13#10;

  if FileExists(ExpandConstant('{sys}\nvml.dll')) then
    Result := Result + '[OK]  NVIDIA driver library (nvml.dll) found: GPU temperature, power and clocks will be available.' + #13#10
  else
    Result := Result + '[i]   NVIDIA driver library (nvml.dll) not found: GPU usage and VRAM still work, but GPU temperature, power and clocks need an NVIDIA GPU with its driver.' + #13#10;

  if RegQueryStringValue(HKLM64, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{#AppIdGuid}_is1', 'DisplayVersion', Prev) then
    Result := Result + '[i]   {#AppName} ' + Prev + ' is already installed. Setup will update it and keep your settings.' + #13#10;
end;

procedure StopApp;
var
  Code: Integer;
begin
  { Ask nicely first (WM_CLOSE), then force. The app runs elevated, and so does Setup. }
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Sleep(800);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

{ ---------- wizard ---------- }

procedure InitializeWizard;
var
  Report: String;
begin
  Report := BuildReport;
  Log('System check report:' + #13#10 + Report);
  CheckPage := CreateOutputMsgMemoPage(wpWelcome, 'System check', 'Setup checked your PC',
    'Here is what Setup found. There is nothing you need to do.', Report);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopApp;
  Result := '';
end;

function OnDownloadProgress(const Url, FileName: String; const Progress, ProgressMax: Int64): Boolean;
begin
  if ProgressMax > 0 then
    WizardForm.StatusLabel.Caption := Format('Downloading the PawnIO driver installer... %d%%', [Progress * 100 div ProgressMax]);
  Result := True;
end;

procedure DownloadPawnIO;
begin
  if not PawnNeeded then Exit;
  WizardForm.StatusLabel.Caption := 'Downloading the PawnIO driver installer...';
  try
    { Fails (and deletes the file) if the SHA-256 doesn't match. }
    DownloadTemporaryFile('{#PawnIOUrl}', 'PawnIO_setup.exe', '{#PawnIOSha256}', @OnDownloadProgress);
    PawnDownloaded := True;
  except
    PawnSummary := 'The PawnIO driver could not be downloaded (' + GetExceptionMessage + '). ' +
      '{#AppName} works without it, except for CPU temperature and power. You can install it later from {#AppName} > Settings > PawnIO driver.';
    Log('PawnIO download failed: ' + GetExceptionMessage);
  end;
end;

procedure InstallPawnIO;
var
  Tmp, Safe: String;
  Code: Integer;
begin
  if (not PawnNeeded) or (not PawnDownloaded) then Exit;
  Tmp := ExpandConstant('{tmp}\PawnIO_setup.exe');
  { %TEMP% can be written by unelevated programs, so copy the verified file into the admin-only install
    folder and check its hash again right before running it. }
  Safe := ExpandConstant('{app}\PawnIO_setup.exe');
  if not FileCopy(Tmp, Safe, False) then
  begin
    PawnSummary := 'The PawnIO installer could not be prepared. Install it later from {#AppName} > Settings.';
    Exit;
  end;
  try
    if CompareText(GetSHA256OfFile(Safe), '{#PawnIOSha256}') <> 0 then
    begin
      PawnSummary := 'The PawnIO installer failed its integrity check and was not run.';
      Log('PawnIO hash mismatch after copy');
      Exit;
    end;
    if PawnDryRun then
    begin
      PawnSummary := 'Dry run: the PawnIO installer was downloaded and its SHA-256 verified. It was NOT run.';
      Log(PawnSummary);
      Exit;
    end;
    WizardForm.StatusLabel.Caption := 'Installing the PawnIO driver...';
    if not Exec(Safe, '-install -silent', '', SW_HIDE, ewWaitUntilTerminated, Code) then
    begin
      PawnSummary := 'The PawnIO installer could not be started.';
    end
    else if Code = 0 then
      PawnSummary := 'PawnIO driver installed. CPU temperature and power are available.'
    else if (Code = 3010) or (Code = 1641) then
    begin
      PawnNeedsRestart := True;
      PawnSummary := 'PawnIO driver installed. Restart Windows to finish enabling CPU temperature and power.';
    end
    else
      PawnSummary := 'The PawnIO installer stopped with code ' + IntToStr(Code) + '. {#AppName} still works; retry from Settings > PawnIO driver.';
    Log('PawnIO installer exit code: ' + IntToStr(Code));
  finally
    DeleteFile(Safe);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    DownloadPawnIO
  else if CurStep = ssPostInstall then
    InstallPawnIO;
end;

function NeedRestart: Boolean;
begin
  Result := PawnNeedsRestart;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpFinished) and (PawnSummary <> '') then
    WizardForm.FinishedLabel.Caption := WizardForm.FinishedLabel.Caption + #13#10#13#10 + PawnSummary;
end;

{ ---------- uninstall ---------- }

function InitializeUninstall: Boolean;
begin
  StopApp;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and (not UninstallSilent) then
  begin
    if MsgBox('Also delete your {#AppName} settings?' + #13#10 + '(PawnIO is left installed: other programs may use it.)',
      mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
    begin
      DelTree(ExpandConstant('{userappdata}\{#AppName}'), True, True, True);
      DelTree(ExpandConstant('{localappdata}\{#AppName}'), True, True, True);
    end;
  end;
end;
