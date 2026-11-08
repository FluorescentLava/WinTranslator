[CmdletBinding(SupportsShouldProcess = $true)]
param(
    [string]$InstallDirectory = ([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))),
    [switch]$Quiet
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'app-registration.ps1')
$directory = Assert-WinTranslaterInstallDirectory $InstallDirectory
$manifest = Read-WinTranslaterInstallManifest $directory
$executable = Join-Path $directory 'WinTranslater.exe'
if (Test-Path -LiteralPath $executable -PathType Leaf) { $null = Assert-WinTranslaterExecutable $directory }
$manifestPath = Join-Path $directory $script:WinTranslaterManifestName
$shortcutPath = Join-Path ([Environment]::GetFolderPath('Programs')) 'WinTranslater.lnk'
$removeShortcut = $false
if (Test-Path -LiteralPath $shortcutPath) {
    Assert-WinTranslaterNoReparsePoint $shortcutPath
    $removeShortcut = Test-WinTranslaterSamePath (Get-WinTranslaterShortcutTarget $shortcutPath) $executable
}
$removeRegistration = $false
if (Test-Path -LiteralPath $script:WinTranslaterUninstallKey) {
    $registration = Get-ItemProperty -LiteralPath $script:WinTranslaterUninstallKey
    $removeRegistration = Test-WinTranslaterSamePath $registration.InstallLocation $directory
    if ($removeRegistration -and (Get-Item -LiteralPath $script:WinTranslaterUninstallKey).SubKeyCount -ne 0) { throw 'Unexpected subkeys in the application uninstall entry; refusing recursive registry removal.' }
}
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$startupNames = @()
if (Test-Path -LiteralPath $runKey) {
    $startupKey = Get-Item -LiteralPath $runKey
    foreach ($name in @('TranslationTool', 'WinTranslater')) {
        if ($startupKey.GetValueNames() -contains $name) {
            $startupExecutable = Get-WinTranslaterCommandExecutable ([string]$startupKey.GetValue($name))
            if ($startupExecutable -and (Test-WinTranslaterSamePath $startupExecutable $executable)) { $startupNames += $name }
        }
    }
}
$targets = @($manifest.files | ForEach-Object { Get-WinTranslaterFullPath (Join-Path $directory $_) })
$supportDirectory = Join-Path $directory 'scripts'
$supportTargets = @($targets | Where-Object { Test-WinTranslaterChildPath $_ $supportDirectory })
$payloadTargets = @($targets | Where-Object { -not (Test-WinTranslaterChildPath $_ $supportDirectory) } | Sort-Object @{ Expression = { if (Test-WinTranslaterSamePath $_ $executable) { 0 } else { 1 } } }, { $_ })
if ($PSCmdlet.ShouldProcess($directory, "Uninstall only $($targets.Count) manifest-owned WinTranslater files; preserve settings, history, credentials, and other files")) {
    Stop-WinTranslaterInstalledProcesses $executable
    foreach ($target in $payloadTargets) {
        # Validate again immediately before each deletion. Only individual files are removed.
        if (-not (Test-WinTranslaterChildPath $target $directory)) { throw "A removal target escaped the installation directory: $target" }
        Assert-WinTranslaterNoReparsePoint $target
        if (Test-Path -LiteralPath $target -PathType Leaf) { Remove-Item -LiteralPath $target -Force }
    }
    foreach ($name in $startupNames) {
        $startupExecutable = Get-WinTranslaterCommandExecutable ([string](Get-Item -LiteralPath $runKey).GetValue($name))
        if ($startupExecutable -and (Test-WinTranslaterSamePath $startupExecutable $executable)) { Remove-ItemProperty -LiteralPath $runKey -Name $name }
    }
    if ($removeShortcut -and (Test-Path -LiteralPath $shortcutPath) -and (Test-WinTranslaterSamePath (Get-WinTranslaterShortcutTarget $shortcutPath) $executable)) { Remove-Item -LiteralPath $shortcutPath -Force }
    if ($removeRegistration) {
        # Remove only the exact application key, never the parent Uninstall key.
        $registeredDirectory = (Get-ItemProperty -LiteralPath $script:WinTranslaterUninstallKey).InstallLocation
        if (Test-WinTranslaterSamePath $registeredDirectory $directory) {
            $uninstallEntry = Get-Item -LiteralPath $script:WinTranslaterUninstallKey
            if ($uninstallEntry.SubKeyCount -ne 0) { throw 'Unexpected subkeys in the application uninstall entry; refusing recursive registry removal.' }
            Remove-Item -LiteralPath $script:WinTranslaterUninstallKey
        }
    }
    # Keep the uninstaller available if an earlier file or registration operation fails.
    foreach ($target in $supportTargets) {
        Assert-WinTranslaterNoReparsePoint $target
        if (Test-Path -LiteralPath $target -PathType Leaf) { Remove-Item -LiteralPath $target -Force }
    }
    Remove-Item -LiteralPath $manifestPath -Force
    $folders = @($targets | ForEach-Object {
        $parent = [IO.Path]::GetDirectoryName($_)
        while ($parent -and (Test-WinTranslaterChildPath $parent $directory)) { $parent; $parent = [IO.Path]::GetDirectoryName($parent) }
    } | Sort-Object -Unique | Sort-Object Length -Descending)
    foreach ($folder in $folders + @($directory)) {
        Assert-WinTranslaterNoReparsePoint $folder
        if ((Test-Path -LiteralPath $folder -PathType Container) -and @(Get-ChildItem -LiteralPath $folder -Force).Count -eq 0) { Remove-Item -LiteralPath $folder }
    }
    if (-not $Quiet) { Write-Host 'WinTranslater was uninstalled. User settings, history, and API credentials were preserved.' }
}
if (-not $Quiet) {
    [pscustomobject]@{ ProductName = 'WinTranslater'; InstallDirectory = $directory; OwnedFileCount = $targets.Count; RemoveStartMenuShortcut = $removeShortcut; RemoveUninstallRegistration = $removeRegistration; StartupEntries = $startupNames; PreservedDataDirectories = @($manifest.dataDirectories) }
}
