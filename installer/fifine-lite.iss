; Inno Setup script for FIFINE Lite. Build it with scripts\release.ps1, which passes
; the version and checks that the release executable is fresh.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=x.y.z fifine-lite.iss
#endif

#define AppName "FIFINE Lite"
#define AppExe "fifine-lite.exe"
#define AppUrl "https://github.com/Holorigg/fifine-lite"
#define RunKey "Software\Microsoft\Windows\CurrentVersion\Run"

[Setup]
AppId={{6E1B2C3A-9F4D-4B7E-A1C8-3D5F7A2E9B41}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Holorigg
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
AppCopyright=© 2026 Holorigg
VersionInfoVersion={#AppVersion}
; Per-user install into %LOCALAPPDATA%\Programs: no administrator rights needed.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\LICENSE
SetupIconFile=..\ui\images\app.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
OutputDir=..\dist
OutputBaseFilename=FIFINE-Lite-{#AppVersion}-setup-x64
; Close a running copy before files are replaced or removed.
AppMutex=Local\fifine-lite-instance
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.StartWithWindows=Start with Windows (in the tray)
russian.StartWithWindows=Запускать вместе с Windows (в трее)
english.DeleteSettings=Also delete the FIFINE Lite settings (lighting, effect names, window options)?
russian.DeleteSettings=Удалить и настройки FIFINE Lite (подсветка, названия эффектов, параметры окна)?

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "autostart"; Description: "{cm:StartWithWindows}"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; The same entry the app writes from its own settings page.
Root: HKCU; Subkey: "{#RunKey}"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"" --tray"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
{ Removes the autostart entry only when it points at this installation, so a copy
  installed elsewhere keeps starting with Windows. }
procedure RemoveAutostart();
var
  Value: String;
begin
  if RegQueryStringValue(HKCU, '{#RunKey}', '{#AppName}', Value) then
    if Pos(Lowercase(ExpandConstant('{app}\{#AppExe}')), Lowercase(Value)) > 0 then
      RegDeleteValue(HKCU, '{#RunKey}', '{#AppName}');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
  begin
    RemoveAutostart();
    if SuppressibleMsgBox(CustomMessage('DeleteSettings'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES then
      DelTree(ExpandConstant('{userappdata}\fifine-lite'), True, True, True);
  end;
end;
