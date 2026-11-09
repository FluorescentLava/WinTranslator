[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$IsccPath,
    [string]$PayloadPath,
    [string]$ReportDirectory,
    [switch]$KeepArtifacts
)
# Windows PowerShell 5.1+. Run the real Inno extraction and Finish actions in
# isolated fixtures. Pass a staging folder from package-release.ps1 -KeepStage
# as PayloadPath to include the large-payload extraction regression check.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$project = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$IsccPath = (Get-Item -LiteralPath $IsccPath -ErrorAction Stop).FullName
if ($PayloadPath) { $PayloadPath = (Get-Item -LiteralPath $PayloadPath -ErrorAction Stop).FullName }
$identity = [Guid]::NewGuid().ToString('N')
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('wintranslater-installer-responsiveness-' + $identity)
$finishIdentity = 'WinTranslater finish ' + $identity
$extractionIdentity = 'WinTranslater extraction ' + $identity
$installerSourceDirectory = Join-Path $project 'installer'
$installerSourcePath = Join-Path $installerSourceDirectory 'WinTranslater.iss'
$installerSource = [IO.File]::ReadAllText($installerSourcePath)
$utf8Bom = [Text.UTF8Encoding]::new($true)
$utf8 = [Text.UTF8Encoding]::new($false)
$report = [ordered]@{ success = $false; source_sha256 = (Get-FileHash -LiteralPath $installerSourcePath -Algorithm SHA256).Hash.ToLowerInvariant() }
$completed = $false
$childExe = Join-Path $testRoot 'finish\payload\WinTranslater.exe'
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

if (-not ('WinTranslaterInstallerProbeDesktop' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
public static class WinTranslaterInstallerProbeDesktop {
    [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] struct StartupInfo {
        public int Size; public string Reserved, Desktop, Title;
        public int X, Y, XSize, YSize, XCountChars, YCountChars, FillAttribute, Flags;
        public short ShowWindow, ReservedSize; public IntPtr ReservedPointer, StdInput, StdOutput, StdError;
    }
    [StructLayout(LayoutKind.Sequential)] struct ProcessInfo { public IntPtr Process, Thread; public int ProcessId, ThreadId; }
    [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern IntPtr CreateDesktop(string name, IntPtr device, IntPtr mode, int flags, uint access, IntPtr attributes);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr desktop);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] static extern bool CreateProcess(string app, StringBuilder command, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint flags, IntPtr environment, string directory, ref StartupInfo startup, out ProcessInfo process);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle, uint timeout);
    [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr process, out uint code);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
    public static uint Run(string executable, string arguments, int timeout) {
        string name = "WinTranslaterProbe" + Guid.NewGuid().ToString("N");
        IntPtr desktop = CreateDesktop(name, IntPtr.Zero, IntPtr.Zero, 0, 0x000F01FF, IntPtr.Zero);
        if (desktop == IntPtr.Zero) throw new Win32Exception();
        ProcessInfo process = new ProcessInfo();
        try {
            StartupInfo startup = new StartupInfo(); startup.Size = Marshal.SizeOf(startup); startup.Desktop = name;
            startup.Flags = 1; startup.ShowWindow = 0;
            if (!CreateProcess(executable, new StringBuilder("\"" + executable + "\" " + arguments), IntPtr.Zero, IntPtr.Zero, false, 0, IntPtr.Zero, System.IO.Path.GetDirectoryName(executable), ref startup, out process)) throw new Win32Exception();
            if (WaitForSingleObject(process.Process, (uint)timeout) != 0) {
                TerminateProcess(process.Process, 99);
                throw new Exception("The owned private-desktop probe exceeded " + timeout + " ms.");
            }
            uint code; if (!GetExitCodeProcess(process.Process, out code)) throw new Win32Exception(); return code;
        } finally {
            if (process.Thread != IntPtr.Zero) CloseHandle(process.Thread);
            if (process.Process != IntPtr.Zero) CloseHandle(process.Process);
            CloseDesktop(desktop);
        }
    }
}
'@
}

function Compile-Probe([string[]]$Arguments, [string]$LogPath) {
    & $IsccPath '/Q' @Arguments 2>&1 | Out-File -LiteralPath $LogPath -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw "Inno probe compilation failed. See $LogPath" }
}

function Invoke-Probe([string]$Path, [string[]]$Arguments, [int]$TimeoutMilliseconds) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    # Never switch to this desktop. The installer and every child window stay
    # outside the user's desktop, so the test cannot interrupt keyboard focus.
    $exitCode = [WinTranslaterInstallerProbeDesktop]::Run($Path, ($Arguments -join ' '), $TimeoutMilliseconds)
    $watch.Stop()
    [pscustomobject]@{ exit_code = $exitCode; elapsed_seconds = $watch.Elapsed.TotalSeconds }
}

