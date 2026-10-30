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
    & $script -DestinationDirectory $stage -ArchivePath $ArchivePath
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
    Write-Host 'PASS: MSVC bundle/payload hash, cabinet bounds, unsigned source, x64 architecture, old compiler plus newer prebuilt dependency, license selection/path/ambiguity, and real signed 14.44 staging controls'
}
finally { Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue }
