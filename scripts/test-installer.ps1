[CmdletBinding()]
param([string]$SignedRuntimeInstaller)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$project = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$specification = [IO.File]::ReadAllText((Join-Path $project 'installer\runtime.json')) | ConvertFrom-Json
. (Join-Path $project 'installer\runtime-prerequisite.ps1')
$passed = 0
function Assert-InstallerTest([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
    $script:passed++
}
function Assert-Throws([scriptblock]$Action) {
    $failed = $false
    try { & $Action } catch { $failed = $true }
    Assert-InstallerTest $failed 'Expected installer operation to fail.'
}
if ($SignedRuntimeInstaller) {
    Assert-OfficialRuntimeInstaller $SignedRuntimeInstaller $specification
    $passed++
}
function Get-AppxPackage {
    [CmdletBinding()]param([string]$Name)
    Assert-InstallerTest ($Name -eq 'Microsoft.WindowsAppRuntime.2') 'Wrong runtime package family requested.'
    $script:packages
}
function New-Package([string]$Version='2.6.5.0', [int]$Architecture=9, [int]$Status=0) {
    [pscustomobject]@{ Version=$Version; Architecture=$Architecture; Status=$Status; IsFramework=$true; PackageFamilyName='Microsoft.WindowsAppRuntime.2_8wekyb3d8bbwe' }
}
$script:packages = @(New-Package)
$result = Invoke-RuntimePrerequisite $specification $false 'nonexistent.exe'
Assert-InstallerTest ($result.exit_code -eq 0 -and $result.outcome -eq 'already_installed') 'An existing runtime must bypass downloads and installation.'
foreach ($invalid in @((New-Package '2.3.1.0'), (New-Package '2.6.5.0' 0), (New-Package '2.6.5.0' 9 1))) {
    $script:packages = @($invalid)
    Assert-InstallerTest ((Invoke-RuntimePrerequisite $specification $true '').exit_code -eq 10) 'Incompatible version, architecture or damaged package was accepted.'
}
$script:packages = @((New-Package '2.4.0.0'), (New-Package '2.6.5.0'))
Assert-InstallerTest ((Get-CompatibleRuntimePackage $specification).Version -eq '2.6.5.0') 'The newest compatible registered runtime must be selected.'
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ('wintranslater-installer-tests-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
try {
    $fakeInstaller = Join-Path $testRoot 'fake-installer.exe'
    [IO.File]::WriteAllText($fakeInstaller, 'fixture; never executed')
    $fixtureSpec = $specification | ConvertTo-Json | ConvertFrom-Json
    $fixtureSpec.sha256 = (Get-FileHash -LiteralPath $fakeInstaller -Algorithm SHA256).Hash
    function Get-AuthenticodeSignature {
        param([string]$LiteralPath)
        [pscustomobject]@{ Status=$script:signatureStatus; SignerCertificate=[pscustomobject]@{ Subject='CN=Microsoft Corporation, O=Microsoft Corporation, C=US' } }
    }
    $script:signatureStatus = 'NotSigned'
    Assert-Throws { Assert-OfficialRuntimeInstaller $fakeInstaller $fixtureSpec }
    $script:signatureStatus = 'Valid'
    Assert-Throws { Assert-OfficialRuntimeInstaller $fakeInstaller $specification }
    $script:installCalls = 0
    function Invoke-OfficialRuntimeInstaller {
        param([string]$Path)
        $script:installCalls++
        if ($script:registerAfterInstall) { $script:packages = @(New-Package) }
        $script:nativeExitCode
    }
    $script:registerAfterInstall=$true
    $script:nativeExitCode=0
    $script:packages=@()
    $result=Invoke-RuntimePrerequisite $fixtureSpec $false $fakeInstaller
    Assert-InstallerTest ($result.outcome -eq 'installed' -and $script:installCalls -eq 1) 'A missing runtime was not installed and rechecked.'
    $script:packages=@()
    $script:registerAfterInstall=$false
    Assert-Throws { Invoke-RuntimePrerequisite $fixtureSpec $false $fakeInstaller }
    $script:nativeExitCode=42
    Assert-Throws { Invoke-RuntimePrerequisite $fixtureSpec $false $fakeInstaller }

    $fixture = Join-Path $testRoot 'WinTranslater'
    New-Item -ItemType Directory -Path (Join-Path $fixture 'de-DE') -Force | Out-Null
    foreach ($relative in @('WinTranslater.exe','Microsoft.ui.xaml.dll','de-DE\Microsoft.ui.xaml.dll.mui','de-DE\personal.txt','settings.json','unlisted.dll')) {
        [IO.File]::WriteAllText((Join-Path $fixture $relative), $relative)
    }
    $owned = @('Microsoft.ui.xaml.dll','de-DE\Microsoft.ui.xaml.dll.mui')
    $oldManifest = @($owned | ForEach-Object { [pscustomobject]@{ path=$_; sha256=(Get-FileHash -LiteralPath (Join-Path $fixture $_) -Algorithm SHA256).Hash } })
    $manifestPath = Join-Path $testRoot 'old-payload.json'
    [IO.File]::WriteAllText($manifestPath, (ConvertTo-Json -InputObject $oldManifest))
    $cleanup = Join-Path $project 'installer\cleanup-runtime.ps1'
    [IO.File]::AppendAllText((Join-Path $fixture $owned[1]), 'modified')
    Assert-Throws { & $cleanup -InstallDirectory $fixture -ManifestPath $manifestPath }
    Assert-InstallerTest (Test-Path -LiteralPath (Join-Path $fixture $owned[0])) 'Cleanup removed files before validating every hash.'
    [IO.File]::WriteAllText((Join-Path $fixture $owned[1]), $owned[1])
    & $cleanup -InstallDirectory $fixture -ManifestPath $manifestPath
    foreach ($relative in $owned) {
        Assert-InstallerTest (-not (Test-Path -LiteralPath (Join-Path $fixture $relative))) 'Owned WinUI runtime file was not removed.'
    }
    foreach ($relative in @('WinTranslater.exe','de-DE\personal.txt','settings.json','unlisted.dll')) {
        Assert-InstallerTest (Test-Path -LiteralPath (Join-Path $fixture $relative)) 'Cleanup touched an application or unlisted file.'
    }
    Write-Output "Installer checks passed: $passed"
} finally {
    $resolvedTestRoot = [IO.Path]::GetFullPath($testRoot)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')+'\'
    if (-not $resolvedTestRoot.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or [IO.Path]::GetFileName($resolvedTestRoot) -notmatch '^wintranslater-installer-tests-[a-f0-9]{32}$') { throw 'Invalid test cleanup directory.' }
    Remove-Item -LiteralPath $resolvedTestRoot -Recurse -Force
}
