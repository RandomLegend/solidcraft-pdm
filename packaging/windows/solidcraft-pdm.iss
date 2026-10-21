; SolidCraft PDM installer (Inno Setup 6). Built by the manual windows:build CI job:
;   ISCC.exe /DAppVersion=0.1.0 packaging\windows\solidcraft-pdm.iss
; after `cargo build --release -p scpdm-gui -p scpdm`. Installs for the current user only (no
; administrator rights needed), into %LOCALAPPDATA%\Programs\SolidCraft PDM.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Never change the AppId: it identifies the app for updates and uninstalling.
AppId={{88835AA5-E594-4BB7-923F-E72D2B62F443}
AppName=SolidCraft PDM
AppVersion={#AppVersion}
AppVerName=SolidCraft PDM {#AppVersion}
DefaultDirName={localappdata}\Programs\SolidCraft PDM
; An update closes the running app first (it quits by itself, this is for other copies).
CloseApplications=yes
DefaultGroupName=SolidCraft PDM
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=SolidCraft-PDM-{#AppVersion}-setup
SetupIconFile=..\..\assets\solidcraft-pdm.ico
UninstallDisplayIcon={app}\solidcraft-pdm.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\..\target\release\solidcraft-pdm.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\target\release\scpdm.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\docs\windows.md"; DestDir: "{app}"; Flags: ignoreversion
; The interface fonts (IBM Plex) are built into the program; their licence goes along.
Source: "..\..\assets\fonts\OFL-IBM-Plex.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\SolidCraft PDM"; Filename: "{app}\solidcraft-pdm.exe"
Name: "{userdesktop}\SolidCraft PDM"; Filename: "{app}\solidcraft-pdm.exe"; Tasks: desktopicon

[Run]
; Also after a silent install: that is how the app updates itself (it quits, the installer runs
; with /SILENT and starts it again).
Filename: "{app}\solidcraft-pdm.exe"; Description: "Start SolidCraft PDM"; Flags: nowait postinstall
