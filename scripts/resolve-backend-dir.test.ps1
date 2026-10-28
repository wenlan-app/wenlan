$ErrorActionPreference = 'Stop'
# SPDX-License-Identifier: Apache-2.0
$previousBackend = $env:WENLAN_BACKEND_DIR
$env:WENLAN_NO_AUTOSTART = '1'
$repo = Split-Path -Parent $PSScriptRoot
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('wenlan-backend-' + [Guid]::NewGuid().ToString('N'))
try {
    $backend = Join-Path $fixture '[Wenlan] 中文 backend'
    foreach ($crate in @('wenlan-server', 'wenlan-mcp', 'wenlan-cli')) {
        [IO.Directory]::CreateDirectory((Join-Path $backend "crates\$crate")) | Out-Null
    }
    [IO.File]::WriteAllText((Join-Path $backend 'Cargo.toml'), '[workspace]')
    foreach ($path in @($backend, $backend.Replace('\', '/'))) {
        $env:WENLAN_BACKEND_DIR = $path
        $result = & node (Join-Path $PSScriptRoot 'run-bash.mjs') (Join-Path $PSScriptRoot 'resolve-backend-dir.sh')
        if ($LASTEXITCODE -ne 0) { throw "backend resolver rejected $path" }
        $env:WENLAN_BACKEND_DIR = $result.Trim()
        $again = & node (Join-Path $PSScriptRoot 'run-bash.mjs') (Join-Path $PSScriptRoot 'resolve-backend-dir.sh')
        if ($LASTEXITCODE -ne 0 -or $again.Trim() -ne $result.Trim()) { throw 'native and MSYS paths resolved differently' }
    }
    $env:WENLAN_BACKEND_DIR = '.'
    $null = & node (Join-Path $PSScriptRoot 'run-bash.mjs') (Join-Path $PSScriptRoot 'resolve-backend-dir.sh') $repo
    if ($LASTEXITCODE -ne 0) { throw 'relative backend override regressed' }
    'PASS: native, slash, MSYS, relative, bracket, space and Unicode backend paths'
} finally {
    $env:WENLAN_BACKEND_DIR = $previousBackend
    $full = [IO.Path]::GetFullPath($fixture)
    $prefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'unsafe fixture cleanup' }
    Remove-Item -LiteralPath $full -Recurse -Force
}
