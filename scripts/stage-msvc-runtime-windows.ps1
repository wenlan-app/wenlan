param(
    [Parameter(Mandatory = $true)][string]$DestinationDirectory,
    [string]$VisualStudioInstallPath,
    # Optional offline copy; the same immutable SHA-256 and signature are mandatory.
    [string]$ArchivePath
)
$ErrorActionPreference = 'Stop'

function Assert-MicrosoftSignature([string]$Path, [switch]$Bundle) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    $signer = if ($signature.SignerCertificate) {
        $signature.SignerCertificate.GetNameInfo([System.Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    } else { '' }
    # DLLs carry Microsoft Windows/WHQL signatures; the source bundle is signed by Microsoft Corporation.
    $publishers = if ($Bundle) { @('Microsoft Corporation') } else {
        @('Microsoft Corporation', 'Microsoft Windows', 'Microsoft Windows Software Compatibility Publisher')
    }
    if ($signature.Status -ne 'Valid' -or $signer -notin $publishers) {
        throw "MSVC Authenticode verification failed: '$Path', status=$($signature.Status), signer='$signer'"
    }
}

function Get-RequiredMsvcVersion([version]$CompilerVersion, [version]$PrebuiltMinimum) {
    if ($CompilerVersion.Major -ne 14 -or $PrebuiltMinimum.Major -ne 14) { throw 'MSVC toolset and dependency floor must be 14.x' }
    if ($CompilerVersion -gt $PrebuiltMinimum) { return $CompilerVersion }
    return $PrebuiltMinimum
}

function Assert-MsvcVersion([version]$RuntimeVersion, [version]$MinimumVersion) {
    if ($RuntimeVersion.Major -ne 14 -or $RuntimeVersion -lt $MinimumVersion) {
        throw "MSVC runtime version $RuntimeVersion is older than required runtime $MinimumVersion or is not 14.x"
    }
}

function Assert-MsvcBinary([string]$Path, [version]$MinimumVersion) {
    Assert-MicrosoftSignature $Path
    $bytes = [IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 77 -or $bytes[1] -ne 90) { throw "MSVC invalid PE: $Path" }
    $pe = [BitConverter]::ToInt32($bytes, 60)
    if ($pe -lt 64 -or $pe -gt $bytes.Length - 26 -or [BitConverter]::ToUInt32($bytes, $pe) -ne 17744 -or
        [BitConverter]::ToUInt16($bytes, $pe + 4) -ne 34404 -or [BitConverter]::ToUInt16($bytes, $pe + 24) -ne 523) {
        throw "MSVC runtime must be x64 PE32+: $Path"
    }
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($Path)
    Assert-MsvcVersion ([version]::new($info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart, $info.FilePrivatePart)) $MinimumVersion
}

function Read-BundleManifest([string]$ManifestPath) {
    $settings = [Xml.XmlReaderSettings]::new()
    $settings.DtdProcessing = [Xml.DtdProcessing]::Prohibit
    $settings.XmlResolver = $null
    $reader = [Xml.XmlReader]::Create($ManifestPath, $settings)
    try { $manifest = [Xml.XmlDocument]::new(); $manifest.XmlResolver = $null; $manifest.Load($reader) }
    finally { $reader.Dispose() }
    return $manifest
}

function Get-LicensePayload([string]$ManifestPath) {
    $manifest = Read-BundleManifest $ManifestPath
    $payloads = @($manifest.SelectNodes("//*[local-name()='UX']/*[local-name()='Payload' and @FilePath='license.rtf']"))
    if ($payloads.Count -ne 1) { throw 'MSVC signed bundle must contain exactly one default license.rtf' }
    $payload = $payloads[0]
    if ($payload.SourcePath -notmatch '^u[0-9]+$' -or $payload.Packaging -ne 'embedded' -or
        $payload.Hash -notmatch '^(?:[A-Fa-f0-9]{40}|[A-Fa-f0-9]{64}|[A-Fa-f0-9]{128})$' -or
        [long]$payload.FileSize -lt 1 -or [long]$payload.FileSize -gt 1048576) {
        throw 'MSVC signed license payload metadata is invalid'
    }
    return $payload
}

function Get-RuntimeCabinetPayload([string]$ManifestPath) {
    $manifest = Read-BundleManifest $ManifestPath
    $payloads = @($manifest.SelectNodes("/*[local-name()='BurnManifest']/*[local-name()='Payload' and @FilePath='packages\vcRuntimeMinimum_amd64\cab1.cab']"))
    if ($payloads.Count -ne 1) { throw 'MSVC signed bundle must contain exactly one x64 runtime cabinet' }
    $payload = $payloads[0]
    if ($payload.SourcePath -notmatch '^a[0-9]+$' -or $payload.Packaging -ne 'embedded' -or
        $payload.Container -ne 'WixAttachedContainer' -or $payload.Hash -notmatch '^[A-Fa-f0-9]{40}$' -or
        [long]$payload.FileSize -lt 1 -or [long]$payload.FileSize -gt 16777216) {
        throw 'MSVC signed runtime cabinet metadata is invalid'
    }
    return $payload
}

function Assert-BundleHash([string]$Path, [string]$ExpectedHash) {
    if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $ExpectedHash) { throw 'MSVC bundle SHA256 mismatch' }
}

