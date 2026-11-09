param([Parameter(Mandatory = $true)][string]$Executable)
$ErrorActionPreference = 'Stop'
$expected = [IO.Path]::GetFullPath($Executable)
if ([IO.Path]::GetFileName($expected) -ine 'WinTranslater.exe') { throw 'Unexpected executable name.' }
foreach ($process in @(Get-Process -Name 'WinTranslater' -ErrorAction SilentlyContinue)) {
    try {
        # Retain the handle before checking the image path to prevent PID reuse.
        $null = $process.Handle
        $image = $process.Path
    } catch { continue }
    if ($image -and [string]::Equals($image, $expected, [StringComparison]::OrdinalIgnoreCase)) {
        $process.Kill()
        if (-not $process.WaitForExit(5000)) { throw 'WinTranslater did not exit.' }
    }
}
