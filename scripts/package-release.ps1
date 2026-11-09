[CmdletBinding()]
param(
    [string]$IsccPath,
    [string]$TargetDirectory = $env:CARGO_TARGET_DIR,
    [string]$VcRuntimeDirectory,
    [string]$OutputDirectory,
    [switch]$SkipBuild,
    [switch]$KeepStage
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$project = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$utf8 = [Text.UTF8Encoding]::new($false)
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $project 'dist' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $project 'target' }
$TargetDirectory = [IO.Path]::GetFullPath($TargetDirectory)
$release = Join-Path $TargetDirectory 'release'
$versionMatch = [regex]::Match([IO.File]::ReadAllText((Join-Path $project 'Cargo.toml')), '(?m)^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) { throw 'Cargo package version is missing.' }
$version = $versionMatch.Groups[1].Value
if ($version -notmatch '^\d+\.\d+\.\d+(-[A-Za-z0-9.-]+)?$') { throw 'Unsupported release version.' }
$numericVersion = ($version -split '-')[0] + '.0'

if (-not $IsccPath) {
    $command = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($command) { $IsccPath = $command.Source }
    foreach ($root in @(${env:ProgramFiles(x86)}, $env:ProgramFiles, $env:LOCALAPPDATA)) {
        if ($IsccPath -or -not $root) { continue }
        foreach ($relative in @('Inno Setup 6\ISCC.exe', 'Programs\Inno Setup 6\ISCC.exe')) {
            $candidate = Join-Path $root $relative
            if (Test-Path -LiteralPath $candidate -PathType Leaf) { $IsccPath = $candidate; break }
        }
    }
}
if (-not $IsccPath -or -not (Test-Path -LiteralPath $IsccPath -PathType Leaf)) {
    throw 'Install Inno Setup 6.5+ from https://jrsoftware.org/isdl.php or pass -IsccPath.'
}
if (-not $SkipBuild) {
    Push-Location $project
    try {
        & cargo build --release --locked --features portable --target-dir $TargetDirectory
        if ($LASTEXITCODE -ne 0) { throw 'Self-contained Cargo build failed.' }
    } finally { Pop-Location }
}
$binary = Join-Path $release 'translator.exe'
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Build output missing: $binary" }
$identity = [Diagnostics.FileVersionInfo]::GetVersionInfo($binary)
if ($identity.ProductName -ne 'WinTranslater' -or $identity.ProductVersion -ne $version) {
    throw 'Executable version does not match Cargo.toml. Rebuild before packaging.'
}
if (-not [Text.Encoding]::UTF8.GetString([IO.File]::ReadAllBytes($binary)).Contains('windows-reactor-self-contained')) {
    throw 'The executable was not built with --features portable.'
}

# App-local Microsoft CRT avoids requiring an administrator or a separate VC++ installer.
if (-not $VcRuntimeDirectory) {
    $redistRoots = @()
    if ($env:VCToolsRedistDir) { $redistRoots += $env:VCToolsRedistDir }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (Test-Path -LiteralPath $vswhere) {
        $visualStudio = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
        if ($visualStudio) {
            $redist = Join-Path $visualStudio 'VC\Redist\MSVC'
            if (Test-Path -LiteralPath $redist) {
                $redistRoots += @(Get-ChildItem -LiteralPath $redist -Directory | Sort-Object Name -Descending | Select-Object -ExpandProperty FullName)
            }
        }
    }
    foreach ($root in $redistRoots) {
        $candidate = Join-Path $root 'x64\Microsoft.VC143.CRT'
        if (Test-Path -LiteralPath $candidate -PathType Container) { $VcRuntimeDirectory = $candidate; break }
    }
}
if (-not $VcRuntimeDirectory) { throw 'Visual C++ x64 redistributable DLL directory missing; pass -VcRuntimeDirectory.' }

New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
$stagingRoot = Join-Path $OutputDirectory 'staging'
$stage = Join-Path $stagingRoot ([Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage -Force | Out-Null
function Copy-PayloadFile([string]$Source, [string]$Relative) {
    if (-not (Test-Path -LiteralPath $Source -PathType Leaf)) { throw "Required payload file missing: $Source" }
    $destination = Join-Path $stage $Relative
    New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($destination)) -Force | Out-Null
    Copy-Item -LiteralPath $Source -Destination $destination -Force
}
try {
    Copy-PayloadFile $binary 'WinTranslater.exe'
    # Explicit build/resource allowlist: never copy an installed folder or user profile.
    Get-ChildItem -LiteralPath $release -File | Where-Object { $_.Extension -in @('.dll', '.pri', '.manifest') } |
        ForEach-Object { Copy-PayloadFile $_.FullName $_.Name }
    Get-ChildItem -LiteralPath $release -Directory | Where-Object { $_.Name -eq 'Microsoft.UI.Xaml' -or $_.Name -match '^[a-z]{2,3}(-[A-Za-z0-9]+)+$' } |
        ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $stage -Recurse }
    Get-ChildItem -LiteralPath $VcRuntimeDirectory -Filter '*.dll' -File |
        ForEach-Object { Copy-PayloadFile $_.FullName $_.Name }
    foreach ($name in @('README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md')) {
        Copy-PayloadFile (Join-Path $project $name) $name
    }
    foreach ($directory in @('assets', 'licenses')) {
        Copy-Item -LiteralPath (Join-Path $project $directory) -Destination $stage -Recurse
    }
    foreach ($name in @('onnxruntime.dll', 'onnxruntime_providers_shared.dll', 'LICENSE.txt')) {
        Copy-PayloadFile (Join-Path $project "runtime\$name") "runtime\$name"
    }
    foreach ($name in @('ecdict.csv', 'ECDICT-LICENSE.txt', 'manifest.json', 'SOURCES.md')) {
        Copy-PayloadFile (Join-Path $project "dictionary\$name") "dictionary\$name"
    }
    foreach ($direction in @('en-zh', 'zh-en')) {
        foreach ($name in @('config.json', 'decoder_model_quantized.onnx', 'encoder_model_quantized.onnx', 'manifest.json', 'source.spm', 'tokenizer.json', 'vocab.json')) {
            Copy-PayloadFile (Join-Path $project "models\$direction\$name") "models\$direction\$name"
        }
        $modelRoot = Join-Path $stage "models\$direction"
        $modelManifest = Get-Content -LiteralPath (Join-Path $modelRoot 'manifest.json') -Raw | ConvertFrom-Json
        foreach ($entry in $modelManifest.files) {
            $file = Get-Item -LiteralPath (Join-Path $modelRoot $entry.file)
            if ($file.Length -ne $entry.bytes -or (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash -ine $entry.sha256) {
                throw "Model hash mismatch: $direction/$($entry.file)"
            }
        }
    }
    foreach ($required in @('Microsoft.ui.xaml.dll', 'Microsoft.UI.Xaml.Controls.dll', 'Microsoft.UI.Input.dll', 'Microsoft.UI.Windowing.dll', 'Microsoft.WindowsAppRuntime.dll', 'Microsoft.Windows.ApplicationModel.Resources.dll', 'Microsoft.Web.WebView2.Core.dll', 'resources.pri', 'Microsoft.UI.Xaml\Assets', 'zh-CN', 'en-us', 'vcruntime140.dll', 'vcruntime140_1.dll', 'msvcp140.dll')) {
        if (-not (Test-Path -LiteralPath (Join-Path $stage $required))) { throw "Self-contained runtime incomplete: $required" }
    }
    $files = @(Get-ChildItem -LiteralPath $stage -Recurse -File -Force)
    foreach ($file in $files) {
        if ($file.Name -match '^(settings|history|free-api-usage)\.(json|tmp)$|^credentials|^secrets?|^configuration-check\.|\.log$|\.dpapi$|^\.env') {
            throw "Private data must not be packaged: $($file.Name)"
        }
    }
    $manifest = @($files | Sort-Object FullName | ForEach-Object {
        [ordered]@{ path = $_.FullName.Substring($stage.Length + 1); bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    })
    [IO.File]::WriteAllText((Join-Path $stage 'payload-sha256.json'), (ConvertTo-Json -InputObject $manifest -Depth 4), $utf8)
    & $IsccPath "/DAppVersion=$version" "/DNumericVersion=$numericVersion" "/DPayloadDir=$stage" "/DReleaseDir=$OutputDirectory" (Join-Path $project 'installer\WinTranslater.iss')
    if ($LASTEXITCODE -ne 0) { throw 'Inno Setup compilation failed.' }
    $installer = Get-Item -LiteralPath (Join-Path $OutputDirectory "WinTranslater-$version-windows-x64-setup.exe")
    $hash = (Get-FileHash -LiteralPath $installer.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'SHA256SUMS.txt'), "$hash  $($installer.Name)`n", $utf8)
    [pscustomobject]@{ Installer = $installer.FullName; Bytes = $installer.Length; SHA256 = $hash; Stage = $(if ($KeepStage) { $stage } else { $null }) }
} finally {
    if (-not $KeepStage) {
        $resolvedStage = [IO.Path]::GetFullPath($stage)
        if (-not $resolvedStage.StartsWith([IO.Path]::GetFullPath($stagingRoot) + '\', [StringComparison]::OrdinalIgnoreCase)) { throw 'Staging cleanup escaped its directory.' }
        if (Test-Path -LiteralPath $resolvedStage) { Remove-Item -LiteralPath $resolvedStage -Recurse -Force }
    }
}
