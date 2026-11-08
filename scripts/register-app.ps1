[CmdletBinding(SupportsShouldProcess = $true)]
param([string]$InstallDirectory = (Join-Path $env:LOCALAPPDATA 'Programs\WinTranslater'))
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'app-registration.ps1')

$directory = Assert-WinTranslaterInstallDirectory $InstallDirectory
$version = Assert-WinTranslaterExecutable $directory
Assert-WinTranslaterRegistrationOwner $directory
$executable = Join-Path $directory 'WinTranslater.exe'
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'WinTranslater.lnk'
Assert-WinTranslaterNoReparsePoint $shortcutPath
if ((Test-Path -LiteralPath $shortcutPath) -and -not (Test-WinTranslaterSamePath (Get-WinTranslaterShortcutTarget $shortcutPath) $executable)) {
    throw 'An unrelated WinTranslater shortcut already exists in the current-user Start menu.'
}
$manifestPath = Join-Path $directory $script:WinTranslaterManifestName
$previous = if (Test-Path -LiteralPath $manifestPath) { Read-WinTranslaterInstallManifest $directory } else { $null }
$dataDirectories = @(Get-WinTranslaterDataDirectories)
if ($null -ne $previous) { $dataDirectories += @($previous.dataDirectories) }
$files = @(Get-WinTranslaterPayloadFiles $directory $dataDirectories)
foreach ($supportFile in $script:WinTranslaterSupportFiles) {
    $source = Join-Path $PSScriptRoot $supportFile
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing registration support file: $source" }
    Assert-WinTranslaterNoReparsePoint $source
    $files += 'scripts\' + $supportFile
}
$files = @($files | Sort-Object -Unique)
$manifest = [ordered]@{
    schemaVersion = 1
    appId = $script:WinTranslaterAppId
    productName = 'WinTranslater'
    installationId = if ($null -ne $previous) { $previous.installationId } else { [Guid]::NewGuid().ToString() }
    installDirectory = $directory
    executable = 'WinTranslater.exe'
    dataDirectories = @($dataDirectories | Sort-Object -Unique)
    files = $files
}
$powershell = Join-Path $env:WINDIR 'System32\WindowsPowerShell\v1.0\powershell.exe'
$uninstaller = Join-Path $directory 'scripts\uninstall.ps1'
$uninstallCommand = '"{0}" -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "{1}" -InstallDirectory "{2}"' -f $powershell, $uninstaller, $directory
if ($PSCmdlet.ShouldProcess($directory, 'Register WinTranslater in the current-user Start menu and Installed apps')) {
    $supportDirectory = Join-Path $directory 'scripts'
    Assert-WinTranslaterNoReparsePoint $supportDirectory
    New-Item -ItemType Directory -Path $supportDirectory -Force | Out-Null
    foreach ($supportFile in $script:WinTranslaterSupportFiles) {
        $source = Join-Path $PSScriptRoot $supportFile
        $destination = Join-Path $supportDirectory $supportFile
        Assert-WinTranslaterNoReparsePoint $destination
        if (-not (Test-WinTranslaterSamePath $source $destination)) { Copy-Item -LiteralPath $source -Destination $destination -Force }
    }
    $manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($shortcutPath)) -Force | Out-Null
    Set-WinTranslaterShortcut $shortcutPath $executable $directory
    New-Item -Path $script:WinTranslaterUninstallKey -Force | Out-Null
    $strings = @{
        DisplayName = 'WinTranslater'
        DisplayVersion = $version.ProductVersion
        DisplayIcon = '"' + $executable + '",0'
        InstallLocation = $directory
        InstallDate = (Get-Date -Format 'yyyyMMdd')
        UninstallString = $uninstallCommand
        QuietUninstallString = $uninstallCommand + ' -Quiet'
        AppUserModelID = $script:WinTranslaterAppId
    }
    foreach ($name in $strings.Keys) { New-ItemProperty -LiteralPath $script:WinTranslaterUninstallKey -Name $name -Value $strings[$name] -PropertyType String -Force | Out-Null }
    foreach ($name in @('NoModify', 'NoRepair')) { New-ItemProperty -LiteralPath $script:WinTranslaterUninstallKey -Name $name -Value 1 -PropertyType DWord -Force | Out-Null }
}
[pscustomobject]@{ ProductName = 'WinTranslater'; InstallDirectory = $directory; StartMenuShortcut = $shortcutPath; AppUserModelID = $script:WinTranslaterAppId; OwnedFileCount = $files.Count; UninstallKey = $script:WinTranslaterUninstallKey }
