; Inno Setup 6 script for the White Rabbit installer.
; Build after the Windows build has put the app together in dist\WhiteRabbit\ (.github/workflows/windows.yml):
;   iscc /DAppVersion=0.1.0 installer\white_rabbit.iss
; Output: dist\WhiteRabbit-setup.exe (same name in every release, so download links keep working)
;
; The installed app is all Rust (rust/): WhiteRabbit.exe is the monitor and tray icon, and WhiteRabbitWindow.exe
; the settings window it opens. It also drives the RGB keyboard and macropad, and offers to remove the old
; Framework 16 Keyboard Control app. Next to them: the firmware (res\releases), PresentMon and the licences.
; It starts when the user logs in.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
; The numbers-only version for the installer's file details (Windows needs e.g. 1.1.0, not 1.1.0-beta)
#ifndef AppVersionNumbers
  #define AppVersionNumbers "0.0.0"
#endif

[Setup]
AppId={{ED186F4E-2A8B-421A-A7B2-CA34A1532E4F}
AppName=White Rabbit
AppVersion={#AppVersion}
AppPublisher=kingof9x
AppPublisherURL=https://github.com/kingof9x/white-rabbit-framework-matrix
AppCopyright=MIT License
; File details shown in the installer's Properties. Without these the installer's file version is 0.0.0.0,
; and an exe with blank details looks more suspicious to antivirus scanners.
VersionInfoVersion={#AppVersionNumbers}
VersionInfoProductVersion={#AppVersionNumbers}
VersionInfoTextVersion={#AppVersion}
VersionInfoProductTextVersion={#AppVersion}
VersionInfoCompany=kingof9x
VersionInfoDescription=White Rabbit installer
VersionInfoProductName=White Rabbit
VersionInfoOriginalFileName=WhiteRabbit-setup.exe
DefaultDirName={localappdata}\Programs\WhiteRabbit
DefaultGroupName=White Rabbit
DisableProgramGroupPage=yes
DisableDirPage=yes
; Per-user install, no admin prompt
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=WhiteRabbit-setup
SetupIconFile=..\res\WhiteRabbit.ico
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

[InstallDelete]
; What the Python window (1.4 and before) left behind; the Rust app doesn't use any of it
Type: filesandordirs; Name: "{app}\_internal"

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

const
  // The Framework 16 Keyboard Control app, now part of White Rabbit
  OldKeyboardApp = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{49DC55DF-26AA-4912-98D2-497D748E6BBE}_is1';

// The old keyboard app's uninstaller, if it is installed (for this user, or for everyone)
function OldKeyboardUninstaller(): String;
begin
  Result := '';
  if not RegQueryStringValue(HKCU, OldKeyboardApp, 'UninstallString', Result) then
    RegQueryStringValue(HKLM, OldKeyboardApp, 'UninstallString', Result);
  Result := RemoveQuotes(Result);
end;

// White Rabbit now does what the keyboard app did, and only one program can drive the keyboard, so
// offer to remove it. Its settings are copied into White Rabbit the first time White Rabbit starts,
// and its settings file is left in place.
procedure RemoveOldKeyboardApp();
var
  Uninstaller: String;
  Code: Integer;
begin
  Uninstaller := OldKeyboardUninstaller();
  if (Uninstaller = '') or WizardSilent() then
    Exit;
  if MsgBox('Framework 16 Keyboard Control is installed on this PC.' + #13#10 + #13#10 +
      'White Rabbit now does everything it did: keyboard lighting, the audio show, key mapping, ' +
      'the macropad and keyboard firmware updates. Your keyboard settings are carried over.' + #13#10 + #13#10 +
      'Running both would make them fight over the keyboard. Remove Framework 16 Keyboard Control now?',
      mbConfirmation, MB_YESNO) = IDYES then
  begin
    if not ShellExec('', Uninstaller, '/SILENT /NORESTART /SUPPRESSMSGBOXES', '', SW_SHOW,
        ewWaitUntilTerminated, Code) then
      MsgBox('Framework 16 Keyboard Control could not be removed (' + SysErrorMessage(Code) + '). ' +
        'You can remove it from Settings > Apps.', mbInformation, MB_OK);
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    RemoveOldKeyboardApp();
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
