param(
    [string]$ExePath = "target\release\wenlan-server.exe",
    [int]$HealthTimeoutSeconds = 60,
    [int]$Port = 0
)

$ErrorActionPreference = "Stop"

$trackedEnvironment = @('WENLAN_BIND_ADDR', 'WENLAN_PORT', 'WENLAN_DATA_DIR', 'WENLAN_NO_AUTOSTART', 'ORT_DYLIB_PATH')
$previousEnvironment = @{}
foreach ($name in $trackedEnvironment) { $previousEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }

if (-not (Test-Path -LiteralPath $ExePath)) {
    throw "expected $ExePath to exist; build wenlan-server first"
}

$ExePath = (Resolve-Path -LiteralPath $ExePath).Path
$ExeDirectory = Split-Path -Parent $ExePath
$ExpectedOrtDll = (Resolve-Path -LiteralPath (Join-Path $ExeDirectory "onnxruntime.dll")).Path
$ExpectedVulkanLoader = (
    Resolve-Path -LiteralPath (Join-Path $ExeDirectory "vulkan-1.dll")
).Path
$Marker = "ORT_VECTOR_$([System.Guid]::NewGuid().ToString('N'))"
$DataDir = $null
$proc = $null
$reservation = $null
$StdoutLog = $null
$StderrLog = $null

