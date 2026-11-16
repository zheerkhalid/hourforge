; HourForge installer (Inno Setup 6). Per-user: no administrator rights needed.

#ifndef AppVersion
  #define AppVersion "1.0.0"
#endif

[Setup]
AppId={{7C4E2B19-5A6D-4F83-B1E7-3D9A60C2F5A8}
AppName=HourForge
AppVersion={#AppVersion}
AppVerName=HourForge {#AppVersion}
AppPublisher=Zheer Khalid
AppPublisherURL=https://zheer.dev
AppSupportURL=https://github.com/zheerkhalid/hourforge
AppUpdatesURL=https://github.com/zheerkhalid/hourforge/releases
DefaultDirName={autopf}\HourForge
DefaultGroupName=HourForge
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=HourForge-Setup
SetupIconFile=..\target\hourforge.ico
UninstallDisplayIcon={app}\HourForge.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=no
LicenseFile=..\LICENSE
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Zheer Khalid (zheer.dev)
VersionInfoDescription=HourForge Setup

[Tasks]
Name: "startup"; Description: "Start HourForge when I sign in to Windows (it tracks from the notification area)"; GroupDescription: "Tracking:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\HourForge.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\HourForge"; Filename: "{app}\HourForge.exe"
Name: "{group}\Uninstall HourForge"; Filename: "{uninstallexe}"
Name: "{autodesktop}\HourForge"; Filename: "{app}\HourForge.exe"; Tasks: desktopicon

[Registry]
; The same value the "Start with Windows" switch in Settings writes.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "HourForge"; ValueData: """{app}\HourForge.exe"" --background"; Tasks: startup; Flags: uninsdeletevalue

[Run]
Filename: "{app}\HourForge.exe"; Description: "{cm:LaunchProgram,HourForge}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Ask the tracker to write its data and exit; then make sure it is gone.
Filename: "{app}\HourForge.exe"; Parameters: "--quit"; Flags: runhidden; RunOnceId: "QuitHourForge"
Filename: "{cmd}"; Parameters: "/C timeout /T 2 /NOBREAK >NUL & taskkill /IM HourForge.exe /F"; Flags: runhidden; RunOnceId: "KillHourForge"

[UninstallDelete]
; The Settings switch may have added the startup value without the installer task.
Type: files; Name: "{app}\LICENSE.txt"

[Code]
// Before files are replaced on an upgrade: ask a running HourForge to save and exit.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Exe: String;
begin
  Result := '';
  Exe := ExpandConstant('{app}\HourForge.exe');
  if FileExists(Exe) then
  begin
    Exec(Exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
    Sleep(1500);
  end;
end;

// The recorded data in %LOCALAPPDATA%\HourForge is the user's: it is kept on
// uninstall unless they say otherwise. The startup value is always removed.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RegDeleteValue(HKEY_CURRENT_USER, 'Software\Microsoft\Windows\CurrentVersion\Run', 'HourForge');
    DataDir := ExpandConstant('{localappdata}\HourForge');
    if DirExists(DataDir) and not UninstallSilent then
      if MsgBox('Also delete everything HourForge recorded on this PC?' + #13#10 + DataDir + #13#10#13#10 +
                'Choose No to keep your history.', mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
        DelTree(DataDir, True, True, True);
  end;
end;
