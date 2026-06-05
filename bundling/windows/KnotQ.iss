#define AppPublisher "Enigmadux"
#define AppURL "https://www.knotq.com/"
#define AppExeName "knotq.exe"
#define AppAumid "com.enigmadux.knotq"

#define AppVersion GetEnv("KNOTQ_VERSION")
#if AppVersion == ""
  #undef AppVersion
  #define AppVersion "0.1.0"
#endif

#define SourceRoot GetEnv("KNOTQ_SOURCE_ROOT")
#if SourceRoot == ""
  #undef SourceRoot
  #define SourceRoot "..\.."
#endif

#define Binary GetEnv("KNOTQ_BINARY")
#if Binary == ""
  #undef Binary
  #define Binary SourceRoot + "\target\x86_64-pc-windows-msvc\release\knotq.exe"
#endif

#define OutputDir GetEnv("KNOTQ_OUTPUT_DIR")
#if OutputDir == ""
  #undef OutputDir
  #define OutputDir SourceRoot + "\dist\windows"
#endif

[Setup]
AppId={{D9FDDF31-43D2-49D7-9B04-44D5E72C5E6F}
AppName=KnotQ
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppURL}
AppSupportURL={#AppURL}
AppUpdatesURL={#AppURL}
DefaultDirName={localappdata}\Programs\KnotQ
DefaultGroupName=KnotQ
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=KnotQ-{#AppVersion}-windows-x64-setup
SetupIconFile={#SourceRoot}\desktop\app\assets\app-icon\windows.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=lowest

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Binary}"; DestDir: "{app}"; DestName: "{#AppExeName}"; Flags: ignoreversion
Source: "{#SourceRoot}\desktop\app\assets\*"; DestDir: "{app}\assets"; Excludes: ".DS_Store"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\KnotQ"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\{#AppExeName}"; AppUserModelID: "{#AppAumid}"
Name: "{autodesktop}\KnotQ"; Filename: "{app}\{#AppExeName}"; WorkingDir: "{app}"; IconFilename: "{app}\{#AppExeName}"; Tasks: desktopicon; AppUserModelID: "{#AppAumid}"

[Registry]
Root: HKCU; Subkey: "Software\Classes\knotq"; ValueType: string; ValueName: ""; ValueData: "URL:KnotQ Notification"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\knotq"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\knotq\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExeName},0"
Root: HKCU; Subkey: "Software\Classes\knotq\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExeName}"" ""%1"""

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,KnotQ}"; Flags: nowait postinstall skipifsilent
