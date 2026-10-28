; Clipingo installer (Inno Setup 6). Build with installer\build.ps1.
; Per-user install (no administrator prompt), modern wizard, LZMA2. Tools and speech models are not bundled:
; the app downloads them on demand (Settings > Downloads). Updates run this installer silently (/SILENT).

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef Source
  #define Source "..\target\installer\app"
#endif

[Setup]
AppId={{6F3D2C1E-8B4A-4E7F-9C2D-5A1B3E8F0C47}
AppName=Clipingo
AppVersion={#AppVersion}
AppVerName=Clipingo {#AppVersion}
AppPublisher=Clipingo
AppPublisherURL=https://github.com/raktim-yoddha/clipingo
AppUpdatesURL=https://github.com/raktim-yoddha/clipingo/releases
DefaultDirName={autopf}\Clipingo
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DisableProgramGroupPage=yes
WizardStyle=modern
WizardSizePercent=100
SetupIconFile=..\app\Clipingo\Assets\AppIcon.ico
UninstallDisplayIcon={app}\Clipingo.exe
UninstallDisplayName=Clipingo
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.19041
Compression=lzma2/ultra64
SolidCompression=yes
LZMAUseSeparateProcess=yes
CloseApplications=force
RestartApplications=no
OutputDir=..\target\installer
OutputBaseFilename=Clipingo-Setup-{#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Source}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Clipingo"; Filename: "{app}\Clipingo.exe"
Name: "{autodesktop}\Clipingo"; Filename: "{app}\Clipingo.exe"; Tasks: desktopicon

[Run]
; Interactive installs offer to launch; silent updates always start the app again.
Filename: "{app}\Clipingo.exe"; Description: "{cm:LaunchProgram,Clipingo}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\Clipingo.exe"; Flags: nowait; Check: WizardSilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM clipd.exe"; Flags: runhidden; RunOnceId: "StopClipd"

[Code]
// The daemon outlives a closed window for a moment; stop it so its files can be replaced.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM clipd.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;
