; Inno Setup script for KeyFlag.
; Builds a single per-user KeyFlag-Setup.exe that needs no admin rights.
; Compile with:  ISCC.exe installer\KeyFlag.iss
; The KeyFlag.exe must already be built at rs\target\release\KeyFlag.exe
; (override with /DBuildDir=... and /DAppVersion=... on the ISCC command line).

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef BuildDir
  #define BuildDir "..\rs\target\release"
#endif

[Setup]
AppId={{D764C8CC-46FF-4027-8715-8C5E52183B41}
AppName=KeyFlag
AppVersion={#AppVersion}
AppPublisher=gabrielchaves6
AppPublisherURL=https://github.com/gabrielchaves6/keyflag
DefaultDirName={autopf}\KeyFlag
DefaultGroupName=KeyFlag
DisableProgramGroupPage=yes
DisableDirPage=auto
; Per-user install: no administrator prompt.
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=dist
OutputBaseFilename=KeyFlag-Setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName=KeyFlag
UninstallDisplayIcon={app}\KeyFlag.exe
SetupIconFile=..\rs\assets\keyflag.ico
SetupLogging=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "brazilianportuguese"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"

[Tasks]
Name: "startup"; Description: "{cm:StartAtLogon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#BuildDir}\KeyFlag.exe"; DestDir: "{app}"; Flags: ignoreversion
; Shipped next to the exe as a runtime icon fallback (and a stable icon for shortcuts).
Source: "..\rs\assets\keyflag.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\KeyFlag"; Filename: "{app}\KeyFlag.exe"; IconFilename: "{app}\keyflag.ico"
Name: "{userstartup}\KeyFlag"; Filename: "{app}\KeyFlag.exe"; IconFilename: "{app}\keyflag.ico"; Tasks: startup

[Run]
; No skipifsilent: the in-app updater runs Setup with /VERYSILENT and relies on this
; entry to relaunch KeyFlag once the new exe is in place (one-click hands-off update).
Filename: "{app}\KeyFlag.exe"; Description: "{cm:LaunchProgram,KeyFlag}"; Flags: nowait postinstall

[CustomMessages]
en.StartAtLogon=Start KeyFlag when I sign in to Windows
brazilianportuguese.StartAtLogon=Iniciar o KeyFlag ao entrar no Windows

[Code]
// A running KeyFlag.exe locks its own file. Kill any instance before
// installing over it and before uninstalling.
procedure KillRunning;
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM KeyFlag.exe /F',
       '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssInstall then
    KillRunning;
end;

function InitializeUninstall(): Boolean;
begin
  KillRunning;
  Result := True;
end;
