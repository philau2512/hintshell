# HintShellOverlay.ps1 - Scrollable Window Rendering (Claude-style)
# Fixed viewport of 6 items with scroll support.
# Uses $e[1B for drawing + SetCursorPosition for restore (always exact).

$script:HS_VIEWPORT_SIZE = 6

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
            $newObj = [PSCustomObject]@{ command = $cleanCmd; description = $s.description; frequency = [int]$s.frequency }
            $processed += $newObj
            $seen[$cleanCmd] = $newObj
        }
    }

    return @($processed |
        Where-Object { $_.command -like "$Typed*" } |
        Sort-Object frequency -Descending |
        Select-Object -First 30)
}
#endregion

#region Scroll Logic

function script:Update-HSScroll {
    $total = $script:HS.Suggestions.Count
    $vp    = $script:HS_VIEWPORT_SIZE
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

    # Only clear lines that actually exist below cursor
    $maxDown = [Console]::BufferHeight - $curTop - 1
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

    $W    = [Math]::Min([Console]::WindowWidth - 2, 70)
    $cmdW = $W - 12

    $total  = $Suggestions.Count
    $vp     = [Math]::Min($script:HS_VIEWPORT_SIZE, $total)
    $offset = $script:HS.ScrollOffset

    # Limit to actual available space below (no scrolling!)
    $maxDown  = [Console]::BufferHeight - $curTop - 1
    $maxItems = [Math]::Max(0, $maxDown - 2)  # -2 for separator + footer
    $vp       = [Math]::Min($vp, $maxItems)

    # Fallback: ghost text when no space for overlay
    if ($vp -le 0) {
        $topCmd = $Suggestions[0].command
        if ($topCmd.Length -gt $TypedSoFar.Length -and $topCmd.StartsWith($TypedSoFar, [System.StringComparison]::OrdinalIgnoreCase)) {
            $ghost = $topCmd.Substring($TypedSoFar.Length)
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

    # Top separator with scroll indicator
    $hasMore = $total -gt $script:HS_VIEWPORT_SIZE
    if ($hasMore) {
        $pos = "$($SelectedIndex + 1)/$total"
        $sepW = $W - $pos.Length - 1
        $separator = ([string][char]0x2500 * [Math]::Max(0, $sepW))
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

        if ($cmd.Length -gt $cmdW) { $cmd = $cmd.Substring(0, $cmdW - 1) + [char]0x2026 }

        $mLen     = [Math]::Min($matchLen, $cmd.Length)
        $matchPrt = $cmd.Substring(0, $mLen)
        $restPrt  = if ($cmd.Length -gt $mLen) { $cmd.Substring($mLen) } else { '' }
        $pad      = ' ' * [Math]::Max(0, ($cmdW - $cmd.Length))
        $countStr = "{0,3}x" -f $freq

        $scrollHint = ' '
        if ($hasMore) {
            if ($i -eq 0 -and $offset -gt 0) { $scrollHint = [char]0x25B2 }
            elseif ($i -eq ($vp - 1) -and ($offset + $vp) -lt $total) { $scrollHint = [char]0x25BC }
        }

        $null = $buf.Append("$e[1B$e[1G$e[2K")
        if ($idx -eq $SelectedIndex) {
            $null = $buf.Append("$e[48;5;236m$e[38;5;15m > $matchPrt$e[38;5;51m$restPrt$pad $e[38;5;244m$countStr $scrollHint$e[0m")
        } else {
            $null = $buf.Append("$e[38;5;255m   $matchPrt$e[38;5;242m$restPrt$pad $e[38;5;239m$countStr $e[38;5;238m$scrollHint$e[0m")
        }
        $lines++
    }

    # Footer (Command Description)
    $selCmdObj = $Suggestions[$SelectedIndex]
    $descText = if (-not [string]::IsNullOrWhiteSpace($selCmdObj.description)) { $selCmdObj.description } else { "No description available" }
    
    # Truncate description safely to avoid word wrapping
    $maxDescLen = [Math]::Max(10, $W - 6)
    if ($descText.Length -gt $maxDescLen) { $descText = $descText.Substring(0, $maxDescLen - 1) + [char]0x2026 }
    
    $hintText = " 💡 $descText"
    $null = $buf.Append("$e[1B$e[1G$e[2K$e[38;5;243m$hintText$e[0m")
    $lines++

    [Console]::Write($buf.ToString())

    # Restore cursor to EXACT saved position
    [Console]::SetCursorPosition($curLeft, $curTop)

    $script:HS.OverlayLines = $lines
    $script:HS.IsVisible    = $true
}

#endregion
