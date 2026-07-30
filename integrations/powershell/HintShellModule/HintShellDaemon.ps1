# HintShellDaemon.ps1 - Named Pipe communication with HintShell Core
# Includes circuit breaker: after 3 consecutive failures, pause queries for 5s.

$script:HS_Circuit = @{
    FailCount    = 0
    MaxFails     = 3
    LastFailTime = [datetime]::MinValue
    CooldownMs   = 5000
}

$script:HS_StartLock = $null

function script:Test-HSDaemonAlive {
    <#
    .SYNOPSIS
    Returns $true if the daemon answers a status IPC request.
    #>
    param(
        [int]$TimeoutMs = 1200,
        [int]$Retries = 1
    )
    for ($attempt = 1; $attempt -le [Math]::Max(1, $Retries); $attempt++) {
        try {
            $pipe = [System.IO.Pipes.NamedPipeClientStream]::new(
                '.', 'hintshell', [System.IO.Pipes.PipeDirection]::InOut
            )
            $pipe.Connect($TimeoutMs)
            $json = (@{ action = 'status' } | ConvertTo-Json -Compress) + "`n"
            $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
            $pipe.Write($bytes, 0, $bytes.Length)
            $pipe.Flush()
            $reader = [System.IO.StreamReader]::new($pipe, [System.Text.Encoding]::UTF8)
            $line = $reader.ReadLine()
            $pipe.Dispose()
            if ([string]::IsNullOrWhiteSpace($line)) {
                if ($attempt -lt $Retries) { Start-Sleep -Milliseconds 150; continue }
                return $false
            }
            $obj = $line | ConvertFrom-Json
            return [bool]$obj.success
        } catch {
            if ($attempt -lt $Retries) { Start-Sleep -Milliseconds 150; continue }
            return $false
        }
    }
    return $false
}

function script:Get-HSDaemonProcessCount {
    @(Get-Process -Name 'hintshell-core' -ErrorAction SilentlyContinue).Count
}

function script:Stop-HSDaemonProcesses {
    param([switch]$Quiet)
    $procs = @(Get-Process -Name 'hintshell-core' -ErrorAction SilentlyContinue)
    if ($procs.Count -eq 0) { return 0 }
    $killed = 0
    foreach ($p in $procs) {
        try {
            Stop-Process -Id $p.Id -Force -ErrorAction Stop
            $killed++
        } catch {
            if (-not $Quiet) {
                Write-Host "   ⚠️ Failed to stop PID $($p.Id): $_" -ForegroundColor DarkYellow
            }
        }
    }
    Start-Sleep -Milliseconds 350
    $left = Get-HSDaemonProcessCount
    if ($left -gt 0 -and -not $Quiet) {
        Write-Host "   ⚠️ Still running after stop: $left process(es)" -ForegroundColor DarkYellow
    }
    return $killed
}

function script:Enter-HSStartLock {
    param([int]$TimeoutMs = 8000)
    $lockPath = Join-Path $env:USERPROFILE '.hintshell\start.lock'
    $dir = Split-Path -Parent $lockPath
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    while ($sw.ElapsedMilliseconds -lt $TimeoutMs) {
        try {
            $fs = [System.IO.File]::Open(
                $lockPath,
                [System.IO.FileMode]::OpenOrCreate,
                [System.IO.FileAccess]::ReadWrite,
                [System.IO.FileShare]::None
            )
            $script:HS_StartLock = $fs
            return $true
        } catch {
            # Another terminal is starting the daemon — wait, or bail if IPC already healthy.
            if (Test-HSDaemonAlive -TimeoutMs 400 -Retries 1) {
                return $false  # lock not held, but daemon already healthy
            }
            Start-Sleep -Milliseconds 120
        }
    }
    return $false
}

function script:Exit-HSStartLock {
    if ($null -ne $script:HS_StartLock) {
        try { $script:HS_StartLock.Dispose() } catch { }
        $script:HS_StartLock = $null
    }
}

function script:Resolve-HSCorePath {
    param(
        [string]$ModulePath,
        [string]$ConfigRoot
    )
    # Prefer bin/ (updated by cargo/hs init) over module/ (can lag behind)
    $candidates = @(
        (Join-Path $ConfigRoot 'bin\hintshell-core.exe'),
        (Join-Path $ModulePath 'hintshell-core.exe'),
        (Join-Path $ConfigRoot 'module\hintshell-core.exe')
    )
    foreach ($c in $candidates) {
        if (Test-Path -LiteralPath $c) { return $c }
    }
    return $null
}

function script:Resolve-HSCliPath {
    param(
        [string]$ModulePath,
        [string]$ConfigRoot
    )
    $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { 'hintshell.exe' } else { 'hintshell' }
    $candidates = @(
        (Join-Path $ConfigRoot "bin\$exeName"),
        (Join-Path $ModulePath $exeName),
        (Join-Path $ConfigRoot "module\$exeName")
    )
    foreach ($c in $candidates) {
        if (Test-Path -LiteralPath $c) { return $c }
    }
    return $null
}

function script:Invoke-HSDaemon {
    param([string]$Query, [int]$Limit = 8)

    # Circuit breaker: skip if too many recent failures
    $cb = $script:HS_Circuit
    if ($cb.FailCount -ge $cb.MaxFails) {
        $elapsed = ([datetime]::Now - $cb.LastFailTime).TotalMilliseconds
        if ($elapsed -lt $cb.CooldownMs) { return @() }
        # Cooldown expired, reset and retry
        $cb.FailCount = 0
    }

    try {
        $pipe  = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'hintshell', [System.IO.Pipes.PipeDirection]::InOut)
        $pipe.Connect(300)
        $cwd = (Get-Location).Path
        $json  = (@{ action = 'suggest'; input = $Query; limit = $Limit; cwd = $cwd; shell = 'powershell' } | ConvertTo-Json -Compress) + "`n"
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
        $pipe.Write($bytes, 0, $bytes.Length)
        $pipe.Flush()
        $reader = [System.IO.StreamReader]::new($pipe, [System.Text.Encoding]::UTF8)
        $line   = $reader.ReadLine()
        $pipe.Dispose()

        # Success: reset circuit breaker
        $cb.FailCount = 0

        if ($line) { return ($line | ConvertFrom-Json).suggestions }
    }
    catch {
        $cb.FailCount++
        $cb.LastFailTime = [datetime]::Now
    }
    return @()
}

function script:Invoke-HSRecord {
    param([string]$Command)
    try {
        $pipe  = [System.IO.Pipes.NamedPipeClientStream]::new('.', 'hintshell', [System.IO.Pipes.PipeDirection]::InOut)
        $pipe.Connect(300)
        $cwd = (Get-Location).Path
        $json  = (@{ action = 'add'; command = $Command; directory = $cwd; shell = 'powershell' } | ConvertTo-Json -Compress) + "`n"
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
        $pipe.Write($bytes, 0, $bytes.Length)
        $pipe.Flush()
        Start-Sleep -Milliseconds 50
        $pipe.Dispose()
    }
    catch { }
}