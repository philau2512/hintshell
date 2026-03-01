# HintShell PowerShell Module
# Real-time auto-suggest with Claude-style minimal overlay

$ErrorActionPreference = 'SilentlyContinue'
$modulePath = $PSScriptRoot

function Start-HintShell {
    <#
    .SYNOPSIS
    Start HintShell daemon and activate real-time auto-suggest overlay.
    #>

    # 1. Load scripts
    . (Join-Path $modulePath "HintShellDaemon.ps1")
    . (Join-Path $modulePath "HintShellOverlay.ps1")
    . (Join-Path $modulePath "HintShellHandlers.ps1")
    # Write-Host "🧠 HintShell overlay loaded!" -ForegroundColor Green

    # 2. Start daemon if not running
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

    # 3. Disable PSReadLine built-in prediction
    Set-PSReadLineOption -PredictionSource None

    # ========================================
    # 4. KEY BINDINGS - Real-time auto-suggest
    # ========================================

    # --- Printable characters: insert + trigger overlay ---
    $hsCharHandler = {
        param($key, $arg)
        # Paste detection: if many keys are buffered, set cooldown and bail
        if ([Console]::KeyAvailable) {
            [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }

        [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)

        # Brief wait then recheck for delayed paste burst
        Start-Sleep -Milliseconds 30
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }

        # Skip if within paste cooldown
        if ([datetime]::Now -lt $script:HS.PasteUntil) { return }

        Invoke-HSAutoSuggest
    }

    # Lowercase letters
    foreach ($c in [char[]]([char]'a'..[char]'z')) {
        Set-PSReadLineKeyHandler -Key ([string]$c) -ScriptBlock $hsCharHandler
    }
    # Uppercase letters
    foreach ($c in [char[]]([char]'a'..[char]'z')) {
        Set-PSReadLineKeyHandler -Key "Shift+$c" -ScriptBlock $hsCharHandler
    }
    # Digits
    foreach ($c in [char[]]([char]'0'..[char]'9')) {
        Set-PSReadLineKeyHandler -Key ([string]$c) -ScriptBlock $hsCharHandler
    }
    # Common symbols used in commands
    foreach ($c in @('-', '.', '/', '\', '_', ':', '=', ',', ';', '+', '*', '~', '@', '!', '"', "'")) {
        Set-PSReadLineKeyHandler -Key $c -ScriptBlock $hsCharHandler
    }

    # --- Spacebar: insert space + trigger overlay ---
    Set-PSReadLineKeyHandler -Key Spacebar -ScriptBlock {
        [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        Start-Sleep -Milliseconds 30
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        if ([datetime]::Now -lt $script:HS.PasteUntil) { return }
        Invoke-HSAutoSuggest
    }

    # --- Backspace: delete char + trigger overlay ---
    Set-PSReadLineKeyHandler -Key Backspace -ScriptBlock {
        [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        Start-Sleep -Milliseconds 30
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        if ([datetime]::Now -lt $script:HS.PasteUntil) { return }
        Invoke-HSAutoSuggest
    }

    # --- Tab: accept ghost text / suggestion OR trigger overlay ---
    Set-PSReadLineKeyHandler -Key Tab -ScriptBlock {
        if ($script:HS.IsVisible -and $script:HS.OverlayLines -eq -1) {
            # Ghost text mode: accept top suggestion
            Clear-HSOverlay
            $sel = $script:HS.Suggestions[0].command
            Reset-HSState
            [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert($sel)
            return
        }
        Invoke-HSAutoSuggest
    }

    # --- Ctrl+Space: manual trigger ---
    Set-PSReadLineKeyHandler -Key Ctrl+Spacebar -ScriptBlock { Invoke-HSAutoSuggest }

    # --- Enter: reset state + execute (don't clear overlay to avoid prompt corruption) ---
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
        # Reset overlay state silently (output will overwrite it)
        $script:HS.OverlayLines = 0
        $script:HS.IsVisible = $false
        Reset-HSState

        # Get command before accepting
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $cmd = "$bufRef"

        # Accept and execute
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()

        # Record to history
        if (-not [string]::IsNullOrWhiteSpace($cmd)) {
            Invoke-HSRecord -Command $cmd
        }
    }

    Write-Host "✨ HintShell Real-time Auto-Suggest Active:" -ForegroundColor Cyan
    Write-Host "   Type anything    : Suggestions appear automatically" -ForegroundColor DarkGray
    Write-Host "   [Up/Down]        : Navigate list" -ForegroundColor DarkGray
    Write-Host "   [Tab]            : Accept  |  [Enter]: Run  |  [Esc]: Close" -ForegroundColor DarkGray
}

function Stop-HintShell {
    <#
    .SYNOPSIS
    Stop the HintShell daemon.
    #>
    $cliPath = Join-Path $modulePath "hintshell.exe"
    if (Test-Path $cliPath) {
        & $cliPath stop
    } else {
        Write-Warning "hintshell.exe not found."
    }
}

function Get-HintShellStatus {
    <#
    .SYNOPSIS
    Show HintShell daemon status.
    #>
    $cliPath = Join-Path $modulePath "hintshell.exe"
    if (Test-Path $cliPath) {
        & $cliPath status
    } else {
        Write-Warning "hintshell.exe not found."
    }
}

Export-ModuleMember -Function Start-HintShell, Stop-HintShell, Get-HintShellStatus