try {
    $DataDir = New-Item -ItemType Directory -Path "$env:TEMP\origin-smoke-$([System.Guid]::NewGuid())"
    $reservation = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Port)
    $reservation.Start()
    $Port = $reservation.LocalEndpoint.Port
    $pages = Join-Path $DataDir.FullName 'pages'
    [IO.Directory]::CreateDirectory($pages) | Out-Null
    [IO.File]::WriteAllText((Join-Path $DataDir.FullName 'config.json'), (@{knowledge_path = $pages} | ConvertTo-Json))
    $env:WENLAN_NO_AUTOSTART = '1'
    $env:WENLAN_PORT = "$Port"
    $env:WENLAN_BIND_ADDR = "127.0.0.1:$Port"
    $env:WENLAN_DATA_DIR = $DataDir.FullName
    Remove-Item Env:ORT_DYLIB_PATH -ErrorAction SilentlyContinue
    $StdoutLog = Join-Path $DataDir.FullName "wenlan-server.stdout.log"
    $StderrLog = Join-Path $DataDir.FullName "wenlan-server.stderr.log"

    Write-Host "==> Starting daemon"
    $reservation.Stop()
    $proc = Start-Process -FilePath $ExePath -PassThru -WindowStyle Hidden `
        -RedirectStandardOutput $StdoutLog -RedirectStandardError $StderrLog
    $null = $proc.Handle

    Write-Host "==> Waiting for /api/health"
    $healthy = $false
    $deadline = [DateTimeOffset]::UtcNow.AddSeconds($HealthTimeoutSeconds)
    while ([DateTimeOffset]::UtcNow -lt $deadline) {
        if ($proc.HasExited) {
            break
        }
        try {
            $resp = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/api/health" -UseBasicParsing -TimeoutSec 1
            if ($resp.StatusCode -eq 200) {
                $healthy = $true
                break
            }
        }
        catch {}
        Start-Sleep -Seconds 1
    }
    if (-not $healthy) {
        $exit = if ($proc.HasExited) { "exit=$($proc.ExitCode)" } else { "still-running" }
        $stdout = if (Test-Path -LiteralPath $StdoutLog) { (Get-Content -LiteralPath $StdoutLog -Tail 40) -join "`n" } else { "<missing>" }
        $stderr = if (Test-Path -LiteralPath $StderrLog) { (Get-Content -LiteralPath $StderrLog -Tail 40) -join "`n" } else { "<missing>" }
        throw "daemon did not become healthy ($exit)`nstdout:`n$stdout`nstderr:`n$stderr"
    }

    $LoadedVulkanModules = @(
        Get-Process -Id $proc.Id -Module |
            Where-Object { $_.ModuleName -ieq "vulkan-1.dll" } |
            ForEach-Object { (Resolve-Path -LiteralPath $_.FileName).Path }
    )
    if ($LoadedVulkanModules.Count -ne 1) {
        throw "expected exactly one loaded vulkan-1.dll, got $($LoadedVulkanModules -join ', ')"
    }
    if (-not [string]::Equals(
        $LoadedVulkanModules[0],
        $ExpectedVulkanLoader,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw (
            "loaded unexpected vulkan-1.dll: expected " +
            "$ExpectedVulkanLoader, got $($LoadedVulkanModules[0])"
        )
    }
    Write-Host "    loaded exact Vulkan loader: $($LoadedVulkanModules[0])"

    Write-Host "==> Store a memory"
    $knowledge = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/knowledge/path"
    $runtime = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/health"
    if (-not $runtime.data_root -or [IO.Path]::GetFullPath($runtime.data_root) -ine [IO.Path]::GetFullPath($DataDir.FullName)) {
        throw 'daemon is not using the scratch data directory'
    }
    if ([IO.Path]::GetFullPath($knowledge.path) -ine [IO.Path]::GetFullPath($pages)) {
        throw "daemon is not using the scratch knowledge directory"
    }
    if ($proc.HasExited) { throw 'scratch daemon exited before the store request' }
    try {
        $StoreBody = @{
            content = "$Marker A cobalt lantern calibrates tidal clocks during winter."
            memory_type = "lesson"
        } | ConvertTo-Json
        $store = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/memory/store" -Method POST `
            -ContentType "application/json" `
            -Body $StoreBody
        Write-Host "    store ok: $($store | ConvertTo-Json -Compress -Depth 5)"
        if ([int]$store.chunks_created -lt 1) {
            throw "store returned no embedded chunks"
        }
    }
    catch {
        # ErrorDetails.Message works on both pwsh 5 (WebException) and pwsh 7
        # (HttpResponseMessage); pwsh 7 dropped GetResponseStream().
        $body = $_.ErrorDetails.Message
        $status = $_.Exception.Response.StatusCode.value__
        throw "store failed: status=$status body=$body"
    }

    Write-Host "==> Semantic search with no lexical overlap"
    $SearchBody = @{
        query = "blue lamp adjusts ocean timepieces"
        limit = 3
    } | ConvertTo-Json
    $search = Invoke-RestMethod -Uri "http://127.0.0.1:$Port/api/memory/search" -Method POST `
        -ContentType "application/json" `
        -Body $SearchBody

    if (($search | ConvertTo-Json -Depth 10) -notmatch [regex]::Escape($Marker)) {
        throw "semantic search did not return the vector-only marker $Marker"
    }

    $LoadedOrtModules = @(
        Get-Process -Id $proc.Id -Module |
            Where-Object { $_.ModuleName -ieq "onnxruntime.dll" } |
            ForEach-Object { (Resolve-Path -LiteralPath $_.FileName).Path }
    )
    if ($LoadedOrtModules.Count -ne 1) {
        throw "expected exactly one loaded onnxruntime.dll, got $($LoadedOrtModules -join ', ')"
    }
    if (-not [string]::Equals(
        $LoadedOrtModules[0],
        $ExpectedOrtDll,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "loaded unexpected onnxruntime.dll: expected $ExpectedOrtDll, got $($LoadedOrtModules[0])"
    }
    Write-Host "    loaded exact ORT module: $($LoadedOrtModules[0])"
    Write-Host "==> PASS"
}
finally {
    if ($reservation) { $reservation.Stop() }
    if ($proc -and -not $proc.HasExited) {
        $proc.Kill()
        if (-not $proc.WaitForExit(5000)) { throw 'scratch daemon did not stop' }
    }
    # SQLite WAL -shm file can stay locked briefly after daemon exit on Windows.
    # Retry deletion a few times before giving up; don't fail the smoke on cleanup.
    if ($DataDir) {
        $safeData = [IO.Path]::GetFullPath($DataDir.FullName)
        $tempPrefix = [IO.Path]::GetFullPath($env:TEMP).TrimEnd('\') + '\'
        if (-not $safeData.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) { throw 'unsafe smoke cleanup' }
        for ($i = 0; $i -lt 5; $i++) {
            Remove-Item -LiteralPath $safeData -Recurse -Force -ErrorAction SilentlyContinue
            if (-not (Test-Path -LiteralPath $safeData)) {
                break
            }
            Start-Sleep -Milliseconds 500
        }
    }
    foreach ($name in $trackedEnvironment) { [Environment]::SetEnvironmentVariable($name, $previousEnvironment[$name], 'Process') }
}
