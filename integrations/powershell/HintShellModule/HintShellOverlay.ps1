# HintShellOverlay.ps1 - Scrollable Window Rendering (Claude-style)
# Viewport + compact mode for narrow terminals.
# Uses $e[1B for drawing + SetCursorPosition for restore (always exact).
# Multi-line buffers: suppressed by handlers (Test-HSMultilineBuffer).

$script:HS_VIEWPORT_SIZE    = 6
$script:HS_COMPACT_VIEWPORT = 3
$script:HS_COMPACT_WIDTH    = 56
$script:HS_MIN_WIDTH        = 20
$script:HS_FULL_MAX_WIDTH   = 70

#region State
$script:HS = @{
    Suggestions     = @()
    SelectedIndex   = 0
    ScrollOffset    = 0
    OverlayLines    = 0
    IsVisible       = $false
    IsActive        = $false
    CurrentInput    = ''
    SavedCursorCol  = 0
    SavedCursorTop  = 0
    PasteUntil      = [datetime]::MinValue
    ViewportSize    = 6
}

function script:Reset-HSState {
    $script:HS.Suggestions     = @()
    $script:HS.SelectedIndex   = 0
    $script:HS.ScrollOffset    = 0
    $script:HS.OverlayLines    = 0
    $script:HS.IsVisible       = $false
    $script:HS.IsActive        = $false
    $script:HS.CurrentInput    = ''
    $script:HS.SavedCursorCol  = 0
    $script:HS.SavedCursorTop  = 0
    $script:HS.ViewportSize    = $script:HS_VIEWPORT_SIZE
}
#endregion

#region Layout helpers (Phase A + B)

function script:Test-HSMultilineBuffer {
    param([string]$Typed)
    if ([string]::IsNullOrEmpty($Typed)) { return $false }
    return $Typed.Contains("`n") -or $Typed.Contains("`r")
}

# Display column width: ASCII / most BMP symbols = 1; CJK / fullwidth / surrogates = 2
function script:Get-HSElementWidth {
    param([string]$Element)
    if ([string]::IsNullOrEmpty($Element)) { return 0 }
    # Surrogate pair / multi-codepoint emoji cluster → treat as wide
    if ($Element.Length -ge 2) { return 2 }
    $cp = [int][char]$Element[0]
    if ($cp -lt 0x80) { return 1 }
    # Fullwidth / CJK / Hangul / Kana (common wide ranges in terminals)
    if (($cp -ge 0x1100 -and $cp -le 0x115F) -or
        ($cp -ge 0x2E80 -and $cp -le 0xA4CF) -or
        ($cp -ge 0xAC00 -and $cp -le 0xD7A3) -or
        ($cp -ge 0xF900 -and $cp -le 0xFAFF) -or
        ($cp -ge 0xFE10 -and $cp -le 0xFE19) -or
        ($cp -ge 0xFE30 -and $cp -le 0xFE6F) -or
        ($cp -ge 0xFF01 -and $cp -le 0xFF60) -or
        ($cp -ge 0xFFE0 -and $cp -le 0xFFE6)) {
        return 2
    }
    return 1
}

function script:Get-HSDisplayWidth {
    param([string]$Text)
    if ([string]::IsNullOrEmpty($Text)) { return 0 }
    $w = 0
    $enum = [System.Globalization.StringInfo]::GetTextElementEnumerator($Text)
    while ($enum.MoveNext()) {
        $w += Get-HSElementWidth -Element $enum.GetTextElement()
    }
    return $w
}

function script:Get-HSTruncateToWidth {
    param(
        [string]$Text,
        [int]$MaxCols
    )
    if ($MaxCols -le 0) { return '' }
    if ([string]::IsNullOrEmpty($Text)) { return '' }
    if ((Get-HSDisplayWidth -Text $Text) -le $MaxCols) { return $Text }

    $ellipsis = [string][char]0x2026
    $budget = $MaxCols - 1
    if ($budget -le 0) { return $ellipsis }

    $acc = ''
    $w = 0
    $enum = [System.Globalization.StringInfo]::GetTextElementEnumerator($Text)
    while ($enum.MoveNext()) {
        $el = $enum.GetTextElement()
        $ew = Get-HSElementWidth -Element $el
        if (($w + $ew) -gt $budget) { break }
        $acc += $el
        $w += $ew
    }
    return $acc + $ellipsis
}

