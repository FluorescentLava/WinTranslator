[CmdletBinding()]
param(
    [switch]$CheckOnly,
    [string]$InstallerPath,
    [string]$ConfigPath,
    [string]$ReportPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not $ConfigPath) { $ConfigPath = Join-Path $PSScriptRoot 'runtime.json' }

function Get-CompatibleRuntimePackage($Specification) {
    # Match the same family, minimum version and architecture as bootstrap.rs.
    # Only the current user's registered packages are usable by this application.
    Get-AppxPackage -Name $Specification.package_name -ErrorAction Stop | Where-Object {
        $_.PackageFamilyName -eq $Specification.package_family -and
        $_.IsFramework -and [int]$_.Architecture -eq 9 -and [int]$_.Status -eq 0 -and
        ([version]$_.Version).Major -eq 2 -and
        [version]$_.Version -ge [version]$Specification.minimum_version
    } | Sort-Object { [version]$_.Version } -Descending | Select-Object -First 1
}

function Assert-OfficialRuntimeInstaller([string]$Path, $Specification) {
    $file = Get-Item -LiteralPath $Path -ErrorAction Stop
    if ($file.PSIsContainer -or (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash -ine $Specification.sha256) {
        throw 'Windows App Runtime installer SHA-256 verification failed.'
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $file.FullName
    if ($signature.Status -ne 'Valid' -or -not $signature.SignerCertificate -or
        $signature.SignerCertificate.Subject -notmatch '(^|,\s*)O=Microsoft Corporation(,|$)') {
        throw 'Windows App Runtime installer must have a valid Microsoft signature.'
    }
}

function Invoke-OfficialRuntimeInstaller([string]$Path) {
    # --force is deliberately omitted: it closes other Windows App SDK programs.
    $process = Start-Process -FilePath $Path -ArgumentList '--quiet' -WindowStyle Hidden -Wait -PassThru
    $process.ExitCode
}

function Invoke-RuntimePrerequisite($Specification, [bool]$Probe, [string]$Path) {
    $package = Get-CompatibleRuntimePackage $Specification
    if ($package) {
        return [pscustomobject]@{ exit_code=0; outcome='already_installed'; package_version=[string]$package.Version }
    }
    if ($Probe) { return [pscustomobject]@{ exit_code=10; outcome='missing' } }
    if (-not $Path) { throw 'The official runtime installer path is required.' }
    Assert-OfficialRuntimeInstaller $Path $Specification
    $nativeExitCode = Invoke-OfficialRuntimeInstaller $Path
    if ($nativeExitCode -notin @(0, 3010)) { throw "Windows App Runtime installer failed: $nativeExitCode" }
    $package = Get-CompatibleRuntimePackage $Specification
    if (-not $package) { throw 'A compatible Windows App Runtime is still missing after installation.' }
    [pscustomobject]@{ exit_code=$nativeExitCode; outcome='installed'; package_version=[string]$package.Version }
}

# Dot sourcing loads the real detection and installation functions for isolated tests.
if ($MyInvocation.InvocationName -eq '.') { return }
try {
    $specification = [IO.File]::ReadAllText($ConfigPath) | ConvertFrom-Json
    $report = Invoke-RuntimePrerequisite $specification ([bool]$CheckOnly) $InstallerPath
} catch {
    $report = [pscustomobject]@{ exit_code=20; outcome='failed'; error=$_.Exception.Message }
}
if ($ReportPath) {
    [IO.File]::WriteAllText($ReportPath, (ConvertTo-Json -InputObject $report -Depth 4), [Text.UTF8Encoding]::new($false))
}
exit $report.exit_code
