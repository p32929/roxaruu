; Roxaruu's Windows installer (Inno Setup), built by .github/workflows/build.yml:
;   iscc /DVersion=x.y.z packaging\roxaruu.iss
; For this user only (no admin prompt): the app in %LOCALAPPDATA%\Programs,
; the Start Menu, `rox` on PATH, and "Open with Roxaruu" in File Explorer.

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{6E2B9F3A-4C7D-4B8E-9A1F-52D3C8E7B140}
AppName=Roxaruu
AppVersion={#Version}
AppPublisher=p32929
AppPublisherURL=https://github.com/p32929/roxaruu
DefaultDirName={localappdata}\Programs\Roxaruu
DefaultGroupName=Roxaruu
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ChangesEnvironment=yes
ChangesAssociations=yes
OutputDir=..\dist
OutputBaseFilename=roxaruu_v{#Version}_windows_x64_setup
SetupIconFile=..\dist\roxaruu.ico
UninstallDisplayIcon={app}\roxaruu.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Updating while Roxaruu is open: ask it to close, and close it if it
; doesn't (Windows' request doesn't reach an egui window), rather than fail
; on a roxaruu.exe in use. Interactive installs still say so first.
CloseApplications=force

[Tasks]
Name: "addtopath"; Description: "Add the ""rox"" command to PATH (open a folder from a terminal: rox .)"
Name: "contextmenu"; Description: "Add ""Open with Roxaruu"" to File Explorer's right-click menu for folders"
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\roxaruu.exe"; DestDir: "{app}"; Flags: ignoreversion
; Claude Code's helper (see src/bin/roxaruu-hook.rs), beside the app.
Source: "..\target\release\roxaruu-hook.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\rox.cmd"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\dist\roxaruu.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Roxaruu"; Filename: "{app}\roxaruu.exe"; IconFilename: "{app}\roxaruu.ico"
Name: "{autodesktop}\Roxaruu"; Filename: "{app}\roxaruu.exe"; IconFilename: "{app}\roxaruu.ico"; Tasks: desktopicon

[Registry]
; Roxaruu's name and icon for its notifications (the app registers it as it
; starts; the uninstaller takes it away).
Root: HKCU; Subkey: "Software\Classes\AppUserModelId\p32929.Roxaruu"; Flags: uninsdeletekey dontcreatekey
; `rox` on PATH, for this user.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; Tasks: addtopath; Check: NeedsAddPath(ExpandConstant('{app}'))
; Right-click a folder, or the inside of one: Open with Roxaruu.
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Roxaruu"; ValueType: string; ValueData: "Open with Roxaruu"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Roxaruu"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\roxaruu.exe"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Roxaruu\command"; ValueType: string; ValueData: """{app}\roxaruu.exe"" ""%V"""; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Roxaruu"; ValueType: string; ValueData: "Open with Roxaruu"; Flags: uninsdeletekey; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Roxaruu"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\roxaruu.exe"; Tasks: contextmenu
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Roxaruu\command"; ValueType: string; ValueData: """{app}\roxaruu.exe"" ""%V"""; Tasks: contextmenu
; Folders only: the files' item earlier versions added goes (an update
; doesn't run the old uninstaller, which would have taken it).
Root: HKCU; Subkey: "Software\Classes\*\shell\Roxaruu"; ValueType: none; Flags: deletekey

[Run]
Filename: "{app}\roxaruu.exe"; Description: "Open Roxaruu"; Flags: nowait postinstall skipifsilent
; An update from inside Roxaruu (Restart to Update): open it again, with
; the folder it had (src/update.rs).
Filename: "{app}\roxaruu.exe"; Parameters: "--after-update"; Flags: nowait; Check: Relaunch

[Code]
function Relaunch: Boolean;
begin
  Result := ExpandConstant('{param:relaunch|0}') = '1';
end;

function NeedsAddPath(Dir: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;
