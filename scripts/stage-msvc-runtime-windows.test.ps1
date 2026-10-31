# SPDX-License-Identifier: AGPL-3.0-only
param([string]$ArchivePath)
# Focused validation controls plus real app-local staging; no runtime installer is executed.
$ErrorActionPreference = 'Stop'
$script = Join-Path $PSScriptRoot 'stage-msvc-runtime-windows.ps1'
$tokens = $null; $errors = $null
$ast = [Management.Automation.Language.Parser]::ParseFile($script, [ref]$tokens, [ref]$errors)
if ($errors.Count) { throw ($errors | Out-String) }
foreach ($function in $ast.FindAll({ param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] }, $true)) {
    Invoke-Expression $function.Extent.Text
}
function Expect-Failure([scriptblock]$Action, [string]$Message) {
    try { & $Action } catch {
        if ($_.Exception.Message -notlike "*$Message*") { throw "Wrong refusal: $($_.Exception.Message)" }
        return
    }
    throw "Expected refusal containing: $Message"
}
$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = [IO.Path]::GetFullPath((Join-Path $tempBase "wenlan-msvc-test-$([guid]::NewGuid())"))
if (-not $tempRoot.StartsWith($tempBase.TrimEnd('\', '/') + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw 'Unsafe test path' }
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $unsigned = Join-Path $tempRoot 'unsigned.dll'
    [IO.File]::WriteAllText($unsigned, 'not a signed Microsoft runtime')
    Expect-Failure { Assert-MicrosoftSignature $unsigned } 'MSVC Authenticode verification failed'
    $manifest = Join-Path $tempRoot 'manifest.xml'
    $valid = '<BurnManifest><UX><Payload FilePath="license.rtf" SourcePath="u4" Packaging="embedded" Hash="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" FileSize="42" /></UX></BurnManifest>'
    [IO.File]::WriteAllText($manifest, $valid)
    if ((Get-LicensePayload $manifest).SourcePath -ne 'u4') { throw 'Default license selection failed' }
    [IO.File]::WriteAllText($manifest, $valid.Replace('SourcePath="u4"', 'SourcePath="../license"'))
    Expect-Failure { Get-LicensePayload $manifest } 'license payload metadata is invalid'
    [IO.File]::WriteAllText($manifest, $valid.Replace('FilePath="license.rtf"', 'FilePath="1028\license.rtf"'))
    Expect-Failure { Get-LicensePayload $manifest } 'exactly one default license.rtf'
    $duplicate = [xml]$valid
    $ux = $duplicate.DocumentElement.UX
    [void]$ux.AppendChild($ux.Payload.CloneNode($true))
    $duplicate.Save($manifest)
    Expect-Failure { Get-LicensePayload $manifest } 'exactly one default license.rtf'
    # The app's older compiler must not lower the prebuilt ONNX deployment floor.
    $required = Get-RequiredMsvcVersion ([version]'14.29.30133') ([version]'14.44.35211.0')
    Expect-Failure { Assert-MsvcVersion ([version]'14.29.30157.0') $required } 'older than required runtime 14.44.35211.0'
    Assert-MsvcVersion ([version]'14.44.35211.0') $required
    $newerCompiler = Get-RequiredMsvcVersion ([version]'14.45.1') ([version]'14.44.35211.0')
    Expect-Failure { Assert-MsvcVersion ([version]'14.44.35211.0') $newerCompiler } 'older than required runtime 14.45.1'
    Expect-Failure { Assert-BundleHash $unsigned ('0' * 64) } 'bundle SHA256 mismatch'
    $cab = New-Object byte[] 40
    [Text.Encoding]::ASCII.GetBytes('MSCF').CopyTo($cab, 0)
    [BitConverter]::GetBytes([uint32]40).CopyTo($cab, 8)
    $cabPath = Join-Path $tempRoot 'test.cab'
    Export-PinnedCabinet $cab 0 40 $cabPath
    Expect-Failure { Export-PinnedCabinet $cab 1 40 $cabPath } 'pinned cabinet layout mismatch'
    Expect-Failure { Export-PinnedCabinet $cab 0 39 $cabPath } 'pinned cabinet layout mismatch'
    $runtimeXml = '<BurnManifest><Payload FilePath="packages\vcRuntimeMinimum_amd64\cab1.cab" SourcePath="a12" Packaging="embedded" Container="WixAttachedContainer" Hash="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA" FileSize="42" /></BurnManifest>'
    [IO.File]::WriteAllText($manifest, $runtimeXml)
    if ((Get-RuntimeCabinetPayload $manifest).SourcePath -ne 'a12') { throw 'Runtime cabinet selection failed' }
    Expect-Failure { Assert-PayloadHash $unsigned (Get-RuntimeCabinetPayload $manifest) } 'payload does not match the signed bundle manifest'
    [IO.File]::WriteAllText($manifest, $runtimeXml.Replace('SourcePath="a12"', 'SourcePath="../a12"'))
    Expect-Failure { Get-RuntimeCabinetPayload $manifest } 'signed runtime cabinet metadata is invalid'
    [IO.File]::WriteAllText($manifest, $runtimeXml.Replace('amd64', 'arm64'))
    Expect-Failure { Get-RuntimeCabinetPayload $manifest } 'exactly one x64 runtime cabinet'
    $stage = Join-Path $tempRoot 'stage'
    # Without a supplied copy, the one download fills a cache path that did not exist.
    $archive = if ($ArchivePath) { $ArchivePath } else { Join-Path $tempRoot 'cache\vc_redist.x64.exe' }
    & $script -DestinationDirectory $stage -ArchivePath $archive
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) { throw 'Verified bundle was not kept at -ArchivePath' }
    Assert-BundleHash $archive 'CC0FF0EB1DC3F5188AE6300FAEF32BF5BEEBA4BDD6E8E445A9184072096B713B'
    if (@(Get-ChildItem -LiteralPath (Split-Path -Parent $archive) -Filter '*.partial').Count) { throw 'Partial cache file left behind' }
    $expected = @('concrt140.dll', 'msvcp140.dll', 'msvcp140_1.dll', 'msvcp140_2.dll',
        'msvcp140_atomic_wait.dll', 'msvcp140_codecvt_ids.dll', 'vccorlib140.dll',
        'vcruntime140.dll', 'vcruntime140_1.dll', 'vcomp140.dll', 'MSVC-Runtime-License.rtf', 'MSVC-Runtime-Source.txt')
    $actual = @(Get-ChildItem -LiteralPath $stage -File | ForEach-Object Name)
    if (@(Compare-Object ($expected | Sort-Object) ($actual | Sort-Object)).Count) { throw 'Staged inventory mismatch' }
    if (-not ([IO.File]::ReadAllText((Join-Path $stage 'MSVC-Runtime-License.rtf'))).StartsWith('{\rtf')) { throw 'Original RTF license missing' }
    foreach ($file in (Get-ChildItem -LiteralPath $stage -Filter '*.dll')) {
        Assert-MsvcBinary $file.FullName ([version]'14.44.35211.0')
    }
    $notice = [IO.File]::ReadAllText((Join-Path $stage 'MSVC-Runtime-Source.txt'))
    if ($notice -notmatch 'Prebuilt ONNX Runtime 1.23.2 deployment floor: 14.44.35211.0') { throw 'Prebuilt dependency provenance missing' }
    # A second run over a complete stage re-verifies it and stages nothing. The cache path
    # it is given does not exist, so reaching the network or 7-Zip would fail the check.
    $missingArchive = Join-Path $tempRoot 'absent\vc_redist.x64.exe'
    $second = (& $script -DestinationDirectory $stage -ArchivePath $missingArchive 6>&1 | Out-String)
    if ($second -notmatch 'already staged') { throw "Complete stage was not reused: $second" }
    if (Test-Path -LiteralPath $missingArchive) { throw 'Reused stage still downloaded the bundle' }
    $noticeLines = @(Get-Content -LiteralPath (Join-Path $stage 'MSVC-Runtime-Source.txt') -Encoding UTF8)
    $runtimeNames = @($expected | Where-Object { $_ -like '*.dll' })
    $header = @($noticeLines[0..($noticeLines.Count - $runtimeNames.Count - 2)])
    if ($header[-1] -notlike 'vc_redist.x64.exe SHA256: *') { throw 'Notice header shape changed' }
    $pinned = [version]'14.44.35211.0'
    if (-not (Test-StagedRuntime $stage $runtimeNames $header $pinned $pinned)) { throw 'Complete stage rejected' }
    # A different pin or compiler changes the header, and the stage is no longer reused.
    $otherHeader = @($header[0..($header.Count - 2)]) + ('vc_redist.x64.exe SHA256: ' + ('0' * 64))
    if (Test-StagedRuntime $stage $runtimeNames $otherHeader $pinned $pinned) { throw 'Stage for another pin reused' }
    function Copy-Stage([string]$Name) {
        $copy = Join-Path $tempRoot $Name
        Copy-Item -LiteralPath $stage -Destination $copy -Recurse
        return $copy
    }
    $partial = Copy-Stage 'missing-dll'
    Remove-Item -LiteralPath (Join-Path $partial 'vcomp140.dll')
    if (Test-StagedRuntime $partial $runtimeNames $header $pinned $pinned) { throw 'Stage missing a DLL reused' }
    $noNotice = Copy-Stage 'missing-notice'
    Remove-Item -LiteralPath (Join-Path $noNotice 'MSVC-Runtime-Source.txt')
    if (Test-StagedRuntime $noNotice $runtimeNames $header $pinned $pinned) { throw 'Stage without a notice reused' }
    $swapped = Copy-Stage 'swapped-dll'
    Copy-Item -LiteralPath (Join-Path $swapped 'msvcp140.dll') -Destination (Join-Path $swapped 'concrt140.dll') -Force
    if (Test-StagedRuntime $swapped $runtimeNames $header $pinned $pinned) { throw 'Stage with a changed DLL reused' }
    $license = Copy-Stage 'changed-license'
    [IO.File]::AppendAllText((Join-Path $license 'MSVC-Runtime-License.rtf'), ' ')
    if (Test-StagedRuntime $license $runtimeNames $header $pinned $pinned) { throw 'Stage with a changed license reused' }
    # Recording the hash of a replaced DLL does not help: the signature is re-verified too.
    $forged = Copy-Stage 'forged-dll'
    [IO.File]::WriteAllText((Join-Path $forged 'vcomp140.dll'), 'not a signed Microsoft runtime')
    $forgedHash = (Get-FileHash -LiteralPath (Join-Path $forged 'vcomp140.dll') -Algorithm SHA256).Hash
    $forgedNotice = @($noticeLines | ForEach-Object { if ($_ -like 'vcomp140.dll SHA256: *') { "vcomp140.dll SHA256: $forgedHash" } else { $_ } })
    $forgedNotice | Set-Content -LiteralPath (Join-Path $forged 'MSVC-Runtime-Source.txt') -Encoding UTF8
    if (Test-StagedRuntime $forged $runtimeNames $header $pinned $pinned) { throw 'Stage with an unsigned DLL reused' }
    $dll = Join-Path $stage 'msvcp140.dll'
    Assert-MsvcBinary $dll ([version]'14.44.35211.0')
    Expect-Failure { Assert-MsvcBinary $dll ([version]'14.99.99999') } 'older than required runtime'
    # The PE validator must also reject the wrong architecture after signature verification.
    # Locally shadow signature verification only for this synthetic malformed-PE control.
    function Assert-MicrosoftSignature([string]$Path) { }
    $bytes = New-Object byte[] 128
    $bytes[0] = 77; $bytes[1] = 90
    [BitConverter]::GetBytes([int]64).CopyTo($bytes, 60)
    [BitConverter]::GetBytes([int]17744).CopyTo($bytes, 64)
    [BitConverter]::GetBytes([uint16]332).CopyTo($bytes, 68)
    [IO.File]::WriteAllBytes($unsigned, $bytes)
    Expect-Failure { Assert-MsvcBinary $unsigned ([version]'14.0') } 'must be x64 PE32+'
    Write-Host 'PASS: MSVC bundle/payload hash, cabinet bounds, archive cache fill, reuse of a verified stage and refusal of incomplete/changed/forged stages, unsigned source, x64 architecture, old compiler plus newer prebuilt dependency, license selection/path/ambiguity, and real signed 14.44 staging controls'
}
finally { Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue }
