# HintShell PowerShell Module
# Event-driven auto-suggest with overlay navigation via PSReadLine key bindings

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
    # 4. KEY BINDINGS - Event-driven
    # ========================================

    # --- Printable characters: insert + trigger overlay ---
    $hsCharHandler = {
        param($key, $arg)

        # FAST PATH: if in cooldown (paste/IDE detected), skip ALL checks
        if ([datetime]::Now -lt $script:HS.PasteUntil) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
            # Extend cooldown if more keys coming
            if ([Console]::KeyAvailable) {
                $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            }
            return
        }

        # Close overlay if visible
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }

        # Instant paste detection
        if ([Console]::KeyAvailable) {
            [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }

        [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)

        # Debounce: wait then check for more input (IDE/paste/IME)
        Start-Sleep -Milliseconds 100
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }

        # Check if IME modified the buffer
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        if ("$bufRef" -match '[^\x00-\x7F]') { return }

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

    # --- Spacebar ---
    Set-PSReadLineKeyHandler -Key Spacebar -ScriptBlock {
        if ([datetime]::Now -lt $script:HS.PasteUntil) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
            if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500) }
            return
        }
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
        if ([Console]::KeyAvailable) {
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
        Start-Sleep -Milliseconds 100
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
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
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        Start-Sleep -Milliseconds 100
        if ([Console]::KeyAvailable) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        Invoke-HSAutoSuggest
    }

    # --- UpArrow: navigate overlay or default history ---
    Set-PSReadLineKeyHandler -Key UpArrow -ScriptBlock {
        if ($script:HS.IsVisible) {
            $script:HS.SelectedIndex--
            Update-HSScroll
            Clear-HSOverlay
            Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
        } else {
            [Microsoft.PowerShell.PSConsoleReadLine]::PreviousHistory()
        }
    }

    # --- DownArrow: navigate overlay or default history ---
    Set-PSReadLineKeyHandler -Key DownArrow -ScriptBlock {
        if ($script:HS.IsVisible) {
            $script:HS.SelectedIndex++
            Update-HSScroll
            Clear-HSOverlay
            Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
        } else {
            [Microsoft.PowerShell.PSConsoleReadLine]::NextHistory()
        }
    }

    # --- Escape: close overlay or default revert ---
    Set-PSReadLineKeyHandler -Key Escape -ScriptBlock {
        if ($script:HS.IsVisible) {
            Clear-HSOverlay
            Reset-HSState
        } else {
            [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
        }
    }

    # --- Tab: accept suggestion OR trigger overlay ---
    Set-PSReadLineKeyHandler -Key Tab -ScriptBlock {
        if ($script:HS.IsVisible) {
            Clear-HSOverlay
            $sel = $script:HS.Suggestions[$script:HS.SelectedIndex].command
            Reset-HSState
            [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert($sel)
            return
        }
        Invoke-HSAutoSuggest
    }

    # --- Ctrl+Space: manual trigger ---
    Set-PSReadLineKeyHandler -Key Ctrl+Spacebar -ScriptBlock { Invoke-HSAutoSuggest }

    # --- Enter: execute command ---
    Set-PSReadLineKeyHandler -Key Enter -ScriptBlock {
        # Clear overlay state
        if ($script:HS.IsVisible) {
            $script:HS.OverlayLines = 0
            $script:HS.IsVisible = $false
        }
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