function Assert-PayloadHash([string]$Path, $Payload) {
    $algorithm = switch ($Payload.Hash.Length) { 40 { 'SHA1' } 64 { 'SHA256' } 128 { 'SHA512' } default { throw 'MSVC invalid payload hash' } }
    if ((Get-Item -LiteralPath $Path).Length -ne [long]$Payload.FileSize -or
        (Get-FileHash -LiteralPath $Path -Algorithm $algorithm).Hash -ne $Payload.Hash) {
        throw 'MSVC payload does not match the signed bundle manifest'
    }
}

function Export-PinnedCabinet([byte[]]$Bytes, [int]$Offset, [int]$Length, [string]$Path) {
    if ($Offset -lt 0 -or $Length -lt 36 -or [long]$Offset + $Length -gt $Bytes.LongLength -or
        [Text.Encoding]::ASCII.GetString($Bytes, $Offset, 4) -ne 'MSCF' -or
        [BitConverter]::ToUInt32($Bytes, $Offset + 8) -ne $Length) {
        throw 'MSVC pinned cabinet layout mismatch'
    }
    $cabinet = New-Object byte[] $Length
    [Array]::Copy($Bytes, $Offset, $cabinet, 0, $Length)
    [IO.File]::WriteAllBytes($Path, $cabinet)
}

# Refresh URL, hash, version, dependency floor and CAB offsets together. Never execute the EXE/MSI.
$bundleUrl = 'https://download.visualstudio.microsoft.com/download/pr/990c720a-46ce-4dc7-87b9-c2eb615d34b9/CC0FF0EB1DC3F5188AE6300FAEF32BF5BEEBA4BDD6E8E445A9184072096B713B/VC_redist.x64.exe'
$bundleHash = 'CC0FF0EB1DC3F5188AE6300FAEF32BF5BEEBA4BDD6E8E445A9184072096B713B'
$pinnedVersion = [version]'14.44.35211.0'
# ONNX Runtime 1.23.2's Windows DLL reports linker 14.40, newer than VS2019's 14.29.
# PE linker metadata does not expose the compiler patch. Use this conservative pinned deployment floor,
# not the local app compiler alone. Import/export-name closure cannot establish STL ABI compatibility.
$prebuiltMinimum = [version]'14.44.35211.0'
if (-not $VisualStudioInstallPath) {
    if ($env:VSINSTALLDIR) { $VisualStudioInstallPath = $env:VSINSTALLDIR }
    else {
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        $installations = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
        if ($LASTEXITCODE -ne 0 -or $installations.Count -ne 1) { throw 'vswhere could not uniquely locate MSVC' }
        $VisualStudioInstallPath = $installations[0]
    }
}
if (-not $VisualStudioInstallPath) { throw 'Visual Studio MSVC build tools are required for runtime staging' }
$vc = Join-Path $VisualStudioInstallPath.Trim() 'VC'
$compilerVersion = if ($env:VCToolsVersion) { $env:VCToolsVersion.Trim() } else {
    (Get-Content -LiteralPath (Join-Path $vc 'Auxiliary\Build\Microsoft.VCToolsVersion.default.txt') -Raw).Trim()
}
if ($compilerVersion -notmatch '^14\.\d+\.\d+(?:\.\d+)?$') { throw 'MSVC compiler toolset version must be 14.x' }
$minimumVersion = Get-RequiredMsvcVersion ([version]$compilerVersion) $prebuiltMinimum
Assert-MsvcVersion $pinnedVersion $minimumVersion
$runtimeNames = @('concrt140.dll', 'msvcp140.dll', 'msvcp140_1.dll', 'msvcp140_2.dll',
    'msvcp140_atomic_wait.dll', 'msvcp140_codecvt_ids.dll', 'vccorlib140.dll',
    'vcruntime140.dll', 'vcruntime140_1.dll', 'vcomp140.dll')
