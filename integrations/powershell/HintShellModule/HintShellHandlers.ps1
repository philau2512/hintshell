# HintShellHandlers.ps1 - Event-driven Auto-Suggest Handler
# No ReadKey loop! All navigation handled by PSReadLine key bindings.
# These factories are the single source of truth: HintShellModule.psm1 binds them directly,
# so a behaviour fix here cannot drift away from what the module installs.

function global:Invoke-HSAutoSuggest {
    if ($script:HS.IsActive) { return }
    if ([datetime]::Now -lt $script:HS.PasteUntil) { return }
    # Esc means "not for this line": the panel stays away until the line is submitted or
    # Ctrl+Space asks for it again.
    if ($script:HS.Dismissed) { return }
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

        # Multi-line buffer (paste / open quote / >>): never paint over continuation lines
        if (Test-HSMultilineBuffer -Typed $typed) {
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
        if ([Console]::KeyAvailable) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::SelfInsert($key, $arg)
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
        if ([Console]::KeyAvailable) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::Insert(' ')
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
        if ([Console]::KeyAvailable) {
            if ($script:HS.IsVisible) { Clear-HSOverlay; Reset-HSState }
            [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::BackwardDeleteChar()
        if ([Console]::KeyAvailable) { $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500); return }
        Invoke-HSAutoSuggest
    }
}

function global:Get-HSEnterHandler {
    return {
        if ($script:HS.IsVisible) { Clear-HSOverlay }
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $cmd = "$bufRef"
        if (Test-HSMultilineBuffer -Typed $cmd) {
            $script:HS.PasteUntil = [datetime]::Now.AddMilliseconds(500)
        }
        Reset-HSState
        # A submitted line starts a fresh input, so a previous Esc no longer applies.
        $script:HS.Dismissed = $false
        [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
        if (-not [string]::IsNullOrWhiteSpace($cmd)) { Invoke-HSRecord -Command $cmd -Asynchronous }
    }
}

# Which way a navigation keypress should move. PSReadLine passes the ConsoleKeyInfo to every
# bound scriptblock, so one handler can serve both arrows. Note the trap: .Key is the
# ConsoleKey enum ('UpArrow'), while .KeyCode is an int - comparing on .KeyCode never matches.
function global:Get-HSNavigationDirection {
    param($Key)
    switch ("$($Key.Key)") {
        'DownArrow' { 'Down' }
        'UpArrow' { 'Up' }
        default { 'Down' }
    }
}

# Up/Down: move the panel selection while it is open, history otherwise.
# This must stay a single parameterless scriptblock. The previous version took a -Direction
# argument and closed over it with GetNewClosure, but a closure COPIES the module's
# $script:HS instead of sharing it: the selection moved in a private clone and the visible
# panel never changed, so Down appeared dead.
function global:Get-HSNavigationHandler {
    return {
        param($key, $arg)
        $direction = Get-HSNavigationDirection -Key $key
        if ($script:HS.IsVisible -and -not $script:HS.Dismissed) {
            if ($direction -eq 'Up') { $script:HS.SelectedIndex-- } else { $script:HS.SelectedIndex++ }
            Update-HSScroll
            Draw-HSOverlay -Suggestions $script:HS.Suggestions -SelectedIndex $script:HS.SelectedIndex -TypedSoFar $script:HS.CurrentInput
            return
        }
        if ($direction -eq 'Up') {
            [Microsoft.PowerShell.PSConsoleReadLine]::PreviousHistory()
        } else {
            [Microsoft.PowerShell.PSConsoleReadLine]::NextHistory()
        }
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

# Alt+Right (or Ctrl+Right) means "one word", plain Right means "the whole inline hint".
function global:Test-HSWordAcceptKey {
    param($Key)
    "$($Key.Modifiers)" -match 'Alt|Control'
}

# RightArrow: accept the inline hint when the cursor is already at the end of the line
# (the Fish/PSReadLine habit), otherwise move as usual. Destructive commands are never
# accepted through an arrow key; they need the visible Tab path. Same reason as above: no
# closure, the mode comes from the key that was actually pressed.
function global:Get-HSHintAcceptHandler {
    return {
        param($key, $arg)
        $word = Test-HSWordAcceptKey -Key $key
        $bufRef = $null; $curRef = $null
        [Microsoft.PowerShell.PSConsoleReadLine]::GetBufferState([ref]$bufRef, [ref]$curRef)
        $typed = "$bufRef"
        $atEnd = $curRef -eq $typed.Length

        if ($script:HS.IsVisible -and $atEnd -and $script:HS.Suggestions.Count -gt 0) {
            $index = [Math]::Min($script:HS.SelectedIndex, $script:HS.Suggestions.Count - 1)
            $s = $script:HS.Suggestions[$index]
            if (-not (Test-HSDestructive -Suggestion $s)) {
                $suffix = if ($word) {
                    Get-HSNextWordSuffix -Command $s.command -Typed $typed
                } else {
                    Get-HSGhostSuffix -Command $s.command -Typed $typed
                }
                if ($suffix) {
                    Clear-HSOverlay; Reset-HSState
                    [Microsoft.PowerShell.PSConsoleReadLine]::Insert($suffix)
                    if (-not $word) { Invoke-HSAutoSuggest }
                    return
                }
            }
        }
        # Nothing to accept: the arrow still does whatever the user pressed it for.
        if ($word) { [Microsoft.PowerShell.PSConsoleReadLine]::ForwardWord() }
        else { [Microsoft.PowerShell.PSConsoleReadLine]::ForwardChar() }
    }
}

function global:Get-HSEscapeHandler {
    return {
        if ($script:HS.IsVisible) {
            Clear-HSOverlay; Reset-HSState
            # Sticky until Enter or Ctrl+Space, so the panel does not reappear on the
            # next keystroke and teach the user that Esc does nothing.
            $script:HS.Dismissed = $true
            return
        }
        [Microsoft.PowerShell.PSConsoleReadLine]::RevertLine()
    }
}

# Ctrl+Space: manual trigger, and the way back after Esc.
function global:Get-HSManualTriggerHandler {
    return {
        $script:HS.Dismissed = $false
        Invoke-HSAutoSuggest
    }
}
