# HintShell PowerShell Module
# Event-driven auto-suggest with overlay navigation via PSReadLine key bindings

$ErrorActionPreference = 'SilentlyContinue'
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
        [switch]$Force
    )

    # Persistence check
    if ($Force) {
        if (Test-Path $disabledFile) { Remove-Item $disabledFile -Force }
    } elseif (Test-Path $disabledFile) {
        return
    }

    if (-not (Test-Path $configRoot)) { New-Item -ItemType Directory -Path $configRoot -Force | Out-Null }

    # 1. Start daemon if not running
    $pipeExists = Test-Path "\\.\pipe\hintshell"
    if (-not $pipeExists) {
        $corePath = Join-Path $modulePath "hintshell-core.exe"
        if (Test-Path $corePath) {
            Start-Process -FilePath $corePath -WindowStyle Hidden
            Write-Host "🚀 HintShell daemon started." -ForegroundColor Cyan
            Start-Sleep -Milliseconds 600
        } else {
            Write-Warning "hintshell-core.exe not found. Run: cargo build first."
            return
        }
    }

    # 2. Disable PSReadLine built-in prediction
    Set-PSReadLineOption -PredictionSource None

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

    Write-Host "✨ HintShell Real-time Auto-Suggest Active:" -ForegroundColor Cyan
    Write-Host "   Type anything    : Suggestions appear automatically" -ForegroundColor DarkGray
    Write-Host "   [Up/Down]        : Navigate list" -ForegroundColor DarkGray
    Write-Host "   [Tab]            : Accept  |  [Enter]: Run  |  [Esc]: Close" -ForegroundColor DarkGray
}

function Stop-HintShell {
    <#
    .SYNOPSIS
    Stop the HintShell daemon and disable auto-start.
    #>
    # Create persistent disable flag
    if (-not (Test-Path $configRoot)) { New-Item -ItemType Directory -Path $configRoot -Force | Out-Null }
    New-Item -ItemType File -Path $disabledFile -Force | Out-Null

    $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
    $cliPath = Join-Path $modulePath $exeName
    if (-not (Test-Path $cliPath)) { $cliPath = Join-Path $configRoot "bin\$exeName" }

    if (Test-Path $cliPath) {
        & $cliPath stop
    } else {
        Get-Process hintshell-core -ErrorAction SilentlyContinue | Stop-Process -Force
    }

    # Unbind ALL character keys back to SelfInsert
    foreach ($k in $script:HSBoundKeys) {
        try { Set-PSReadLineKeyHandler -Key $k -Function SelfInsert } catch {}
    }

    # Unbind special keys
    Set-PSReadLineKeyHandler -Key Tab -Function TabCompleteNext
    Set-PSReadLineKeyHandler -Key UpArrow -Function PreviousHistory
    Set-PSReadLineKeyHandler -Key DownArrow -Function NextHistory
    Set-PSReadLineKeyHandler -Key Backspace -Function BackwardDeleteChar
    Set-PSReadLineKeyHandler -Key Spacebar -Function SelfInsert
    Set-PSReadLineKeyHandler -Key Enter -Function AcceptLine
    Set-PSReadLineKeyHandler -Key Escape -Function RevertLine

    Write-Host "🛑 HintShell stopped and disabled. Start it again with 'hs start'" -ForegroundColor Yellow
}

