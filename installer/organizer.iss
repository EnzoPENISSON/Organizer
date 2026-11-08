; Inno Setup script for Organizer. Built by the release workflow:
;   ISCC.exe /DAppVersion=0.1.0 /DAppBuild=12 installer\organizer.iss
; Output: target\installer\Organizer-Setup-v<version>-build.<build>.exe, the name the
; in-app updater looks for (src/update.rs).
;
; Installs for the current user only, so neither install nor update needs admin rights.
; The updater runs it with /SILENT: progress is shown, nothing is asked, and Organizer
; restarts at the end.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AppBuild
  #define AppBuild "0"
#endif
#define AppExe "organizer.exe"

[Setup]
; Never change AppId: it is how updates find the existing installation.
AppId={{05A01CED-374F-44F6-A197-71304AB4EB6E}
AppName=Organizer
AppVersion={#AppVersion}.{#AppBuild}
AppVerName=Organizer {#AppVersion} (build {#AppBuild})
AppPublisher=Enzo PENISSON
AppPublisherURL=https://github.com/EnzoPENISSON/Organizer
AppUpdatesURL=https://github.com/EnzoPENISSON/Organizer/releases
VersionInfoVersion={#AppVersion}.{#AppBuild}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Organizer
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile=..\assets\organizer.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName=Organizer
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\target\installer
OutputBaseFilename=Organizer-Setup-v{#AppVersion}-build.{#AppBuild}
LicenseFile=..\LICENSE

[Tasks]
Name: autostart; Description: "Start Organizer with Windows"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Organizer"; Filename: "{app}\{#AppExe}"

[Registry]
; Same value the tray menu's "Start with Windows" writes. Left alone on silent updates so
; the user's choice in the tray menu is kept.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; \
  ValueName: "Organizer"; ValueData: """{app}\{#AppExe}"""; Tasks: autostart; \
  Check: not WizardSilent; Flags: uninsdeletevalue
; Created by the app itself (desktop right-click > New box); removed on uninstall.
Root: HKCU; Subkey: "Software\Classes\DesktopBackground\Shell\OrganizerNewBox"; \
  Flags: uninsdeletekey dontcreatekey

[Run]
Filename: "{app}\{#AppExe}"; Description: "Launch Organizer"; Flags: nowait postinstall skipifsilent
; Silent installs are updates started from the app, which quit to let them run: restart it.
Filename: "{app}\{#AppExe}"; Flags: nowait; Check: WizardSilent

[UninstallRun]
Filename: "{app}\{#AppExe}"; Parameters: "exit"; Flags: runhidden waituntilterminated; RunOnceId: "StopOrganizer"

; Boxes, theme.css and the log in %APPDATA%\Organizer are kept, so a reinstall picks them up.

[Code]
// Closes a running Organizer cleanly (tray icon removed, boxes saved) before its exe is
// replaced. `organizer.exe exit` waits until the running copy has quit.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  if FileExists(ExpandConstant('{app}\{#AppExe}')) then
    Exec(ExpandConstant('{app}\{#AppExe}'), 'exit', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;
