; Inno Setup 6 script for the White Rabbit installer.
; Build after the one-dir PyInstaller build (dist\WhiteRabbit\):
;   iscc /DAppVersion=0.1.0 installer\white_rabbit.iss
; Output: dist\WhiteRabbit-setup.exe (same name in every release, so download links keep working)
;
; The installed app is the same as the portable exe, except that it starts when the user logs in.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{ED186F4E-2A8B-421A-A7B2-CA34A1532E4F}
AppName=White Rabbit
AppVersion={#AppVersion}
AppPublisher=kingof9x
AppPublisherURL=https://github.com/kingof9x/white-rabbit-framework-matrix
DefaultDirName={localappdata}\Programs\WhiteRabbit
DefaultGroupName=White Rabbit
DisableProgramGroupPage=yes
DisableDirPage=yes
; Per-user install, no admin prompt
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=WhiteRabbit-setup
SetupIconFile=..\build\WhiteRabbit.ico
UninstallDisplayIcon={app}\WhiteRabbit.exe
UninstallDisplayName=White Rabbit
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Close the running monitor and window before installing or uninstalling. The mutex names match the app's.
CloseApplications=yes
AppMutex=WhiteRabbit_single_instance,WhiteRabbit_window

[Files]
Source: "..\dist\WhiteRabbit\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\White Rabbit"; Filename: "{app}\WhiteRabbit.exe"
Name: "{group}\Uninstall White Rabbit"; Filename: "{uninstallexe}"

[Registry]
; Start with Windows on a first install (monitor and tray icon only, no window). After that it is up to
; the "Start with Windows" item in the tray menu (the same registry value), so an update does not turn it back on.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "WhiteRabbit"; ValueData: """{app}\WhiteRabbit.exe"" --background"; Flags: uninsdeletevalue; Check: not IsUpgrade

[Run]
Filename: "{app}\WhiteRabbit.exe"; Description: "Open White Rabbit"; Flags: nowait postinstall skipifsilent

; The user's settings in %APPDATA%\WhiteRabbit are not touched by the uninstaller.

[Code]
var
  WasInstalled: Boolean;

function InitializeSetup(): Boolean;
begin
  // Decided before anything is written, so this install's own uninstall entry can't count
  WasInstalled := RegValueExists(HKCU,
    'Software\Microsoft\Windows\CurrentVersion\Uninstall\{ED186F4E-2A8B-421A-A7B2-CA34A1532E4F}_is1',
    'UninstallString');
  Result := True;
end;

function IsUpgrade(): Boolean;
begin
  Result := WasInstalled;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // Remove the startup entry even if it was turned on from the tray menu after installing
  if CurUninstallStep = usUninstall then
  begin
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'WhiteRabbit');
    RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run', 'WhiteRabbit');
  end;
end;