function script:Get-HSPadToWidth {
    param(
        [string]$Text,
        [int]$Width
    )
    $cur = Get-HSDisplayWidth -Text $Text
    $need = $Width - $cur
    if ($need -le 0) { return $Text }
    return $Text + (' ' * $need)
}

# Safe console int read (no console handle in headless/CI → default)
function script:Get-HSConsoleInt {
    param(
        [Parameter(Mandatory)][ValidateSet('WindowWidth','WindowHeight','WindowTop','BufferHeight')]
        [string]$Name,
        [int]$Default
    )
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Stop'
    try {
        switch ($Name) {
            'WindowWidth'  { return [int][Console]::WindowWidth }
            'WindowHeight' { return [int][Console]::WindowHeight }
            'WindowTop'    { return [int][Console]::WindowTop }
            'BufferHeight' { return [int][Console]::BufferHeight }
        }
    } catch {
        return $Default
    } finally {
        $ErrorActionPreference = $prev
    }
    return $Default
}

function script:Get-HSAvailableRows {
    param([int]$CursorTop)
    $bufH = Get-HSConsoleInt -Name BufferHeight -Default 100
    $bufAvail = [Math]::Max(0, $bufH - $CursorTop - 1)
    $winH   = Get-HSConsoleInt -Name WindowHeight -Default 30
    $winTop = Get-HSConsoleInt -Name WindowTop -Default 0
    $rowInWin = $CursorTop - $winTop
    $winAvail = [Math]::Max(0, $winH - $rowInWin - 1)
    return [Math]::Min($winAvail, $bufAvail)
}

function script:Get-HSLayoutBudget {
    param([int]$CursorTop)

    $winW = Get-HSConsoleInt -Name WindowWidth -Default 80
    if ($winW -lt 1) { $winW = 80 }

    $W = [Math]::Max($script:HS_MIN_WIDTH, $winW - 2)
    $compact = $W -lt $script:HS_COMPACT_WIDTH
    if (-not $compact) {
        $W = [Math]::Min($W, $script:HS_FULL_MAX_WIDTH)
    }

    $vpMax = if ($compact) { $script:HS_COMPACT_VIEWPORT } else { $script:HS_VIEWPORT_SIZE }
    $availableRows = Get-HSAvailableRows -CursorTop $CursorTop

    return @{
        W             = $W
        Compact       = $compact
        VpMax         = $vpMax
        AvailableRows = $availableRows
    }
}
#endregion

#region Suggestion Processing

function script:Get-HSSuggestions {
    param([string]$Typed)

    $allSuggestions = Invoke-HSDaemon -Query $Typed -Limit 50
    $processed = @()
    $seen = @{}

    foreach ($s in $allSuggestions) {
        $cleanCmd = $s.command.Replace("`r", "").Replace("`n", " ").Replace("`t", " ").Trim()
        if ([string]::IsNullOrWhiteSpace($cleanCmd)) { continue }
        if ($seen.ContainsKey($cleanCmd)) {
            $seen[$cleanCmd].frequency += [int]$s.frequency
        } else {
            $newObj = [PSCustomObject]@{ command = $cleanCmd; description = $s.description; frequency = [int]$s.frequency; source = $s.source }
            $processed += $newObj
            $seen[$cleanCmd] = $newObj
        }
    }

    return @($processed |
        Where-Object { $_.command -like "$Typed*" } |
        Select-Object -First 30)
}
#endregion

#region Scroll Logic

function script:Update-HSScroll {
    $total = $script:HS.Suggestions.Count
    $vp    = if ($script:HS.ViewportSize -gt 0) { $script:HS.ViewportSize } else { $script:HS_VIEWPORT_SIZE }
    $sel   = $script:HS.SelectedIndex

    if ($sel -lt 0) { $sel = $total - 1; $script:HS.SelectedIndex = $sel }
    if ($sel -ge $total) { $sel = 0; $script:HS.SelectedIndex = $sel }

    if ($sel -lt $script:HS.ScrollOffset) { $script:HS.ScrollOffset = $sel }
    if ($sel -ge ($script:HS.ScrollOffset + $vp)) { $script:HS.ScrollOffset = $sel - $vp + 1 }

    $maxOffset = [Math]::Max(0, $total - $vp)
    if ($script:HS.ScrollOffset -gt $maxOffset) { $script:HS.ScrollOffset = $maxOffset }
    if ($script:HS.ScrollOffset -lt 0) { $script:HS.ScrollOffset = 0 }
}
#endregion

