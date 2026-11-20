; The Moonglass installer for Windows (Inno Setup 6). Built by
; scripts\package-windows.ps1, which passes the defines below.
;
; It installs for the current user only, into
; %LOCALAPPDATA%\Programs\Moonglass, so it needs no administrator rights and
; the in-app updater can replace moonglass.exe there. The installed program
; is the same file as the portable .exe. Uninstalling removes the program
; and its shortcuts and keeps the library and settings, which live in
; %APPDATA%\Moonglass.

#ifndef Root
  #error Run scripts\package-windows.ps1, which defines Root, AppVersion, NumericVersion, OutputDir and OutputName.
#endif

[Setup]
; Never change: Windows finds an earlier install by it.
AppId={{EB8973CA-2D86-4FC8-BA25-44A939CA0CED}
AppName=Moonglass
AppVersion={#AppVersion}
AppVerName=Moonglass {#AppVersion}
AppPublisher=hxpe-dev
AppPublisherURL=https://github.com/hxpe-dev/moonglass-launcher
AppSupportURL=https://github.com/hxpe-dev/moonglass-launcher/issues
AppUpdatesURL=https://github.com/hxpe-dev/moonglass-launcher/releases
VersionInfoVersion={#NumericVersion}
DefaultDirName={autopf}\Moonglass
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile={#Root}\assets\logo.ico
UninstallDisplayIcon={app}\moonglass.exe
UninstallDisplayName=Moonglass
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
; A running Moonglass is closed before its file is replaced.
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Root}\target\release\moonglass.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Root}\dist\THIRD-PARTY-NOTICES.html"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist

[Icons]
Name: "{autoprograms}\Moonglass"; Filename: "{app}\moonglass.exe"
Name: "{autodesktop}\Moonglass"; Filename: "{app}\moonglass.exe"; Tasks: desktopicon

[Registry]
; Settings → System → Start Moonglass at login writes this value; it goes
; with the program.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Moonglass"; Flags: uninsdeletevalue dontcreatekey

[Run]
Filename: "{app}\moonglass.exe"; Description: "{cm:LaunchProgram,Moonglass}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; Left by the in-app updater.
Type: files; Name: "{app}\moonglass.exe.old"
Type: files; Name: "{app}\moonglass.exe.download"
