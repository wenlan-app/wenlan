$ErrorActionPreference = "Stop"
$smokePath = Join-Path $PSScriptRoot 'smoke-windows.ps1'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
    $smokePath, [ref]$tokens, [ref]$parseErrors
)
if ($parseErrors.Count -ne 0) {
    throw "smoke-windows.ps1 has PowerShell parse errors: $($parseErrors -join '; ')"
}
foreach ($name in @('Wait-EmbeddingModelReady', 'Get-SmokeDiagnostics')) {
    $definition = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq $name
    }, $true)
    if ($null -eq $definition) { throw "smoke-windows.ps1 lacks $name" }
    Invoke-Expression $definition.Extent.Text
}

$script:clock = [System.Collections.Generic.Queue[DateTimeOffset]]::new()
$script:statuses = [System.Collections.Generic.Queue[object]]::new()
$script:polls = 0
$script:sleeps = 0
function Get-Date {
    if ($script:clock.Count -eq 0) { throw 'test clock exhausted' }
    return $script:clock.Dequeue()
}
function Invoke-RestMethod {
    param([string]$Uri, [int]$TimeoutSec)
    if ($Uri -ne 'http://smoke-test/api/status' -or $TimeoutSec -ne 1) {
        throw "unexpected status request: $Uri timeout=$TimeoutSec"
    }
    $script:polls++
    if ($script:statuses.Count -eq 0) { throw 'test status queue exhausted' }
    return $script:statuses.Dequeue()
}
function Start-Sleep {
    param([int]$Seconds)
    if ($Seconds -ne 1) { throw "unexpected sleep duration: $Seconds" }
    $script:sleeps++
}
function Set-Scenario {
    param([DateTimeOffset[]]$Times, [object[]]$Statuses)
    $script:clock.Clear()
    $script:statuses.Clear()
    foreach ($time in $Times) { $script:clock.Enqueue($time) }
    foreach ($status in $Statuses) { $script:statuses.Enqueue($status) }
    $script:polls = 0
    $script:sleeps = 0
}
function Status {
    param([string]$State, [string]$Reason = '')
    return [pscustomobject]@{
        embedding_model = [pscustomobject]@{ state = $State; reason = $Reason }
    }
}
function Assert-Fails {
    param([scriptblock]$Action, [string]$ExpectedMessage)
    $failed = $false
    try { & $Action }
    catch {
        $failed = $true
        if (-not $_.Exception.Message.Contains($ExpectedMessage)) {
            throw "failed for wrong reason: $($_.Exception.Message)"
        }
    }
    if (-not $failed) { throw "unexpected success; expected '$ExpectedMessage'" }
}

$base = [DateTimeOffset]::Parse('2026-11-06T00:00:00Z')
$daemon = [pscustomobject]@{ HasExited = $false; ExitCode = 0 }
Set-Scenario -Times @($base, $base, $base.AddSeconds(1), $base.AddSeconds(2)) `
    -Statuses @((Status 'unavailable' 'loading'), (Status 'unavailable' 'loading'), (Status 'ready'))
Wait-EmbeddingModelReady -BaseUrl 'http://smoke-test' -TimeoutSeconds 5 -DaemonProcess $daemon
if ($script:polls -ne 3 -or $script:sleeps -ne 2) {
    throw "delayed readiness was not polled: polls=$script:polls sleeps=$script:sleeps"
}

Set-Scenario -Times @($base, $base, $base.AddSeconds(1)) `
    -Statuses @((Status 'unavailable' 'model missing'))
Assert-Fails {
    Wait-EmbeddingModelReady -BaseUrl 'http://smoke-test' -TimeoutSeconds 1 -DaemonProcess $daemon
} 'last_status='
if ($script:polls -ne 1 -or $script:sleeps -ne 1) {
    throw "unavailable state did not reach bounded timeout: polls=$script:polls sleeps=$script:sleeps"
}

Set-Scenario -Times @($base, $base) `
    -Statuses @([pscustomobject]@{ embedding_model = [pscustomobject]@{ reason = 'missing state' } })
Assert-Fails {
    Wait-EmbeddingModelReady -BaseUrl 'http://smoke-test' -TimeoutSeconds 5 -DaemonProcess $daemon
} 'malformed /api/status embedding_model.state'
if ($script:polls -ne 1 -or $script:sleeps -ne 0) { throw 'malformed status was retried or accepted' }

Set-Scenario -Times @($base, $base) -Statuses @((Status 'warming'))
Assert-Fails {
    Wait-EmbeddingModelReady -BaseUrl 'http://smoke-test' -TimeoutSeconds 5 -DaemonProcess $daemon
} "unknown /api/status embedding_model.state 'warming'"

$source = Get-Content -LiteralPath $smokePath -Raw
$readinessCall = $source.IndexOf('Wait-EmbeddingModelReady -BaseUrl')
$storeCall = $source.IndexOf('Write-Host "==> Store a memory"')
if ($readinessCall -lt 0 -or $storeCall -le $readinessCall) {
    throw 'embedding readiness must be checked before the semantic store'
}
if (-not $source.Contains('query = "blue lamp adjusts ocean timepieces"') -or
    -not $source.Contains('ORT_VECTOR_') -or
    -not $source.Contains('[regex]::Escape($Marker)') -or
    -not $source.Contains('vulkan-1.dll') -or
    -not $source.Contains('onnxruntime.dll') -or
    -not $source.Contains('Get-Process -Id $proc.Id -Module')) {
    throw 'semantic marker or native DLL proof was removed'
}
if (-not $source.Contains('Get-SmokeDiagnostics -DaemonProcess $proc')) {
    throw 'daemon log diagnostics are missing from the failure path'
}
$stdout = [IO.Path]::GetTempFileName()
$stderr = [IO.Path]::GetTempFileName()
try {
    Set-Content -LiteralPath $stdout -Value 'stdout tail'
    Set-Content -LiteralPath $stderr -Value 'stderr tail'
    $diagnostics = Get-SmokeDiagnostics -DaemonProcess $daemon -StdoutPath $stdout -StderrPath $stderr
    if (-not $diagnostics.Contains('stdout tail') -or -not $diagnostics.Contains('stderr tail')) {
        throw 'failure diagnostics omitted daemon logs'
    }
}
finally {
    Remove-Item -LiteralPath $stdout, $stderr -Force
}
Write-Host 'PASS: Windows embedding readiness waits, fails closed, and keeps semantic proof'