#region Rendering

function script:Clear-HSOverlay {
    if ($script:HS.OverlayLines -eq -1) {
        # Ghost text mode: clear from cursor to end of line
        $e = [char]27
        [Console]::Write("$e[K")
        $script:HS.OverlayLines = 0
        $script:HS.IsVisible    = $false
        return
    }

    if ($script:HS.OverlayLines -le 0) { return }

    $e = [char]27
    $n = $script:HS.OverlayLines

    # Save exact cursor position BEFORE any movement
    $curTop  = [Console]::CursorTop
    $curLeft = [Console]::CursorLeft

    # Only clear lines that actually exist below cursor (window-aware)
    $maxDown = Get-HSAvailableRows -CursorTop $curTop
    $toClear = [Math]::Min($n, $maxDown)

    if ($toClear -gt 0) {
        $buf = [System.Text.StringBuilder]::new()
        for ($i = 0; $i -lt $toClear; $i++) {
            $null = $buf.Append("$e[1B$e[1G$e[2K")
        }
        [Console]::Write($buf.ToString())
    }

    # Restore cursor to EXACT saved position (not relative!)
    [Console]::SetCursorPosition($curLeft, $curTop)

    $script:HS.OverlayLines = 0
    $script:HS.IsVisible    = $false
}

function script:Draw-HSOverlay {
    param([array]$Suggestions, [int]$SelectedIndex, [string]$TypedSoFar)

    $e = [char]27
    if (-not $Suggestions -or $Suggestions.Count -eq 0) { return }

    # Save exact cursor position BEFORE any movement
    $curTop  = [Console]::CursorTop
    $curLeft = [Console]::CursorLeft
    $script:HS.SavedCursorCol = $curLeft
    $script:HS.SavedCursorTop = $curTop

    $budget = Get-HSLayoutBudget -CursorTop $curTop
    $W      = $budget.W
    $compact = $budget.Compact
    $vpMax  = $budget.VpMax
    $script:HS.ViewportSize = $vpMax

    $total  = $Suggestions.Count
    $vp     = [Math]::Min($vpMax, $total)
    $offset = $script:HS.ScrollOffset

    # Limit to actual available space below (chrome = separator + footer)
    $maxDown  = $budget.AvailableRows
    $maxItems = [Math]::Max(0, $maxDown - 2)
    $vp       = [Math]::Min($vp, $maxItems)

    # Fallback: ghost text when no space for overlay
    if ($vp -le 0) {
        $topCmd = $Suggestions[0].command
        if ($topCmd.Length -gt $TypedSoFar.Length -and $topCmd.StartsWith($TypedSoFar, [System.StringComparison]::OrdinalIgnoreCase)) {
            $ghost = $topCmd.Substring($TypedSoFar.Length)
            $maxGhost = [Math]::Max(0, (Get-HSConsoleInt -Name WindowWidth -Default 80) - $curLeft - 1)
            $ghost = Get-HSTruncateToWidth -Text $ghost -MaxCols $maxGhost
            [Console]::Write("$e[38;5;240m$ghost$e[0m")
            [Console]::SetCursorPosition($curLeft, $curTop)
            $script:HS.OverlayLines = -1  # ghost mode flag
            $script:HS.IsVisible    = $true
        }
        return
    }

    $buf      = [System.Text.StringBuilder]::new()
    $lines    = 0
    $matchLen = $TypedSoFar.Length

    # Row layout: " > " (3) + cmd + " " + countStr + " " + scrollHint (1)
    # Reserve right side for count + scroll; rest for command.
    $prefixW = 3
    $gapW    = 1
    $scrollW = 1

    # Top separator with scroll indicator
    $hasMore = $total -gt $vpMax
    if ($hasMore) {
        $pos = "$($SelectedIndex + 1)/$total"
        $posW = Get-HSDisplayWidth -Text $pos
        $sepW = [Math]::Max(0, $W - $posW - 1)
        $separator = ([string][char]0x2500 * $sepW)
        $null = $buf.Append("$e[1B$e[1G$e[2K$e[38;5;238m$separator $e[38;5;244m$pos$e[0m")
    } else {
        $separator = [string][char]0x2500 * $W
        $null = $buf.Append("$e[1B$e[1G$e[2K$e[38;5;238m$separator$e[0m")
    }
    $lines++

    # Draw visible items
    for ($i = 0; $i -lt $vp; $i++) {
        $idx = $offset + $i
        if ($idx -ge $total) { break }

        $s    = $Suggestions[$idx]
        $cmd  = $s.command.Replace("`r","").Replace("`n"," ").Replace("`t"," ").Trim()
        $freq = if ($s.frequency) { [int]$s.frequency } else { 1 }

        $src = if ($s.source) { $s.source } else { '' }
        $freqStr = "{0}x" -f $freq
        if ($compact) {
            $countStr = $freqStr
        } else {
            switch ($src) {
                'recent'   { $countStr = "$freqStr (recent)" }
                'frequent' { $countStr = "$freqStr (most use)" }
                default    { $countStr = "{0,5}x" -f $freq }
            }
        }

        $scrollHint = ' '
        if ($hasMore) {
            if ($i -eq 0 -and $offset -gt 0) { $scrollHint = [string][char]0x25B2 }
            elseif ($i -eq ($vp - 1) -and ($offset + $vp) -lt $total) { $scrollHint = [string][char]0x25BC }
        }

        $countW = Get-HSDisplayWidth -Text $countStr
        # prefix(3) + cmd + gap(1) + count + gap(1) + scroll(1) <= W
        $cmdW = $W - $prefixW - $gapW - $countW - $gapW - $scrollW
        if ($cmdW -lt 4) { $cmdW = 4 }

        $cmd = Get-HSTruncateToWidth -Text $cmd -MaxCols $cmdW

        $mLen     = [Math]::Min($matchLen, $cmd.Length)
        # Match highlight on truncated cmd (char-based; display-safe enough for ASCII commands)
        $matchPrt = $cmd.Substring(0, $mLen)
        $restPrt  = if ($cmd.Length -gt $mLen) { $cmd.Substring($mLen) } else { '' }
        # Pad after full command display width
        $padLen = $cmdW - (Get-HSDisplayWidth -Text $cmd)
        if ($padLen -lt 0) { $padLen = 0 }
        $pad = ' ' * $padLen

        $null = $buf.Append("$e[1B$e[1G$e[2K")
        if ($idx -eq $SelectedIndex) {
            $null = $buf.Append("$e[48;5;236m$e[38;5;15m > $matchPrt$e[38;5;51m$restPrt$pad $e[38;5;244m$countStr $scrollHint$e[0m")
        } else {
            $null = $buf.Append("$e[38;5;255m   $matchPrt$e[38;5;242m$restPrt$pad $e[38;5;239m$countStr $e[38;5;238m$scrollHint$e[0m")
        }
        $lines++
    }

    # Footer (Command Description) — no emoji (display-width safe)
    $selCmdObj = $Suggestions[$SelectedIndex]
    $descText = if (-not [string]::IsNullOrWhiteSpace($selCmdObj.description)) { $selCmdObj.description } else { "No description available" }

    $footerPrefix = " > "
    $prefixDisp = Get-HSDisplayWidth -Text $footerPrefix
    $maxDescCols = [Math]::Max(4, $W - $prefixDisp)
    $descText = Get-HSTruncateToWidth -Text $descText -MaxCols $maxDescCols
    $hintText = "$footerPrefix$descText"

    $null = $buf.Append("$e[1B$e[1G$e[2K$e[38;5;243m$hintText$e[0m")
    $lines++

    [Console]::Write($buf.ToString())

    # Restore cursor to EXACT saved position
    [Console]::SetCursorPosition($curLeft, $curTop)

    $script:HS.OverlayLines = $lines
    $script:HS.IsVisible    = $true
}

#endregion