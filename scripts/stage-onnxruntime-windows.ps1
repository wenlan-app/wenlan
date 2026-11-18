param(
    [Parameter(Mandatory = $true)]
    [string]$DestinationDirectory
)

$ErrorActionPreference = "Stop"
# Windows PowerShell 5.1 (used by prepare-sidecars.sh) redraws a progress bar per
# chunk, which makes a 120 MB download crawl.
$ProgressPreference = "SilentlyContinue"

# Verified against ort 2.0.0-rc.11 (ort commit
# 2de34065983a5c034f5afcc072b23b99479f465b):
# ort-sys/build/download/dist.txt pins Windows x64 CPU to ms@1.23.2.
#
# Source: Microsoft's official NuGet package (Microsoft.ML.OnnxRuntime). The
# GitHub release zip (onnxruntime-win-x64-1.23.2.zip) started returning 404 on
# 2026-11-18. NuGet packages are immutable once published, so the pinned hash
# below cannot drift. A .nupkg is a zip; the DLL lives at
# runtimes/win-x64/native/onnxruntime.dll (PE linker 14.40, same as the old zip).
$OrtVersion = "1.23.2"
$ExpectedDllSha256 = "dec964ab1ee36cc9b0ae247d13b376627992fc57dec0454354017ab8fd84f1ea"
$ExpectedZipSha256 = "25fe172f7fcdf34f2b5b02c8b997f6b88abc282956849d4af4335af23d0c4e4a"
$ZipUrl = "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/$OrtVersion/microsoft.ml.onnxruntime.$OrtVersion.nupkg"
$TempBase = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$TempRoot = Join-Path $TempBase "wenlan-ort-$([System.Guid]::NewGuid())"
$ZipPath = Join-Path $TempRoot "onnxruntime.zip"
$ExtractPath = Join-Path $TempRoot "extracted"

New-Item -ItemType Directory -Path $TempRoot | Out-Null

try {
    Write-Host "Downloading $ZipUrl"
    # Invoke-WebRequest waits forever by default, so a stalled connection looks
    # exactly like a slow build. CI's job timeout would eventually catch it; a
    # developer's machine never would.
    Invoke-WebRequest -Uri $ZipUrl -OutFile $ZipPath -TimeoutSec 300

    $ActualZipSha256 = (Get-FileHash -LiteralPath $ZipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualZipSha256 -ne $ExpectedZipSha256) {
        throw "onnxruntime archive SHA-256 mismatch: expected $ExpectedZipSha256, got $ActualZipSha256"
    }

    # The package also ships win-arm64 and win-x86 DLLs and is ~120 MB, so read
    # only the win-x64 entry instead of expanding the whole archive.
    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $EntryName = "runtimes/win-x64/native/onnxruntime.dll"
    $Archive = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        $Entry = $Archive.GetEntry($EntryName)
        if (-not $Entry) {
            throw "$EntryName not found in verified archive"
        }
        New-Item -ItemType Directory -Path $ExtractPath | Out-Null
        $ExtractedDll = Join-Path $ExtractPath "onnxruntime.dll"
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($Entry, $ExtractedDll, $true)
    }
    finally {
        $Archive.Dispose()
    }

    $ActualDllSha256 = (Get-FileHash -LiteralPath $ExtractedDll -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($ActualDllSha256 -ne $ExpectedDllSha256) {
        throw "onnxruntime.dll SHA-256 mismatch: expected $ExpectedDllSha256, got $ActualDllSha256"
    }

    New-Item -ItemType Directory -Path $DestinationDirectory -Force | Out-Null
    $Destination = Join-Path $DestinationDirectory "onnxruntime.dll"
    Copy-Item -LiteralPath $ExtractedDll -Destination $Destination -Force
    Write-Host "Staged verified ONNX Runtime $OrtVersion at $Destination"
}
finally {
    Remove-Item -LiteralPath $TempRoot -Recurse -Force -ErrorAction SilentlyContinue
}
