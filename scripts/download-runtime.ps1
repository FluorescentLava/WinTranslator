param([string]$Destination = (Join-Path $PSScriptRoot '..\runtime'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$cache = Join-Path $env:TEMP 'lingua-onnxruntime-1.24.4'
New-Item -ItemType Directory -Path $cache,$Destination -Force | Out-Null
$package = Join-Path $cache 'runtime.nupkg'
Invoke-WebRequest 'https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/1.24.4/microsoft.ml.onnxruntime.1.24.4.nupkg' -OutFile $package
tar -xf $package -C $cache
Copy-Item -Path (Join-Path $cache 'runtimes\win-x64\native\*.dll') -Destination $Destination -Force
Copy-Item -LiteralPath (Join-Path $cache 'LICENSE') -Destination (Join-Path $Destination 'LICENSE.txt') -ErrorAction SilentlyContinue
Write-Host 'ONNX Runtime x64 ready.'
