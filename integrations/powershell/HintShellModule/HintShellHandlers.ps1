# HintShellHandlers.ps1 - Event-driven Auto-Suggest Handler
# No ReadKey loop! All navigation handled by PSReadLine key bindings.

function global:Invoke-HSAutoSuggest {
    if ($script:HS.IsActive) { return }
    if ([datetime]::Now -lt $script:HS.PasteUntil) { return }
    $script:HS.IsActive = $true

    try {
        # Clear any leftover overlay
        if ($script:HS.IsVisible) { Clear-HSOverlay }

        # Read current buffer
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $typed = "$bufRef"

        if ([string]::IsNullOrWhiteSpace($typed)) {
            Reset-HSState
            return
        }

        # Skip if input contains non-ASCII (Vietnamese IME, etc.)
        if ($typed -match '[^\x00-\x7F]') {
            Reset-HSState
            return
        }

        # Query daemon for suggestions
        $suggestions = Get-HSSuggestions -Typed $typed
        if (-not $suggestions -or $suggestions.Count -eq 0) {
            Reset-HSState
            return
        }

        # Store state and render overlay
        $script:HS.Suggestions   = $suggestions
        $script:HS.SelectedIndex = 0
        $script:HS.ScrollOffset  = 0
        $script:HS.CurrentInput  = $typed

        Update-HSScroll
        Draw-HSOverlay -Suggestions $suggestions -SelectedIndex 0 -TypedSoFar $typed

        # RETURN IMMEDIATELY — no ReadKey loop!
        # Navigation is handled by PSReadLine key handlers (Up/Down/Tab/Enter/Esc)
    }
    finally {
        $script:HS.IsActive = $false
    }
}
