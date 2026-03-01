# HintShellDaemon.ps1 - Named Pipe communication with HintShell Core
# Includes circuit breaker: after 3 consecutive failures, pause queries for 5s.

$script:HS_Circuit = @{
    FailCount    = 0
    MaxFails     = 3
    LastFailTime = [datetime]::MinValue
    CooldownMs   = 5000
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
        $pipe.Connect(50)
        $json  = (@{ action = 'suggest'; input = $Query; limit = $Limit } | ConvertTo-Json -Compress) + "`n"
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
        $json  = (@{ action = 'add'; command = $Command; shell = 'powershell' } | ConvertTo-Json -Compress) + "`n"
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
        $pipe.Write($bytes, 0, $bytes.Length)
        $pipe.Flush()
        Start-Sleep -Milliseconds 50
        $pipe.Dispose()
    }
    catch { }
}
