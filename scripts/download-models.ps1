param([string]$Destination = (Join-Path $PSScriptRoot '..\models'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
$files = @('config.json', 'source.spm', 'vocab.json', 'onnx/encoder_model_quantized.onnx', 'onnx/decoder_model_quantized.onnx')
foreach ($pair in @('en-zh', 'zh-en')) {
    $repo = "Xenova/opus-mt-$pair"
    $metadata = Invoke-RestMethod "https://huggingface.co/api/models/${repo}?blobs=true"
    $revision = $metadata.sha
    $pairPath = Join-Path $Destination $pair
    New-Item -ItemType Directory -Path $pairPath -Force | Out-Null
    $manifest = @()
    foreach ($file in $files) {
        $name = Split-Path $file -Leaf
        $dest = Join-Path $pairPath $name
        $entry = $metadata.siblings | Where-Object rfilename -eq $file
        if (!$entry) { throw "Model file missing: $repo/$file" }
        if (!(Test-Path -LiteralPath $dest) -or (Get-Item -LiteralPath $dest).Length -ne $entry.size) {
            Write-Host "Downloading $pair/$name ($([math]::Round($entry.size / 1MB, 1)) MiB)..."
            $partial = "$dest.partial"
            Invoke-WebRequest "https://huggingface.co/$repo/resolve/$revision/$file" -OutFile $partial
            if ((Get-Item -LiteralPath $partial).Length -ne $entry.size) { throw "Incomplete download: $name" }
            Move-Item -LiteralPath $partial -Destination $dest -Force
        }
        $sha = (Get-FileHash -LiteralPath $dest -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($entry.lfs.sha256 -and $sha -ne $entry.lfs.sha256) { throw "Checksum mismatch: $name" }
        $manifest += @{ file = $name; sha256 = $sha; bytes = $entry.size }
    }
    @{ repository = $repo; revision = $revision; files = $manifest } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $pairPath 'manifest.json') -Encoding utf8
    Write-Host "Verified $pair."
}
Write-Host 'Offline models ready: English <-> Simplified Chinese.'
