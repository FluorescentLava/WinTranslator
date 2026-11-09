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
Source: "runtime-prerequisite.ps1"; Flags: dontcopy
Source: "runtime.json"; Flags: dontcopy
Source: "cleanup-runtime.ps1"; Flags: dontcopy

[CustomMessages]
chinesesimplified.RuntimeDownloadTitle=安装 Windows App Runtime
chinesesimplified.RuntimeDownloadDescription=正在下载微软官方运行库，仅在缺少兼容版本时安装。
chinesesimplified.RuntimeCheckFailed=无法检查或安装 Windows App Runtime。请检查网络后重试。
chinesesimplified.RuntimeInstallFailed=Windows App Runtime 安装失败。请重新运行安装程序。
chinesesimplified.CleanupFailed=旧运行库文件清理失败。请退出 WinTranslater 后重新运行安装程序。
english.RuntimeDownloadTitle=Install Windows App Runtime
english.RuntimeDownloadDescription=Downloading the official Microsoft runtime, required only when a compatible version is missing.
english.RuntimeCheckFailed=Unable to check or install Windows App Runtime. Check your connection and retry.
english.RuntimeInstallFailed=Windows App Runtime installation failed. Run this installer again.
english.CleanupFailed=Unable to remove the previous bundled runtime. Exit WinTranslater and run this installer again.

[Icons]
Name: "{autoprograms}\WinTranslater"; Filename: "{app}\WinTranslater.exe"; WorkingDir: "{app}"; AppUserModelID: "WinTranslater.Desktop"

[Run]
Filename: "{app}\WinTranslater.exe"; Description: "{cm:LaunchProgram,WinTranslater}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\WindowsPowerShell\v1.0\powershell.exe"; Parameters: "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File ""{app}\support\stop-app.ps1"" -Executable ""{app}\WinTranslater.exe"""; Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "StopWinTranslater"

[Code]
var
  RuntimeDownloadPage: TDownloadWizardPage;
  PreviousPayloadManifest: String;

procedure InitializeWizard;
begin
  RuntimeDownloadPage := CreateDownloadPage(CustomMessage('RuntimeDownloadTitle'),
    CustomMessage('RuntimeDownloadDescription'), nil);
  RuntimeDownloadPage.ShowBaseNameInsteadOfUrl := True;
end;

function RunPrerequisite(const Arguments: String; var ExitCode: Integer): Boolean;
begin
  Result := Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' +
    ExpandConstant('{tmp}\runtime-prerequisite.ps1') + '" -ConfigPath "' +
    ExpandConstant('{tmp}\runtime.json') + '" -ReportPath "' +
    ExpandConstant('{tmp}\runtime-prerequisite-report.json') + '" ' + Arguments,
    '', SW_HIDE, ewWaitUntilTerminated, ExitCode);
end;

function EnsureWindowsAppRuntime(var NeedsRestart: Boolean): String;
var
  ExitCode: Integer;
begin
  Result := '';
  ExtractTemporaryFile('runtime-prerequisite.ps1');
  ExtractTemporaryFile('runtime.json');
  if not RunPrerequisite('-CheckOnly', ExitCode) then
    Result := CustomMessage('RuntimeCheckFailed')
  else if ExitCode = 10 then
  begin
    Log('No compatible current-user x64 Windows App Runtime is registered.');
    RuntimeDownloadPage.Clear;
    RuntimeDownloadPage.Add('{#RuntimeUrl}', 'WindowsAppRuntimeInstall-x64.exe', '{#RuntimeSHA256}');
    RuntimeDownloadPage.Show;
    try
      try
        RuntimeDownloadPage.Download;
        if not RunPrerequisite('-InstallerPath "' + ExpandConstant('{tmp}\WindowsAppRuntimeInstall-x64.exe') + '"', ExitCode) then
          Result := CustomMessage('RuntimeInstallFailed')
        else if ExitCode = 3010 then
          NeedsRestart := True
        else if ExitCode <> 0 then
          Result := CustomMessage('RuntimeInstallFailed');
      except
        Result := CustomMessage('RuntimeCheckFailed') + #13#10 + GetExceptionMessage;
      end;
    finally
      RuntimeDownloadPage.Hide;
    end;
  end
  else if ExitCode <> 0 then
    Result := CustomMessage('RuntimeCheckFailed')
  else
    Log('Reusing the installed Windows App Runtime; no download or installation is needed.');
end;

// Upgrade and uninstall stop only the executable owned by this installation.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ExitCode: Integer;
begin
  PreviousPayloadManifest := '';
  Result := EnsureWindowsAppRuntime(NeedsRestart);
  if Result <> '' then Exit;
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
  if (Result = '') and FileExists(ExpandConstant('{app}\payload-sha256.json')) then
  begin
    PreviousPayloadManifest := ExpandConstant('{tmp}\previous-payload-sha256.json');
    if not CopyFile(ExpandConstant('{app}\payload-sha256.json'), PreviousPayloadManifest, False) then
      Result := CustomMessage('CleanupFailed');
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ExitCode: Integer;
begin
  // The replacement executable is in place before removing old runtime files.
  if (CurStep = ssPostInstall) and (PreviousPayloadManifest <> '') then
  begin
    ExtractTemporaryFile('cleanup-runtime.ps1');
    if not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' +
      ExpandConstant('{tmp}\cleanup-runtime.ps1') + '" -InstallDirectory "' +
      ExpandConstant('{app}') + '" -ManifestPath "' + PreviousPayloadManifest + '"',
      '', SW_HIDE, ewWaitUntilTerminated, ExitCode) then
      RaiseException(CustomMessage('CleanupFailed'))
    else if ExitCode <> 0 then
      RaiseException(CustomMessage('CleanupFailed'));
    Log('Removed manifest-owned files from the previous bundled WinUI runtime.');
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
