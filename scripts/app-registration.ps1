# Shared current-user installation identity and path validation. Windows PowerShell 5.1+.
Set-StrictMode -Version Latest

$script:WinTranslaterAppId = 'WinTranslater.Desktop'
$script:WinTranslaterManifestName = '.wintranslater-install.json'
$script:WinTranslaterUninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\WinTranslater'
$script:WinTranslaterSupportFiles = @('app-registration.ps1', 'register-app.ps1', 'uninstall.ps1')

function Get-WinTranslaterFullPath([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'An absolute filesystem path is required.' }
    if ($Path -notmatch '^[A-Za-z]:[\\/]') { throw "A local absolute path is required: $Path" }
    $fullPath = [IO.Path]::GetFullPath($Path)
    if ($fullPath -eq [IO.Path]::GetPathRoot($fullPath)) { return $fullPath }
    $fullPath.TrimEnd([char[]]'\/')
}

function Test-WinTranslaterSamePath([string]$Left, [string]$Right) {
    [string]::Equals((Get-WinTranslaterFullPath $Left), (Get-WinTranslaterFullPath $Right), [StringComparison]::OrdinalIgnoreCase)
}

function Test-WinTranslaterChildPath([string]$Path, [string]$Parent) {
    (Get-WinTranslaterFullPath $Path).StartsWith((Get-WinTranslaterFullPath $Parent) + '\', [StringComparison]::OrdinalIgnoreCase)
}

function Assert-WinTranslaterNoReparsePoint([string]$Path) {
    $candidate = Get-WinTranslaterFullPath $Path
    while ($candidate) {
        if (Test-Path -LiteralPath $candidate) {
            $entry = Get-Item -LiteralPath $candidate -Force
            if (($entry.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse points are not supported in installation paths: $candidate" }
        }
        $candidate = [IO.Path]::GetDirectoryName($candidate)
    }
}

function Assert-WinTranslaterInstallDirectory([string]$Path, [switch]$AllowMissing) {
    $directory = Get-WinTranslaterFullPath $Path
    $profileDirectory = Get-WinTranslaterFullPath ([Environment]::GetFolderPath('UserProfile'))
    if ([IO.Path]::GetFileName($directory) -ine 'WinTranslater' -or -not (Test-WinTranslaterChildPath $directory $profileDirectory)) {
        throw 'The installation directory must be a dedicated WinTranslater folder inside the current user profile.'
    }
    Assert-WinTranslaterNoReparsePoint $directory
    if ((Test-Path -LiteralPath $directory) -and -not (Test-Path -LiteralPath $directory -PathType Container)) { throw "The installation path is not a directory: $directory" }
    if (-not $AllowMissing -and -not (Test-Path -LiteralPath $directory -PathType Container)) { throw "The installation directory does not exist: $directory" }
    $directory
}

function Get-WinTranslaterDataDirectories {
    Join-Path ([Environment]::GetFolderPath('LocalApplicationData')) 'TranslationTool'
    if (-not [string]::IsNullOrWhiteSpace($env:LINGUA_DATA_DIR)) { Get-WinTranslaterFullPath $env:LINGUA_DATA_DIR }
}

function Test-WinTranslaterProtectedFile([string]$Path, [string[]]$DataDirectories) {
    if ([IO.Path]::GetFileName($Path) -match '^(settings|history)\.json$|^credentials([.]|$)|^secrets?([.]|$)|^configuration-check\..*\.json$') { return $true }
    foreach ($dataDirectory in $DataDirectories) {
        if ((Test-WinTranslaterSamePath $Path $dataDirectory) -or (Test-WinTranslaterChildPath $Path $dataDirectory)) { return $true }
    }
    $false
}

function Test-WinTranslaterPayloadPath([string]$RelativePath) {
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [IO.Path]::IsPathRooted($RelativePath) -or $RelativePath.Contains(':')) { return $false }
    $segments = @($RelativePath -split '[\\/]')
    if (@($segments | Where-Object {
        $_ -in @('', '.', '..') -or $_.EndsWith('.') -or $_.EndsWith(' ') -or
        $_ -match '^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])([.]|$)' -or
        $_.IndexOfAny([IO.Path]::GetInvalidFileNameChars()) -ge 0
    }).Count -ne 0) { return $false }
    if ($segments.Count -eq 1) { return $segments[0] -match '^(WinTranslater\.exe|README\.md|LICENSE|THIRD_PARTY_NOTICES\.md|[^.][^\\/]*\.(dll|pri|manifest))$' }
    if ($segments[0] -ieq 'scripts') { return $segments.Count -eq 2 -and $segments[1] -in $script:WinTranslaterSupportFiles }
    $segments[0] -in @('assets', 'dictionary', 'licenses', 'models', 'runtime') -or $segments[0] -cmatch '^[a-z]{2}(-[A-Za-z0-9]+)*$'
}

function Get-WinTranslaterPayloadFiles([string]$Directory, [string[]]$DataDirectories = @()) {
    $root = Get-WinTranslaterFullPath $Directory
    Assert-WinTranslaterNoReparsePoint $root
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($root)
    while ($pending.Count -gt 0) {
        $folder = $pending.Pop()
        foreach ($entry in Get-ChildItem -LiteralPath $folder -Force) {
            $relative = $entry.FullName.Substring($root.Length + 1)
            if (Test-WinTranslaterProtectedFile $entry.FullName $DataDirectories) { continue }
            if ($entry.PSIsContainer) {
                # A candidate file below a directory must belong to the payload allowlist.
                if (-not (Test-WinTranslaterPayloadPath (Join-Path $relative 'payload.dll'))) { continue }
                Assert-WinTranslaterNoReparsePoint $entry.FullName
                $pending.Push($entry.FullName)
            } elseif (Test-WinTranslaterPayloadPath $relative) {
                Assert-WinTranslaterNoReparsePoint $entry.FullName
                $relative
            }
        }
    }
}

function Assert-WinTranslaterExecutable([string]$Directory) {
    $executable = Join-Path $Directory 'WinTranslater.exe'
    Assert-WinTranslaterNoReparsePoint $executable
    if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) { throw "WinTranslater.exe is missing: $Directory" }
    $version = [Diagnostics.FileVersionInfo]::GetVersionInfo($executable)
    if ($version.ProductName -ne 'WinTranslater' -or $version.FileDescription -ne 'WinTranslater' -or $version.OriginalFilename -ne 'WinTranslater.exe') {
        throw "The executable does not identify itself as WinTranslater: $executable"
    }
    $version
}

function Read-WinTranslaterInstallManifest([string]$Directory) {
    $root = Assert-WinTranslaterInstallDirectory $Directory
    $manifestPath = Join-Path $root $script:WinTranslaterManifestName
    Assert-WinTranslaterNoReparsePoint $manifestPath
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "The installation manifest is missing; refusing to remove files: $manifestPath" }
    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $installationId = [Guid]::Empty
    if ($manifest.schemaVersion -ne 1 -or $manifest.appId -ne $script:WinTranslaterAppId -or $manifest.productName -ne 'WinTranslater' -or
        $manifest.executable -ne 'WinTranslater.exe' -or -not [Guid]::TryParse([string]$manifest.installationId, [ref]$installationId) -or
        -not (Test-WinTranslaterSamePath $manifest.installDirectory $root)) { throw 'The installation manifest does not match this WinTranslater directory.' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $protectedDirectories = @($manifest.dataDirectories) + @(Get-WinTranslaterDataDirectories)
    foreach ($relative in @($manifest.files)) {
        if (-not (Test-WinTranslaterPayloadPath $relative) -or -not $seen.Add($relative)) { throw "Invalid or duplicate manifest entry: $relative" }
        $target = Get-WinTranslaterFullPath (Join-Path $root $relative)
        if (-not (Test-WinTranslaterChildPath $target $root) -or (Test-WinTranslaterProtectedFile $target $protectedDirectories)) { throw "The manifest includes a protected path: $relative" }
        Assert-WinTranslaterNoReparsePoint $target
        if ((Test-Path -LiteralPath $target) -and -not (Test-Path -LiteralPath $target -PathType Leaf)) { throw "A manifest file is a directory: $relative" }
    }
    foreach ($required in @('WinTranslater.exe', 'scripts\uninstall.ps1', 'scripts\app-registration.ps1')) {
        if (-not $seen.Contains($required)) { throw "The installation manifest is incomplete: $required" }
    }
    $manifest
}

function Assert-WinTranslaterRegistrationOwner([string]$Directory) {
    if (Test-Path -LiteralPath $script:WinTranslaterUninstallKey) {
        $registered = Get-ItemProperty -LiteralPath $script:WinTranslaterUninstallKey
        if (-not (Test-WinTranslaterSamePath $registered.InstallLocation $Directory)) { throw 'Another WinTranslater installation owns the current-user uninstall entry.' }
    }
}

function Stop-WinTranslaterInstalledProcesses([string]$Executable) {
    foreach ($process in @(Get-Process -Name 'WinTranslater' -ErrorAction SilentlyContinue)) {
        try {
            # Retain the process handle before inspecting its image, so PID reuse cannot retarget Kill.
            $null = $process.Handle
            $processPath = $process.Path
        } catch { continue }
        if ($processPath -and (Test-WinTranslaterSamePath $processPath $Executable)) {
            $process.Kill()
            if (-not $process.WaitForExit(5000)) { throw "The installed process did not stop: $($process.Id)" }
        }
    }
}

function Get-WinTranslaterShortcutTarget([string]$ShortcutPath) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $null
    try {
        $shortcut = $shell.CreateShortcut($ShortcutPath)
        $shortcut.TargetPath
    } finally {
        if ($null -ne $shortcut) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
    }
}

