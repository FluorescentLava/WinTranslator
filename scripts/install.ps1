[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$SourceDirectory = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))),
    [string]$InstallDirectory = (Join-Path $env:LOCALAPPDATA 'Programs\WinTranslater')
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'app-registration.ps1')
$source = Get-WinTranslaterFullPath $SourceDirectory
$directory = Assert-WinTranslaterInstallDirectory $InstallDirectory -AllowMissing
$null = Assert-WinTranslaterExecutable $source
Assert-WinTranslaterRegistrationOwner $directory
if (Test-Path -LiteralPath $directory -PathType Container) {
    $manifestPath = Join-Path $directory $script:WinTranslaterManifestName
    if (Test-Path -LiteralPath $manifestPath) {
        $null = Read-WinTranslaterInstallManifest $directory
    } elseif (@(Get-ChildItem -LiteralPath $directory -Force).Count -gt 0) {
        # Adopt an existing manual installation only when its executable has our version identity.
        $null = Assert-WinTranslaterExecutable $directory
    }
}
$files = @(Get-WinTranslaterPayloadFiles $source @(Get-WinTranslaterDataDirectories))
foreach ($relative in $files) { Assert-WinTranslaterNoReparsePoint (Join-Path $directory $relative) }
if ($PSCmdlet.ShouldProcess($directory, "Install or update $($files.Count) WinTranslater application files and register the app")) {
    Stop-WinTranslaterInstalledProcesses (Join-Path $directory 'WinTranslater.exe')
    New-Item -ItemType Directory -Path $directory -Force | Out-Null
    foreach ($relative in $files) {
        $destination = Join-Path $directory $relative
        New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($destination)) -Force | Out-Null
        $sourceFile = Join-Path $source $relative
        if (-not (Test-WinTranslaterSamePath $sourceFile $destination)) { Copy-Item -LiteralPath $sourceFile -Destination $destination -Force }
    }
    & (Join-Path $PSScriptRoot 'register-app.ps1') -InstallDirectory $directory -Confirm:$false
}
