param([string]$Destination = (Join-Path $PSScriptRoot '..\dictionary'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$revision = '39cfb9672c0906b63569e0453c2e6abfb9ab182a'
$sourceHash = 'd1ecbdf9ee2b920afb6f430ca6b6bb9f559588cf0dff86e04bd255f9390843cc'
$baseUrl = "https://raw.githubusercontent.com/skywind3000/ECDICT/$revision"
New-Item -ItemType Directory -Path $Destination -Force | Out-Null
$Destination = (Resolve-Path -LiteralPath $Destination).Path
$sourcePath = Join-Path $Destination 'ecdict.source.partial'
$partialPath = Join-Path $Destination 'ecdict.csv.partial'
$targetPath = Join-Path $Destination 'ecdict.csv'
$encoding = [System.Text.UTF8Encoding]::new($false)

# ECDICT's base CSV has 770,611 entries. Keep common words using only the
# upstream frequency/exam metadata; no manually selected words or definitions.
try {
    Write-Host 'Downloading pinned ECDICT base dictionary (66 MB)...'
    Invoke-WebRequest "$baseUrl/ecdict.csv" -OutFile $sourcePath
    if ((Get-Item -LiteralPath $sourcePath).Length -ne 66009548) { throw 'Incomplete dictionary download.' }
    if ((Get-FileHash -LiteralPath $sourcePath -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sourceHash) { throw 'ECDICT checksum mismatch.' }
    Invoke-WebRequest "$baseUrl/LICENSE" -OutFile (Join-Path $Destination 'ECDICT-LICENSE.txt')
    Add-Type -AssemblyName Microsoft.VisualBasic
    $parser = [Microsoft.VisualBasic.FileIO.TextFieldParser]::new($sourcePath, $encoding)
    $parser.SetDelimiters(',')
    $parser.HasFieldsEnclosedInQuotes = $true
    $parser.TrimWhiteSpace = $false
    $writer = [System.IO.StreamWriter]::new($partialPath, $false, $encoding)
    $writer.NewLine = "`n"
    $total = 0; $retained = 0
    try {
        $header = $parser.ReadFields()
        $columns = @{}
        for ($i = 0; $i -lt $header.Length; $i++) { $columns[$header[$i]] = $i }
        foreach ($required in @('word','phonetic','translation','definition','pos','tag','bnc','frq','collins','oxford')) {
            if (!$columns.ContainsKey($required)) { throw "Missing ECDICT field: $required" }
        }
        $writer.WriteLine(($header -join ','))
        while (!$parser.EndOfData) {
            $fields = $parser.ReadFields()
            $total++
            $bnc = 0; $frq = 0
            $null = [int]::TryParse($fields[$columns['bnc']], [ref]$bnc)
            $null = [int]::TryParse($fields[$columns['frq']], [ref]$frq)
            $collins = $fields[$columns['collins']].Trim()
            $keep = $fields[$columns['tag']].Trim().Length -gt 0 -or
                ($collins.Length -gt 0 -and $collins -ne '0') -or
                $fields[$columns['oxford']].Trim() -eq '1' -or
                ($bnc -gt 0 -and $bnc -le 100000) -or ($frq -gt 0 -and $frq -le 100000)
            if (!$keep) { continue }
            $escaped = foreach ($field in $fields) {
                if ($field.IndexOfAny([char[]]@(',', '"', "`r", "`n")) -ge 0) { '"' + $field.Replace('"', '""') + '"' } else { $field }
            }
            $writer.WriteLine(($escaped -join ','))
            $retained++
        }
    } finally { $writer.Dispose(); $parser.Dispose() }
    if ($retained -ne 59137 -or $total -ne 770611) { throw "Unexpected ECDICT row counts: $retained/$total" }
    Move-Item -LiteralPath $partialPath -Destination $targetPath -Force
    $manifest = [ordered]@{
        repository = 'https://github.com/skywind3000/ECDICT'
        revision = $revision
        license = 'MIT'
        source_url = "$baseUrl/ecdict.csv"
        source_bytes = 66009548
        source_sha256 = $sourceHash
        source_rows = $total
        filter = 'tag nonempty OR collins nonzero OR oxford=1 OR 1<=bnc<=100000 OR 1<=frq<=100000'
        rows = $retained
        bytes = (Get-Item -LiteralPath $targetPath).Length
        sha256 = (Get-FileHash -LiteralPath $targetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    }
    [System.IO.File]::WriteAllText((Join-Path $Destination 'manifest.json'), (($manifest | ConvertTo-Json -Depth 4) + "`n"), $encoding)
    Write-Host "Offline dictionary ready: $retained entries. Restart the app after downloading."
} finally {
    if (Test-Path -LiteralPath $sourcePath) { Remove-Item -LiteralPath $sourcePath -Force }
    if (Test-Path -LiteralPath $partialPath) { Remove-Item -LiteralPath $partialPath -Force }
}
