# HintShellHandlers.ps1 - Real-time Auto-Suggest Handler
# Scrollable viewport with Claude-style navigation.

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

        if ([string]::IsNullOrWhiteSpace($typed)) { return }

        # Skip if input contains non-ASCII (Vietnamese IME, etc.)
        if ($typed -match '[^\x00-\x7F]') { return }

        # Query + process
        $suggestions = Get-HSSuggestions -Typed $typed
        if (-not $suggestions -or $suggestions.Count -eq 0) { return }

        # Initialize state
        $script:HS.Suggestions   = $suggestions
        $script:HS.SelectedIndex = 0
        $script:HS.ScrollOffset  = 0
        $script:HS.CurrentInput  = $typed

        Update-HSScroll
        Draw-HSOverlay -Suggestions $suggestions -SelectedIndex 0 -TypedSoFar $typed

        # === Navigation Loop ===
        while ($true) {
            # Fast exit for paste: if keys are waiting, clear and return to PSReadLine
            if ([Console]::KeyAvailable) {
                Clear-HSOverlay
                return
            }

            $k = [Console]::ReadKey($true)

            # Post-read paste detection: wait briefly for more keys
            # This catches IDE/programmatic input where chars arrive with slight delays
            Start-Sleep -Milliseconds 30
            if ([Console]::KeyAvailable) {
                # Insert the character we consumed back into PSReadLine
                if ($k.KeyChar -ne 0) {
                    [Microsoft.PowerShell.PSConsoleReadLine]::Insert([string]$k.KeyChar)
                }
                $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
                Clear-HSOverlay
                Reset-HSState
                return
            }

            switch ($k.Key) {
                'UpArrow' {
                    $script:HS.SelectedIndex--
                    Update-HSScroll
                    Clear-HSOverlay
                    Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
                }
                'DownArrow' {
                    $script:HS.SelectedIndex++
                    Update-HSScroll
                    Clear-HSOverlay
                    Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
                }
                'Tab' {
                    # Accept suggestion (don't execute)
                    Clear-HSOverlay
                    $sel = $script:HS.Suggestions[$script:HS.SelectedIndex].command
                    Reset-HSState
                    [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
                    [Microsoft.PowerShell.PSConsoleReadLine]::Insert($sel)
                    return
                }
                'Enter' {
                    # Don't Clear overlay (let command output overwrite it naturally)
                    # This prevents PSReadLine prompt corruption
                    $bufRef3 = $null; $curRef3 = $null
                    [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef3, [ref]$curRef3)
                    $currentCmd = "$bufRef3"
                    $script:HS.OverlayLines = 0
                    $script:HS.IsVisible = $false
                    Reset-HSState
                    [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
                    if (-not [string]::IsNullOrWhiteSpace($currentCmd)) {
                        Invoke-HSRecord -Command $currentCmd
                    }
                    return
                }
                'Escape' {
                    Clear-HSOverlay
                    Reset-HSState
                    return
                }
                'Backspace' {
                    # Delete char + re-query (STAY in loop)
                    Clear-HSOverlay
                    [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()

                    $bufRef2 = $null; $curRef2 = $null
                    [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef2, [ref]$curRef2)
                    $newTyped = "$bufRef2"

                    if ([string]::IsNullOrWhiteSpace($newTyped)) {
                        Reset-HSState
                        return
                    }

                    $newSuggs = Get-HSSuggestions -Typed $newTyped
                    if (-not $newSuggs -or $newSuggs.Count -eq 0) {
                        Reset-HSState
                        return
                    }

                    $script:HS.Suggestions   = $newSuggs
                    $script:HS.SelectedIndex = 0
                    $script:HS.ScrollOffset  = 0
                    $script:HS.CurrentInput  = $newTyped
                    Update-HSScroll
                    Draw-HSOverlay -Suggestions $newSuggs -SelectedIndex 0 -TypedSoFar $newTyped
                }
                default {
                    # Skip non-ASCII (Vietnamese IME, etc.)
                    if ($k.KeyChar -ne [char]0 -and -not [char]::IsControl($k.KeyChar) -and [int]$k.KeyChar -lt 128) {
                        # Insert char
                        Clear-HSOverlay
                        [Microsoft.PowerShell.PSConsoleReadLine]::Insert($k.KeyChar)

                        # Paste/programmatic input check before expensive IPC
                        Start-Sleep -Milliseconds 30
                        if ([Console]::KeyAvailable) {
                            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
                            Reset-HSState
                            return
                        }
                        if ([datetime]::Now -lt $script:HS.PasteUntil) {
                            Reset-HSState
                            return
                        }

                        $bufRef2 = $null; $curRef2 = $null
                        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef2, [ref]$curRef2)
                        $newTyped = "$bufRef2"

                        $newSuggs = Get-HSSuggestions -Typed $newTyped
                        if (-not $newSuggs -or $newSuggs.Count -eq 0) {
                            Reset-HSState
                            return
                        }

                        $script:HS.Suggestions   = $newSuggs
                        $script:HS.SelectedIndex = 0
                        $script:HS.ScrollOffset  = 0
                        $script:HS.CurrentInput  = $newTyped
                        Update-HSScroll
                        Draw-HSOverlay -Suggestions $newSuggs -SelectedIndex 0 -TypedSoFar $newTyped
                    } else {
                        # Non-ASCII printable (Vietnamese IME, etc.): insert then close
                        if ($k.KeyChar -ne [char]0 -and -not [char]::IsControl($k.KeyChar)) {
                            [Microsoft.PowerShell.PSConsoleReadLine]::Insert([string]$k.KeyChar)
                        }
                        Clear-HSOverlay
                        Reset-HSState
                        return
                    }
                }
            }
        }
    }
    finally {
        $script:HS.IsActive = $false
    }
}