function Stop-OwnedProbeProcesses {
    foreach ($process in @(Get-Process -Name 'WinTranslater*' -ErrorAction SilentlyContinue)) {
        try {
            $null = $process.Handle
            $path = $process.Path
            $owned = [string]::Equals($path, $childExe, [StringComparison]::OrdinalIgnoreCase)
            if (-not $owned) {
                $product = ([string][Diagnostics.FileVersionInfo]::GetVersionInfo($path).ProductName).Trim()
                $owned = $product -eq $finishIdentity -or $product -eq $extractionIdentity
            }
        } catch { continue }
        if ($owned) {
            $process.Kill()
            $null = $process.WaitForExit(5000)
        }
    }
}

try {
    if ($PayloadPath) {
        $extraction = Join-Path $testRoot 'extraction'
        New-Item -ItemType Directory -Path $extraction -Force | Out-Null
        $source = $installerSource.Replace('[Setup]', "[Setup]`r`nSourceDir=$installerSourceDirectory")
        $source = [regex]::Replace($source, '(?m)^AppName=.*$', "AppName=$extractionIdentity")
        $source += @'

function ExtractionProbeTicks: LongWord;
  external 'GetTickCount@kernel32.dll stdcall';

// Returning False prevents the wizard, file installation, and registration.
function InitializeSetup: Boolean;
var
  Started: LongWord;
begin
  Started := ExtractionProbeTicks;
  ExtractTemporaryFile('runtime-prerequisite.ps1');
  Log(Format('EXTRACTION runtime-prerequisite.ps1: %d ms', [ExtractionProbeTicks - Started]));
  Started := ExtractionProbeTicks;
  ExtractTemporaryFile('runtime.json');
  Log(Format('EXTRACTION runtime.json: %d ms', [ExtractionProbeTicks - Started]));
  Started := ExtractionProbeTicks;
  ExtractTemporaryFile('stop-app.ps1');
  Log(Format('EXTRACTION stop-app.ps1: %d ms', [ExtractionProbeTicks - Started]));
  Started := ExtractionProbeTicks;
  ExtractTemporaryFile('cleanup-runtime.ps1');
  Log(Format('EXTRACTION cleanup-runtime.ps1: %d ms', [ExtractionProbeTicks - Started]));
  Log('EXTRACTION probe complete; installation deliberately aborted.');
  Result := False;
end;
'@
        $probeSource = Join-Path $extraction 'extraction-probe.iss'
        [IO.File]::WriteAllText($probeSource, $source, $utf8Bom)
        $runtime = [IO.File]::ReadAllText((Join-Path $installerSourceDirectory 'runtime.json')) | ConvertFrom-Json
        $version = [regex]::Match([IO.File]::ReadAllText((Join-Path $project 'Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
        if (-not $version) { throw 'The Cargo version is missing.' }
        $numericVersion = ($version -split '-')[0] + '.0'
        Compile-Probe @("/DAppVersion=$version", "/DNumericVersion=$numericVersion", "/DPayloadDir=$PayloadPath", "/DReleaseDir=$extraction", "/DRuntimeUrl=$($runtime.url)", "/DRuntimeSHA256=$($runtime.sha256)", $probeSource) (Join-Path $extraction 'compile.log')
        $log = Join-Path $extraction 'extraction.log'
        $run = Invoke-Probe (Join-Path $extraction "WinTranslater-$version-windows-x64-setup.exe") @('/SP-', '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', ('/LOG="' + $log + '"')) 60000
        $lines = [IO.File]::ReadAllLines($log)
        if (-not @($lines | Where-Object { $_ -match 'EXTRACTION probe complete' }).Count) { throw 'The extraction probe did not finish.' }
        $measurements = @($lines | ForEach-Object {
            if ($_ -match 'EXTRACTION ([^:]+): (\d+) ms') { [pscustomobject]@{ file = $matches[1]; milliseconds = [int]$matches[2] } }
        })
        if ($measurements.Count -ne 4 -or @($measurements | Where-Object milliseconds -GT 1000).Count) { throw 'A small installer helper took more than one second to extract. Check solid-compression order.' }
        $report.extraction = [ordered]@{ installation_aborted = $true; elapsed_seconds = $run.elapsed_seconds; helpers = $measurements }
    }

    $finish = Join-Path $testRoot 'finish'
    $payload = Join-Path $finish 'payload'
    New-Item -ItemType Directory -Path $payload -Force | Out-Null
    $childSource = @'
using System;
using System.IO;
using System.Threading;
public static class SlowStartupFixture {
    public static void Main() {
        string root = AppDomain.CurrentDomain.BaseDirectory;
        string marker = Path.Combine(root, "child-started.tmp");
        File.WriteAllText(marker, System.Diagnostics.Process.GetCurrentProcess().Id.ToString());
        File.Move(marker, Path.Combine(root, "child-started.txt"));
        Thread.Sleep(20000); // No message loop or input-idle state during startup.
        File.WriteAllText(Path.Combine(root, "child-finished.txt"), "done");
    }
}
'@
    Add-Type -OutputAssembly $childExe -OutputType WindowsApplication -TypeDefinition ($childSource.Replace('SlowStartupFixture', ('SlowStartupFixture_' + $identity)))
    $runAction = [regex]::Match($installerSource, '(?s)\[Run\]\r?\n(.*?)(?=\r?\n\[UninstallRun\])').Groups[1].Value.Trim()
    $callbacks = [regex]::Match($installerSource, '(?s)procedure BeforeLaunchApplication;.*?(?=procedure RemoveOwnedStartupValue)').Value
    if (-not $runAction -or -not $callbacks) { throw 'The production launch action or callbacks were not found.' }
    $source = @"
[Setup]
AppId=WinTranslaterIsolatedFinishProbe-$identity
AppName=$finishIdentity
AppVersion=1.0
DefaultDirName=$payload
PrivilegesRequired=lowest
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableWelcomePage=yes
DisableReadyPage=yes
CreateUninstallRegKey=no
Uninstallable=no
OutputDir=$finish
OutputBaseFilename=WinTranslater-Finish-Probe
WizardStyle=modern

[Run]
$runAction

[Code]
$callbacks
function ProbePostMessage(Window: HWND; Msg, WParam, LParam: LongWord): Boolean;
  external 'PostMessageW@user32.dll stdcall';

function ProbeShowWindow(Window: HWND; Command: Integer): Boolean;
  external 'ShowWindow@user32.dll stdcall';

procedure InitializeWizard;
begin
  WizardForm.Left := -32000;
  WizardForm.Top := -32000;
  WizardForm.Visible := False;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  ProbeShowWindow(WizardForm.Handle, 0);
  if (CurPageID = wpWelcome) or (CurPageID = wpReady) then
    ProbePostMessage(WizardForm.NextButton.Handle, `$00F5, 0, 0);
  if CurPageID = wpFinished then
  begin
    Log(Format('FINISH button enabled=%d', [Ord(WizardForm.NextButton.Enabled)]));
    // Only this fixture's own hidden button receives a click; no global input.
    ProbePostMessage(WizardForm.NextButton.Handle, `$00F5, 0, 0);
  end;
end;
"@
    $probeSource = Join-Path $finish 'finish-probe.iss'
    [IO.File]::WriteAllText($probeSource, $source, $utf8Bom)
    Compile-Probe @($probeSource) (Join-Path $finish 'compile.log')
    $log = Join-Path $finish 'finish.log'
    $run = Invoke-Probe (Join-Path $finish 'WinTranslater-Finish-Probe.exe') @('/SP-', '/SUPPRESSMSGBOXES', '/NORESTART', ('/LOG="' + $log + '"')) 10000
    $started = Join-Path $payload 'child-started.txt'
    $deadline = [DateTime]::UtcNow.AddSeconds(3)
    while (-not (Test-Path -LiteralPath $started) -and [DateTime]::UtcNow -lt $deadline) { Start-Sleep -Milliseconds 50 }
    if (-not (Test-Path -LiteralPath $started)) { throw 'Finish did not start the fixture application.' }
    $child = Get-Process -Id ([int][IO.File]::ReadAllText($started)) -ErrorAction SilentlyContinue
    $logText = [IO.File]::ReadAllText($log)
    if ($run.exit_code -ne 0 -or -not $child -or
        -not [string]::Equals($child.Path, $childExe, [StringComparison]::OrdinalIgnoreCase) -or
        (Test-Path -LiteralPath (Join-Path $payload 'child-finished.txt')) -or
        -not $logText.Contains('FINISH button enabled=1') -or
        -not $logText.Contains('WinTranslater launch returned; closing Setup.') -or $run.elapsed_seconds -ge 5) {
        throw 'Finish blocked on application startup, or its launch action failed.'
    }
    $report.finish = [ordered]@{ setup_exit_code = $run.exit_code; setup_seconds = $run.elapsed_seconds; child_startup_seconds = 20; child_still_running = $true; finish_button_enabled = $true; private_unseen_desktop = $true }
    $report.success = $true
    $completed = $true
    if ($KeepArtifacts) { $report.artifact_directory = $testRoot }
    if ($ReportDirectory) {
        $ReportDirectory = [IO.Path]::GetFullPath($ReportDirectory)
        New-Item -ItemType Directory -Path $ReportDirectory -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $finish 'finish.log') -Destination (Join-Path $ReportDirectory 'finish.log') -Force
        if ($PayloadPath) { Copy-Item -LiteralPath (Join-Path $testRoot 'extraction\extraction.log') -Destination (Join-Path $ReportDirectory 'extraction.log') -Force }
        [IO.File]::WriteAllText((Join-Path $ReportDirectory 'result.json'), (ConvertTo-Json -InputObject $report -Depth 5), $utf8)
    }
    $report | ConvertTo-Json -Depth 5
} finally {
    Stop-OwnedProbeProcesses
    if (-not $KeepArtifacts -and $completed) {
        $resolved = [IO.Path]::GetFullPath($testRoot)
        $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolved) -notmatch '^wintranslater-installer-responsiveness-[a-f0-9]{32}$') { throw 'Invalid probe cleanup directory.' }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    } elseif (-not $completed) {
        Write-Warning "Retained failed probe artifacts: $testRoot"
    }
}
