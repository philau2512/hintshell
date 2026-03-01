# HintShellHandlers.ps1 - Event-driven Auto-Suggest Handler
# No ReadKey loop! All navigation handled by PSReadLine key bindings.

function global:Invoke-HSAutoSuggest {
    if ($script:HS.IsActive) { return }
    if ([datetime]::Now -lt $script:HS.PasteUntil) { return }
    $script:HS.IsActive = $true

    try {
        # Read current buffer
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $typed = "$bufRef"

        if ([string]::IsNullOrWhiteSpace($typed)) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            return
        }

        # Skip if input contains non-ASCII (Vietnamese IME, etc.)
        if ($typed -match '[^\x00-\x7F]') {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            return
        }

        # Query daemon for suggestions
        $suggestions = Get-HSSuggestions -Typed $typed
        if (-not $suggestions -or $suggestions.Count -eq 0) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            return
        }

        # Store state and render overlay
        $script:HS.Suggestions   = $suggestions
        $script:HS.SelectedIndex = 0
        $script:HS.ScrollOffset  = 0
        $script:HS.CurrentInput  = $typed

        Update-HSScroll
        Draw-HSOverlay -Suggestions $suggestions -SelectedIndex 0 -TypedSoFar $typed
    }
    finally {
        $script:HS.IsActive = $false
    }
}

# --- Shared Handlers for PSReadLine ---

function global:Get-HSCharHandler {
    return {
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
        Invoke-HSAutoSuggest
    }
}

function global:Get-HSSpaceHandler {
    return {
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
}

function global:Get-HSBackspaceHandler {
    return {
        if ([datetime]::Now -lt $script:HS.PasteUntil) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
            if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500) }
            return
        }
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
        [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
        Start-Sleep -Milliseconds 80
        Invoke-HSAutoSuggest
    }
}

function global:Get-HSEnterHandler {
    return {
        if ($script:HS.IsVisible) { Clear-HSOverlay }
        Reset-HSState
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $cmd = "$bufRef"
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        if (-not [string]::IsNullOrWhiteSpace($cmd)) { Invoke-HSRecord -Command $cmd }
    }
}

function global:Get-HSNavigationHandler {
    param([string]$Direction)
    return {
        if ($script:HS.IsVisible) {
            if ($Direction -eq 'Up') { $script:HS.SelectedIndex-- } else { $script:HS.SelectedIndex++ }
            Update-HSScroll
            Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
            return
        }
        if ($Direction -eq 'Up') { [Microsoft.PowerShell.PSConsoleReadLine]::PreviousHistory() } else { [Microsoft.PowerShell.PSConsoleReadLine]::NextHistory() }
    }
}

function global:Get-HSTabHandler {
    return {
        if ($script:HS.IsVisible) {
            $sel = $script:HS.Suggestions[$script:HS.SelectedIndex].command
            Clear-HSOverlay; Reset-HSState
            [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert($sel)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::TabComplete()
    }
}

function global:Get-HSEscapeHandler {
    return {
        if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState; return }
        [Microsoft.PowerShell.PSConsoleReadLine]::CancelLine() # or fallback
    }
}