$sevenZip = Get-Command 7z.exe -ErrorAction SilentlyContinue
$sevenZipPath = if ($sevenZip) { $sevenZip.Source } else { Join-Path $env:ProgramFiles '7-Zip\7z.exe' }
if (-not (Test-Path -LiteralPath $sevenZipPath -PathType Leaf)) { throw '7-Zip is required to extract the signed MSVC runtime without running the installer' }
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "wenlan-msvc-$([guid]::NewGuid())"))
if (-not $tempRoot.StartsWith($tempBase.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe MSVC staging path' }
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $bundle = $ArchivePath
    if (-not $bundle) {
        $bundle = Join-Path $tempRoot 'vc_redist.x64.exe'
        Invoke-WebRequest -UseBasicParsing -Uri $bundleUrl -OutFile $bundle -TimeoutSec 120
    }
    Assert-BundleHash $bundle $bundleHash
    Assert-MicrosoftSignature $bundle -Bundle
    $bundleInfo = [Diagnostics.FileVersionInfo]::GetVersionInfo($bundle)
    if ([version]::new($bundleInfo.FileMajorPart, $bundleInfo.FileMinorPart, $bundleInfo.FileBuildPart, $bundleInfo.FilePrivatePart) -ne $pinnedVersion) {
        throw 'MSVC pinned bundle version mismatch'
    }
    $bytes = [IO.File]::ReadAllBytes($bundle)
    $uxCab = Join-Path $tempRoot 'ux.cab'
    $attachedCab = Join-Path $tempRoot 'attached.cab'
    Export-PinnedCabinet $bytes 479744 195988 $uxCab
    Export-PinnedCabinet $bytes 686152 24939223 $attachedCab
    & $sevenZipPath e -y "-o$tempRoot" $uxCab '0' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'MSVC signed manifest extraction failed' }
    $manifest = Join-Path $tempRoot '0'
    $licensePayload = Get-LicensePayload $manifest
    $runtimePayload = Get-RuntimeCabinetPayload $manifest
    & $sevenZipPath e -y "-o$tempRoot" $uxCab $licensePayload.SourcePath | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'MSVC license extraction failed' }
    & $sevenZipPath e -y "-o$tempRoot" $attachedCab $runtimePayload.SourcePath | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'MSVC runtime cabinet extraction failed' }
    $license = Join-Path $tempRoot $licensePayload.SourcePath
    $runtimeCab = Join-Path $tempRoot $runtimePayload.SourcePath
    Assert-PayloadHash $license $licensePayload
    Assert-PayloadHash $runtimeCab $runtimePayload
    $entries = @($runtimeNames | ForEach-Object { "${_}_amd64" })
    & $sevenZipPath e -y "-o$tempRoot" $runtimeCab @entries | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'MSVC DLL extraction failed' }
    # Validate every payload before writing any destination DLL.
    foreach ($name in $runtimeNames) {
        $source = Join-Path $tempRoot "${name}_amd64"
        Assert-MsvcBinary $source $minimumVersion
        $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($source)
        if ([version]::new($info.FileMajorPart, $info.FileMinorPart, $info.FileBuildPart, $info.FilePrivatePart) -ne $pinnedVersion) {
            throw "MSVC incoherent runtime payload version: $name"
        }
    }
    $destination = [IO.Path]::GetFullPath($DestinationDirectory)
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    $notice = @('Microsoft Visual C++ 14.x x64 app-local runtime',
        "Source: $bundleUrl", "Runtime version: $pinnedVersion", "Compiler toolset: $compilerVersion",
        "Prebuilt ONNX Runtime 1.23.2 deployment floor: $prebuiltMinimum (PE linker family 14.40)",
        "Effective required runtime: $minimumVersion",
        'Publisher: Microsoft; valid bundle and individual DLL Authenticode signatures verified before copying',
        'Original license: MSVC-Runtime-License.rtf, extracted unmodified from the signed vc_redist.x64.exe',
        'https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution',
        'https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files',
        'https://learn.microsoft.com/en-us/cpp/porting/binary-compat-2015-2017',
        "vc_redist.x64.exe SHA256: $bundleHash")
    foreach ($name in $runtimeNames) {
        $source = Join-Path $tempRoot "${name}_amd64"
        $hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
        Copy-Item -LiteralPath $source -Destination (Join-Path $destination $name) -Force
        if ((Get-FileHash -LiteralPath (Join-Path $destination $name) -Algorithm SHA256).Hash -ne $hash) { throw "MSVC staged hash mismatch: $name" }
        $notice += "$name SHA256: $hash"
    }
    Copy-Item -LiteralPath $license -Destination (Join-Path $destination 'MSVC-Runtime-License.rtf') -Force
    $notice | Set-Content -LiteralPath (Join-Path $destination 'MSVC-Runtime-Source.txt') -Encoding UTF8
    Write-Host "PASS: ten Microsoft-signed x64 CRT/OpenMP DLLs $pinnedVersion and original license staged at $destination"
}
finally { Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue }
