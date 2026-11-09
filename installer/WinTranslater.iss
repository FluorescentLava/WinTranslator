#ifndef AppVersion
  #error AppVersion must be supplied by scripts/package-release.ps1
#endif
#ifndef PayloadDir
  #error PayloadDir must be supplied by scripts/package-release.ps1
#endif
#ifndef ReleaseDir
  #error ReleaseDir must be supplied by scripts/package-release.ps1
#endif

[Setup]
AppId=WinTranslater
AppName=WinTranslater
AppVersion={#AppVersion}
AppVerName=WinTranslater {#AppVersion}
AppPublisher=FluorescentLava
AppPublisherURL=https://github.com/FluorescentLava/WinTranslator
AppSupportURL=https://github.com/FluorescentLava/WinTranslator/issues
AppUpdatesURL=https://github.com/FluorescentLava/WinTranslator/releases
DefaultDirName={localappdata}\Programs\WinTranslater
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.22000
WizardStyle=modern
SetupIconFile=..\assets\icons\app.ico
UninstallDisplayIcon={app}\WinTranslater.exe
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoTextVersion={#AppVersion}
VersionInfoProductTextVersion={#AppVersion}
OutputDir={#ReleaseDir}
OutputBaseFilename=WinTranslater-{#AppVersion}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
LZMANumBlockThreads=2
CloseApplications=yes
RestartApplications=no
UninstallLogMode=append

[Languages]
Name: "chinesesimplified"; MessagesFile: "Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
Source: "stop-app.ps1"; DestDir: "{app}\support"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\WinTranslater"; Filename: "{app}\WinTranslater.exe"; WorkingDir: "{app}"; AppUserModelID: "WinTranslater.Desktop"

[Run]
Filename: "{app}\WinTranslater.exe"; Description: "{cm:LaunchProgram,WinTranslater}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ""{app}\support\stop-app.ps1"" -Executable ""{app}\WinTranslater.exe"""; Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "StopWinTranslater"

[Code]
// Upgrade and uninstall stop only the executable owned by this installation.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExitCode: Integer;
begin
  Result := '';
  if FileExists(ExpandConstant('{app}\WinTranslater.exe')) then
  begin
    ExtractTemporaryFile('stop-app.ps1');
    if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' +
      ExpandConstant('{tmp}\stop-app.ps1') + '" -Executable "' +
      ExpandConstant('{app}\WinTranslater.exe') + '"', '', SW_HIDE,
      ewWaitUntilTerminated, ExitCode) then
      Result := 'Unable to stop the installed WinTranslater.'
    else if ExitCode <> 0 then
      Result := 'Please exit WinTranslater and retry.';
  end;
end;

procedure RemoveOwnedStartupValue(const Name: String);
var
  Command, Executable: String;
begin
  Executable := ExpandConstant('{app}\WinTranslater.exe');
  if RegQueryStringValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', Name, Command) then
    if (CompareText(Command, '"' + Executable + '" --background') = 0) or
       (CompareText(Command, Executable + ' --background') = 0) then
      RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', Name);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
  begin
    RemoveOwnedStartupValue('TranslationTool');
    RemoveOwnedStartupValue('WinTranslater');
  end;
  // User settings, DPAPI credentials, history and usage are never installer files.
end;
