param([switch]$SelfContained)
$ErrorActionPreference = 'Stop'
$project = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
Push-Location $project
try {
    if (!(Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Rust 1.95+ is required. Install the stable MSVC toolchain from https://rustup.rs/ and restart the terminal.' }
    $arguments = @('build', '--release', '--locked')
    if ($SelfContained) { $arguments += @('--features', 'portable') }
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Cargo build failed.' }
    $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $project 'target' }
    Copy-Item -LiteralPath (Join-Path $target 'release\translator.exe') -Destination (Join-Path $project 'WinTranslater.exe') -Force
    if ($SelfContained) {
        # Setup stages the complete Windows App SDK runtime next to the release binary.
        Get-ChildItem -LiteralPath (Join-Path $target 'release') | Where-Object { $_.Extension -in @('.dll','.pri','.manifest') -or $_.Name -match '^[a-z]{2}(-[A-Za-z]+)?$' } |
            Copy-Item -Destination $project -Recurse -Force
    }
    Write-Host "Ready: $project\WinTranslater.exe"
} finally { Pop-Location }
