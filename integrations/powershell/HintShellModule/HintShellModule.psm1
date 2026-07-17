# HintShell PowerShell Module
# Event-driven auto-suggest with overlay navigation via PSReadLine key bindings

# Do NOT set global SilentlyContinue — it hides start/status failures in IDE terminals.
$modulePath = $PSScriptRoot
$configRoot = Join-Path $env:USERPROFILE ".hintshell"
$disabledFile = Join-Path $configRoot ".disabled"

# Load sub-scripts immediately
. (Join-Path $modulePath "HintShellDaemon.ps1")
. (Join-Path $modulePath "HintShellOverlay.ps1")
. (Join-Path $modulePath "HintShellHandlers.ps1")

# Track which keys we bind so Stop can unbind them all
$script:HSBoundKeys = @()

function Start-HintShell {
    <#
    .SYNOPSIS
    Initialize HintShell integration and start the daemon.
    #>
    param(
        [switch]$Force,
        [switch]$Quiet
    )

    # Persistence check
    if ($Force) {
        if (Test-Path $disabledFile) {
            Remove-Item $disabledFile -Force -ErrorAction SilentlyContinue
        }
    } elseif (Test-Path $disabledFile) {
        if (-not $Quiet) {
            Write-Host "⏸️  HintShell is disabled. Run 'hs start' to enable." -ForegroundColor DarkYellow
        }
        return
    }

    if (-not (Test-Path $configRoot)) {
        New-Item -ItemType Directory -Path $configRoot -Force | Out-Null
    }

    # 1. Ensure ONE healthy daemon (IPC), with cross-terminal lock to stop IDE races
    $alive = Test-HSDaemonAlive -TimeoutMs 800 -Retries 2
    if ($alive) {
        if ($Force -and -not $Quiet) {
            Write-Host "✅ Daemon already running and healthy." -ForegroundColor Green
        }
    } else {
        $gotLock = Enter-HSStartLock -TimeoutMs 8000
        try {
            # Re-check after lock (or while waiting for another terminal's start)
            if (Test-HSDaemonAlive -TimeoutMs 1000 -Retries 2) {
                if ($Force -and -not $Quiet) {
                    Write-Host "✅ Daemon became healthy (started by another session)." -ForegroundColor Green
                }
            } else {
                $orphanCount = Get-HSDaemonProcessCount
                if ($orphanCount -gt 0) {
                    if (-not $Quiet) {
                        Write-Host "🧹 Found $orphanCount process(es) not answering IPC; waiting briefly..." -ForegroundColor Yellow
                    }
                    $becameReady = $false
                    for ($w = 1; $w -le 8; $w++) {
                        Start-Sleep -Milliseconds 200
                        if (Test-HSDaemonAlive -TimeoutMs 600 -Retries 1) {
                            $becameReady = $true
                            if (-not $Quiet) {
                                Write-Host "✅ Daemon became healthy while waiting (~$($w * 200)ms)." -ForegroundColor Green
                            }
                            break
                        }
                    }
                    if (-not $becameReady) {
                        if (-not $Quiet) {
                            Write-Host "   Still unhealthy — stopping stale process(es)..." -ForegroundColor DarkGray
                        }
                        $killed = Stop-HSDaemonProcesses -Quiet:$Quiet
                        if (-not $Quiet) {
                            Write-Host "   Stopped $killed process(es)." -ForegroundColor DarkGray
                        }
                    }
                }

                if (-not (Test-HSDaemonAlive -TimeoutMs 600 -Retries 1)) {
                    $corePath = Resolve-HSCorePath -ModulePath $modulePath -ConfigRoot $configRoot
                    if (-not $corePath) {
                        Write-Host "❌ hintshell-core.exe not found in ~/.hintshell/bin or module." -ForegroundColor Red
                        Write-Host "   Run: cargo build --release && hs init   (or reload-hintshell.ps1)" -ForegroundColor DarkGray
                    } else {
                        if (-not $Quiet) {
                            Write-Host "🚀 Starting daemon: $corePath" -ForegroundColor Cyan
                        }
                        try {
                            $proc = Start-Process -FilePath $corePath -WindowStyle Hidden -PassThru -ErrorAction Stop
                            if (-not $Quiet) {
                                Write-Host "   Spawned PID $($proc.Id)" -ForegroundColor DarkGray
                            }
                        } catch {
                            Write-Host "❌ Failed to start daemon: $_" -ForegroundColor Red
                        }

                        $ready = $false
                        for ($i = 1; $i -le 20; $i++) {
                            Start-Sleep -Milliseconds 200
                            if (Test-HSDaemonAlive -TimeoutMs 800 -Retries 1) {
                                $ready = $true
                                if (-not $Quiet) {
                                    Write-Host "✅ Daemon started successfully (ready after ~$($i * 200)ms)." -ForegroundColor Green
                                }
                                break
                            }
                        }
                        if (-not $ready) {
                            Write-Host "❌ Daemon process started but IPC is not ready." -ForegroundColor Red
                            Write-Host "   Pipe: \\.\pipe\hintshell  |  Processes: $(Get-HSDaemonProcessCount)" -ForegroundColor DarkGray
                            Write-Host "   Core: $corePath" -ForegroundColor DarkGray
                            Write-Host "   Tip: hs stop ; hs start" -ForegroundColor DarkGray
                        }
                    }
                }
            }
        } finally {
            if ($gotLock) { Exit-HSStartLock }
        }
    }

    # 2. Disable PSReadLine built-in prediction
    try { Set-PSReadLineOption -PredictionSource None -ErrorAction SilentlyContinue } catch { }

    # 3. Key Bindings
    $script:HSBoundKeys = @()

    # --- Char handler (a-z, 0-9, symbols) ---
    $hsCharHandler = {
        param($key, $arg)
        if ([datetime]::Now -lt $script:HS.PasteUntil) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
            if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500) }
            return
        }
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
        if ([Console]::KeyAvailable) {
            [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
        Start-Sleep -Milliseconds 100
        if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500); return }
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        if ("$bufRef" -match '[^\x00-\x7F]') { return }
        Invoke-HSAutoSuggest
    }

    foreach ($c in [char[]]([char]'a'..[char]'z')) {
        Set-PSReadLineKeyHandler -Key ([string]$c) -ScriptBlock $hsCharHandler
        $script:HSBoundKeys += ([string]$c)
    }
    foreach ($c in [char[]]([char]'a'..[char]'z')) {
        Set-PSReadLineKeyHandler -Key "Shift+$c" -ScriptBlock $hsCharHandler
        $script:HSBoundKeys += "Shift+$c"
    }
    foreach ($c in [char[]]([char]'0'..[char]'9')) {
        Set-PSReadLineKeyHandler -Key ([string]$c) -ScriptBlock $hsCharHandler
        $script:HSBoundKeys += ([string]$c)
    }
    foreach ($c in @('-', '.', '/', '\', '_', ':', '=', ',', ';', '+', '*', '~', '@', '!', '"', "'")) {
        Set-PSReadLineKeyHandler -Key $c -ScriptBlock $hsCharHandler
        $script:HSBoundKeys += $c
    }

    # --- Spacebar ---
    Set-PSReadLineKeyHandler -Key Spacebar -ScriptBlock {
        if ([datetime]::Now -lt $script:HS.PasteUntil) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
            if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500) }
            return
        }
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
        [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
        Start-Sleep -Milliseconds 100
        if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500); return }
        Invoke-HSAutoSuggest
    }

    # --- Backspace ---
    Set-PSReadLineKeyHandler -Key Backspace -ScriptBlock {
        if ([datetime]::Now -lt $script:HS.PasteUntil) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
            if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500) }
            return
        }
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
        [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
        Start-Sleep -Milliseconds 80
        if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500); return }
        Invoke-HSAutoSuggest
    }

    # --- Enter ---
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
        if ($script:HS.IsVisible) { Clear-HSOverlay }
        Reset-HSState
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $cmd = "$bufRef"
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        if (-not [string]::IsNullOrWhiteSpace($cmd)) { Invoke-HSRecord -Command $cmd }
    }

    # --- Ctrl+Space: manual trigger ---
    Set-PSReadLineKeyHandler -Key Ctrl+Spacebar -ScriptBlock { Invoke-HSAutoSuggest }

    # --- Up Arrow ---
    Set-PSReadLineKeyHandler -Key UpArrow -ScriptBlock {
        if ($script:HS.IsVisible) {
            $script:HS.SelectedIndex--
            Update-HSScroll
            Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::PreviousHistory()
    }

    # --- Down Arrow ---
    Set-PSReadLineKeyHandler -Key DownArrow -ScriptBlock {
        if ($script:HS.IsVisible) {
            $script:HS.SelectedIndex++
            Update-HSScroll
            Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::NextHistory()
    }

    # --- Tab: accept suggestion ---
    Set-PSReadLineKeyHandler -Key Tab -ScriptBlock {
        if ($script:HS.IsVisible) {
            $sel = $script:HS.Suggestions[$script:HS.SelectedIndex].command
            Clear-HSOverlay; Reset-HSState
            [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert($sel)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::TabCompleteNext()
    }

    # --- Escape: close overlay ---
    Set-PSReadLineKeyHandler -Key Escape -ScriptBlock {
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState; return }
        [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
    }

    # Write-Host "✨ HintShell Real-time Auto-Suggest Active:" -ForegroundColor Cyan
    # Write-Host "   Type anything    : Suggestions appear automatically" -ForegroundColor DarkGray
    # Write-Host "   [Up/Down]        : Navigate list" -ForegroundColor DarkGray
    # Write-Host "   [Tab]            : Accept  |  [Enter]: Run  |  [Esc]: Close" -ForegroundColor DarkGray
}

function Stop-HintShell {
    <#
    .SYNOPSIS
    Stop the HintShell daemon and disable auto-start.
    #>
    Write-Host "▶ hs stop" -ForegroundColor Cyan
    # Create persistent disable flag
    if (-not (Test-Path $configRoot)) { New-Item -ItemType Directory -Path $configRoot -Force | Out-Null }
    New-Item -ItemType File -Path $disabledFile -Force | Out-Null

    $cliPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot

    if ($cliPath) {
        Write-Host "🛑 Stopping daemon via CLI: $cliPath" -ForegroundColor Yellow
        & $cliPath stop
    } else {
        $killed = Stop-HSDaemonProcesses
        Write-Host "🛑 Stopped $killed hintshell-core process(es) (CLI not found)." -ForegroundColor Yellow
    }

    # Unbind ALL character keys back to SelfInsert
    foreach ($k in $script:HSBoundKeys) {
        try { Set-PSReadLineKeyHandler -Key $k -Function SelfInsert -ErrorAction SilentlyContinue } catch {}
    }

    # Unbind special keys
    try {
        Set-PSReadLineKeyHandler -Key Tab -Function TabCompleteNext -ErrorAction SilentlyContinue
        Set-PSReadLineKeyHandler -Key UpArrow -Function PreviousHistory -ErrorAction SilentlyContinue
        Set-PSReadLineKeyHandler -Key DownArrow -Function NextHistory -ErrorAction SilentlyContinue
        Set-PSReadLineKeyHandler -Key Backspace -Function BackwardDeleteChar -ErrorAction SilentlyContinue
        Set-PSReadLineKeyHandler -Key Spacebar -Function SelfInsert -ErrorAction SilentlyContinue
        Set-PSReadLineKeyHandler -Key Enter -Function AcceptLine -ErrorAction SilentlyContinue
        Set-PSReadLineKeyHandler -Key Escape -Function RevertLine -ErrorAction SilentlyContinue
    } catch {}

    Write-Host "✅ HintShell stopped and disabled. Start it again with 'hs start'" -ForegroundColor Yellow
}

function Get-HintShellStatus {
    Write-Host "▶ hs status" -ForegroundColor Cyan

    $cliPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot
    $corePath = Resolve-HSCorePath -ModulePath $modulePath -ConfigRoot $configRoot
    $procCount = Get-HSDaemonProcessCount
    $alive = Test-HSDaemonAlive -TimeoutMs 1200 -Retries 2

    if (Test-Path $disabledFile) {
        Write-Host "⏸️  HintShell UI is currently DISABLED (Run 'hs start' to enable)" -ForegroundColor DarkYellow
    } else {
        Write-Host "✨ HintShell UI is ACTIVE in this session" -ForegroundColor Cyan
    }

    Write-Host "   Module : $modulePath" -ForegroundColor DarkGray
    Write-Host "   CLI    : $(if ($cliPath) { $cliPath } else { '(not found)' })" -ForegroundColor DarkGray
    Write-Host "   Core   : $(if ($corePath) { $corePath } else { '(not found)' })" -ForegroundColor DarkGray
    Write-Host "   Processes hintshell-core: $procCount" -ForegroundColor DarkGray
    Write-Host "   IPC alive: $alive" -ForegroundColor DarkGray

    if ($alive) {
        Write-Host "✅ Daemon is running and answering IPC." -ForegroundColor Green
    } else {
        Write-Host "❌ Daemon is not running (IPC failed)." -ForegroundColor Red
        if ($procCount -gt 0) {
            Write-Host "   ⚠️ $procCount process(es) exist but are NOT healthy — run: hs start" -ForegroundColor Yellow
        } else {
            Write-Host "   Run: hs start" -ForegroundColor DarkGray
        }
    }

    if ($cliPath) {
        Write-Host "--- CLI status ---" -ForegroundColor DarkGray
        & $cliPath status
    } elseif (-not $alive) {
        Write-Warning "hintshell binary not found in ~/.hintshell/bin or module."
    }
}

function Invoke-HSWrapper {
    param(
        [Parameter(Position = 0)] [string]$Command,
        [Parameter(ValueFromRemainingArguments)] [string[]]$Args
    )

    switch ($Command) {
        'start' {
            Write-Host "▶ hs start" -ForegroundColor Cyan
            Start-HintShell -Force
            $alive = Test-HSDaemonAlive -TimeoutMs 1200 -Retries 2
            $procs = Get-HSDaemonProcessCount
            if ($alive) {
                Write-Host "✅ hs start complete — daemon healthy (processes=$procs)." -ForegroundColor Green
            } else {
                Write-Host "❌ hs start finished but daemon is still NOT healthy (processes=$procs)." -ForegroundColor Red
            }
        }
        'stop' {
            Stop-HintShell
        }
        'status' {
            Get-HintShellStatus
        }
        'update' {
            Write-Host "▶ hs update" -ForegroundColor Cyan
            try {
                # 1. Stop daemon FIRST to release file locks on Windows
                Write-Host "🛑 Stopping daemon (release file locks)..." -ForegroundColor Yellow
                $cliPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot
                if ($cliPath) {
                    & $cliPath stop
                } else {
                    $killed = Stop-HSDaemonProcesses
                    Write-Host "   Stopped $killed process(es) (CLI not found)." -ForegroundColor DarkGray
                }
                # Do not create .disabled flag here — user wants update, not disable

                # 2. Update via npm (postinstall also stops daemon + runs init)
                Write-Host "🔄 npm install -g hintshell@latest ..." -ForegroundColor Cyan
                npm install -g hintshell@latest
                if ($LASTEXITCODE -ne 0) {
                    Write-Host "❌ npm install failed (exit $LASTEXITCODE)." -ForegroundColor Red
                    Write-Host "   Tip: taskkill /F /IM hintshell-core.exe ; npm i -g hintshell@latest" -ForegroundColor DarkGray
                    return
                }
                Write-Host "✅ npm install finished." -ForegroundColor Green

                # 3. Ensure module + daemon (init may already have run in postinstall)
                Write-Host "📦 Ensuring local install (hintshell init)..." -ForegroundColor Cyan
                $npmVendor = Join-Path ((npm root -g) | ForEach-Object { $_ }) "hintshell\vendor"
                $exeInit = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
                $initPath = Join-Path $npmVendor $exeInit
                if (-not (Test-Path $initPath)) {
                    $initPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot
                }

                if ($initPath -and (Test-Path $initPath)) {
                    & $initPath init
                } else {
                    Write-Host "⚠️ Could not find binary to run init. Please run 'hintshell init' manually." -ForegroundColor Yellow
                }

                # 4. Reload module from ~/.hintshell and restart UI
                Write-Host "🔄 Reloading module + starting daemon..." -ForegroundColor Cyan
                $freshModule = Join-Path $configRoot "module\HintShellModule.psd1"
                if (Test-Path $freshModule) {
                    Remove-Module HintShellModule -Force -ErrorAction SilentlyContinue
                    Import-Module $freshModule -Force -DisableNameChecking -ErrorAction SilentlyContinue
                }
                Start-HintShell -Force
                if (Test-HSDaemonAlive -TimeoutMs 1200 -Retries 2) {
                    Write-Host "✅ hs update complete — daemon healthy." -ForegroundColor Green
                } else {
                    Write-Host "⚠️ Update installed but daemon not healthy. Run: hs start" -ForegroundColor Yellow
                }
            } catch {
                Write-Error "❌ Update failed: $_"
            }
        }
        '--version' {
            $cliPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot
            if ($cliPath) { & $cliPath --version } else { Write-Host "HintShell PowerShell Module Configured" }
        }
        '-v' {
            $cliPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot
            if ($cliPath) { & $cliPath -v } else { Write-Host "HintShell PowerShell Module Configured" }
        }
        default {
            $cliPath = Resolve-HSCliPath -ModulePath $modulePath -ConfigRoot $configRoot
            $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }

            # Filter out empty args to avoid 'unexpected argument' error
            $cleanArgs = @()
            if ($Args) { $cleanArgs = @($Args | Where-Object { $_ -ne '' -and $null -ne $_ }) }

            if ($cliPath) {
                if ($Command) {
                    if ($cleanArgs.Count -gt 0) { & $cliPath $Command @cleanArgs } else { & $cliPath $Command }
                } else { & $cliPath }
            } else {
                Write-Warning "HintShell binary not found locally."
                if ($Command) {
                    if ($cleanArgs.Count -gt 0) { & $exeName $Command @cleanArgs } else { & $exeName $Command }
                } else { & $exeName }
            }
        }
    }
}

function hs {
    param(
        [Parameter(Position = 0)] [string]$Command,
        [Parameter(ValueFromRemainingArguments)] [string[]]$ArgsArr
    )
    if ($Command) { Invoke-HSWrapper $Command @ArgsArr } else { Invoke-HSWrapper }
}

function hintshell {
    param(
        [Parameter(Position = 0)] [string]$Command,
        [Parameter(ValueFromRemainingArguments)] [string[]]$ArgsArr
    )
    if ($Command) { Invoke-HSWrapper $Command @ArgsArr } else { Invoke-HSWrapper }
}

Export-ModuleMember -Function Start-HintShell, Stop-HintShell, Get-HintShellStatus, Invoke-HSWrapper, hs, hintshell
