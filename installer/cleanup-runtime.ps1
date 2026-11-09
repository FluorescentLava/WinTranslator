[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InstallDirectory,
    [Parameter(Mandatory = $true)][string]$ManifestPath
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$directory = [IO.Path]::GetFullPath($InstallDirectory).TrimEnd('\')
if ([IO.Path]::GetFileName($directory) -ine 'WinTranslater' -or
    -not (Test-Path -LiteralPath (Join-Path $directory 'WinTranslater.exe') -PathType Leaf)) {
    throw 'A dedicated existing WinTranslater installation is required.'
}
$runtimeNames = @(
    'CoreMessagingXP.dll','dcompi.dll','dwmcorei.dll','DwmSceneI.dll','DWriteCore.dll','marshal.dll',
    'Microsoft.DirectManipulation.dll','Microsoft.Graphics.Imaging.dll','Microsoft.InputStateManager.dll',
    'Microsoft.Internal.FrameworkUdk.dll','Microsoft.UI.Composition.OSSupport.dll','Microsoft.UI.dll',
    'Microsoft.UI.Input.dll','Microsoft.UI.pri','Microsoft.UI.Windowing.Core.dll','Microsoft.UI.Windowing.dll',
    'Microsoft.UI.Xaml.Controls.dll','Microsoft.UI.Xaml.Controls.pri','Microsoft.ui.xaml.dll',
    'Microsoft.UI.Xaml.Internal.dll','Microsoft.UI.Xaml.Phone.dll','Microsoft.ui.xaml.resources.19h1.dll',
    'Microsoft.ui.xaml.resources.common.dll','Microsoft.Web.WebView2.Core.dll',
    'Microsoft.Windows.ApplicationModel.Resources.dll','Microsoft.WindowsAppRuntime.dll',
    'Microsoft.WindowsAppRuntime.pri','MRM.dll','resources.pri','SessionHandleIPCProxyStub.dll',
    'WinUIEdit.dll','wuceffectsi.dll'
)
function Assert-PlainPath([string]$Path) {
    $candidate = $Path
    while ($candidate -and $candidate.Length -ge $directory.Length) {
        if ((Test-Path -LiteralPath $candidate) -and
            ((Get-Item -LiteralPath $candidate -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Runtime cleanup does not follow reparse points.'
        }
        $candidate = [IO.Path]::GetDirectoryName($candidate)
    }
}
$manifest = [IO.File]::ReadAllText($ManifestPath) | ConvertFrom-Json
$targets = @()
foreach ($entry in $manifest) {
    $relative = [string]$entry.path
    $allowed = $relative -in $runtimeNames -or
        $relative -cmatch '^[a-z]{2,3}(-[A-Za-z0-9]+)+\\Microsoft\.(ui\.xaml|UI\.Xaml\.Phone)\.dll\.mui$' -or
        $relative -in @('Microsoft.UI.Xaml\Assets\map.html','Microsoft.UI.Xaml\Assets\NoiseAsset_256x256_PNG.png')
    if (-not $allowed) { continue }
    $target = [IO.Path]::GetFullPath((Join-Path $directory $relative))
    if (-not $target.StartsWith($directory+'\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Runtime cleanup path escaped installation.' }
    Assert-PlainPath $target
    if (Test-Path -LiteralPath $target -PathType Leaf) {
        if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ine $entry.sha256) {
            throw "An old runtime file has been modified: $relative"
        }
        $targets += $target
    }
}
# Validate every owned file before removing any. User data and unlisted files are untouched.
foreach ($target in $targets) {
    Remove-Item -LiteralPath $target -Force
    $parent = [IO.Path]::GetDirectoryName($target)
    while ($parent -and $parent.StartsWith($directory+'\', [StringComparison]::OrdinalIgnoreCase)) {
        Assert-PlainPath $parent
        if (@(Get-ChildItem -LiteralPath $parent -Force).Count -ne 0) { break }
        Remove-Item -LiteralPath $parent
        $parent = [IO.Path]::GetDirectoryName($parent)
    }
}
Write-Output "Removed $($targets.Count) manifest-owned runtime files."