function Initialize-WinTranslaterShellInterop {
    if ('WinTranslaterRegistrationShell' -as [type]) { return }
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class WinTranslaterRegistrationShell {
    [StructLayout(LayoutKind.Sequential)] public struct PropertyKey { public Guid FormatId; public uint PropertyId; }
    [StructLayout(LayoutKind.Explicit, Size = 24)] public struct PropVariant {
        [FieldOffset(0)] public ushort Type;
        [FieldOffset(8)] public IntPtr Pointer;
    }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        [PreserveSig] int GetCount(out uint count);
        [PreserveSig] int GetAt(uint index, out PropertyKey key);
        [PreserveSig] int GetValue(ref PropertyKey key, out PropVariant value);
        [PreserveSig] int SetValue(ref PropertyKey key, ref PropVariant value);
        [PreserveSig] int Commit();
    }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SHGetPropertyStoreFromParsingName(string path, IntPtr context, uint flags, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IPropertyStore store);
    [DllImport("ole32.dll")] static extern int PropVariantClear(ref PropVariant value);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode)] static extern void SHChangeNotify(uint eventId, uint flags, string path, IntPtr other);
    static PropertyKey AppIdKey() { return new PropertyKey { FormatId = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), PropertyId = 5 }; }
    static IPropertyStore Open(string path, uint flags) {
        Guid iid = new Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99");
        IPropertyStore store; SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, flags, ref iid, out store); return store;
    }
    public static void SetAppId(string path, string appId) {
        IPropertyStore store = Open(path, 2);
        PropVariant value = new PropVariant { Type = 31, Pointer = Marshal.StringToCoTaskMemUni(appId) };
        try { PropertyKey key = AppIdKey(); Marshal.ThrowExceptionForHR(store.SetValue(ref key, ref value)); Marshal.ThrowExceptionForHR(store.Commit()); }
        finally { PropVariantClear(ref value); Marshal.FinalReleaseComObject(store); }
        SHChangeNotify(0x2000, 0x1005, path, IntPtr.Zero);
    }
    public static string GetAppId(string path) {
        IPropertyStore store = Open(path, 0); PropVariant value = new PropVariant();
        try { PropertyKey key = AppIdKey(); Marshal.ThrowExceptionForHR(store.GetValue(ref key, out value)); return value.Type == 31 ? Marshal.PtrToStringUni(value.Pointer) : null; }
        finally { PropVariantClear(ref value); Marshal.FinalReleaseComObject(store); }
    }
}
'@
}

function Set-WinTranslaterShortcut([string]$ShortcutPath, [string]$Executable, [string]$Directory) {
    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $null
    try {
        $shortcut = $shell.CreateShortcut($ShortcutPath)
        $shortcut.TargetPath = $Executable
        $shortcut.WorkingDirectory = $Directory
        $shortcut.Description = 'WinTranslater'
        $shortcut.IconLocation = $Executable + ',0'
        $shortcut.Save()
    } finally {
        if ($null -ne $shortcut) { [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shortcut) }
        [void][Runtime.InteropServices.Marshal]::FinalReleaseComObject($shell)
    }
    Initialize-WinTranslaterShellInterop
    [WinTranslaterRegistrationShell]::SetAppId($ShortcutPath, $script:WinTranslaterAppId)
}

function Get-WinTranslaterCommandExecutable([string]$Command) {
    if ($Command -match '^\s*"([^"\r\n]+)"(?:\s|$)') { return $matches[1] }
    if ($Command -match '^\s*([^\s"]+)(?:\s|$)') { return $matches[1] }
    $null
}