function Get-HintShellStatus {
    $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
    $cliPath = Join-Path $modulePath $exeName
    if (-not (Test-Path $cliPath)) { $cliPath = Join-Path $configRoot "bin\$exeName" }
    
    if (Test-Path $disabledFile) {
        Write-Host "⏸️  HintShell UI is currently DISABLED (Run 'hs start' to enable)" -ForegroundColor DarkYellow
    } else {
        Write-Host "✨ HintShell UI is ACTIVE in this session" -ForegroundColor Cyan
    }

    if (Test-Path $cliPath) { 
        # Get status from daemon
        $statusOutput = & $cliPath status
        $statusOutput | Write-Host
        
        # Extract version from output (e.g. "🧠 HintShell Daemon v0.1.0-beta.4")
        $localVersion = $null
        foreach ($line in $statusOutput) {
            if ($line -match "v([0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9\.]+)?)$") {
                $localVersion = $matches[1]
                break
            }
        }

        # Check npm for latest beta version
        if ($localVersion) {
            try {
                # Setup silent web request to check latest beta tag
                $npmInfo = Invoke-RestMethod -Uri "https://registry.npmjs.org/hintshell" -UseBasicParsing -ErrorAction Ignore
                $latestVersion = $npmInfo.'dist-tags'.beta
                
                if ($latestVersion) {
                    if ($localVersion -ne $latestVersion) {
                        Write-Host ""
                        Write-Host "🆙 Update Available: $latestVersion (You have $localVersion)" -ForegroundColor Yellow
                        Write-Host "   Run 'hs update' to upgrade." -ForegroundColor Gray
                    } else {
                        Write-Host ""
                        Write-Host "✅ You are using the latest version." -ForegroundColor Green
                    }
                }
            } catch {
                # Silently ignore network errors during version check
            }
        }

    } else { 
        Write-Warning "hintshell binary not found." 
    }
}

function Invoke-HSWrapper {
    param(
        [Parameter(Position = 0)] [string]$Command,
        [Parameter(ValueFromRemainingArguments)] [string[]]$Args
    )

    switch ($Command) {
        'start' {
            Start-HintShell -Force
        }
        'stop' {
            Stop-HintShell
        }
        'status' {
            Get-HintShellStatus
        }
        'update' {
            Write-Host "🔄 Updating HintShell via npm..." -ForegroundColor Cyan
            try {
                npm install -g hintshell@beta
                Write-Host "📦 Re-initializing..." -ForegroundColor Cyan
                # Find the CLI binary from npm global path
                $npmGlobalBin = (npm root -g) -replace 'node_modules$', 'node_modules\hintshell\vendor'
                $exeInit = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
                $initPath = Join-Path $npmGlobalBin $exeInit
                if (Test-Path $initPath) {
                    & $initPath init
                } else {
                    Write-Host "⚠️ Could not find binary to run init. Please run 'hintshell init' manually." -ForegroundColor Yellow
                }
                # Restart daemon
                Write-Host "🔄 Restarting daemon..." -ForegroundColor Cyan
                Stop-HintShell
                Start-HintShell -Force
                Write-Host "✅ Update complete!" -ForegroundColor Green
            } catch {
                Write-Error "❌ Update failed. Do you have npm installed?"
            }
        }
        '--version' {
            $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
            $cliPath = Join-Path $modulePath $exeName
            if (-not (Test-Path $cliPath)) { $cliPath = Join-Path $configRoot "bin\$exeName" }
            if (Test-Path $cliPath) { & $cliPath --version } else { Write-Host "HintShell PowerShell Module Configured" }
        }
        '-v' {
            $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
            $cliPath = Join-Path $modulePath $exeName
            if (-not (Test-Path $cliPath)) { $cliPath = Join-Path $configRoot "bin\$exeName" }
            if (Test-Path $cliPath) { & $cliPath -v } else { Write-Host "HintShell PowerShell Module Configured" }
        }
        default {
            $exeName = if ([Environment]::OSVersion.Platform -eq 'Win32NT') { "hintshell.exe" } else { "hintshell" }
            $cliPath = Join-Path $modulePath $exeName
            if (-not (Test-Path $cliPath)) { $cliPath = Join-Path $configRoot "bin\$exeName" }
            
            # Filter out empty args to avoid 'unexpected argument' error
            $cleanArgs = @()
            if ($Args) { $cleanArgs = @($Args | Where-Object { $_ -ne '' -and $null -ne $_ }) }

            if (Test-Path $cliPath) {
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
