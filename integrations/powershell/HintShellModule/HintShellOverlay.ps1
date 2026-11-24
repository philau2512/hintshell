# HintShellOverlay.ps1 - Scrollable Window Rendering (Claude-style)
# Viewport + compact mode for narrow terminals.
# Uses $e[1B for drawing + SetCursorPosition for restore (always exact).
# Multi-line buffers: suppressed by handlers (Test-HSMultilineBuffer).
# Presentation follows the same rules as the live Bash overlay: same labels, same tiers,
# same ~/.hintshell/config.toml, and text colours that survive light terminals.

$script:HS_VIEWPORT_SIZE    = 6
$script:HS_COMPACT_VIEWPORT = 3
$script:HS_COMPACT_WIDTH    = 56
$script:HS_MIN_WIDTH        = 20
$script:HS_FULL_MAX_WIDTH   = 70
$script:HS_INFO_WIDTH       = 18

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
    GhostCols       = 0
    Dismissed       = $false
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
    $script:HS.ViewportSize    = (Get-HSConfig).MaxVisible
    $script:HS.GhostCols       = 0
}
#endregion

#region Configuration (~/.hintshell/config.toml, shared with the Rust overlay)
function script:Get-HSConfigPath {
    if ($env:HINTSHELL_HOME) { return Join-Path $env:HINTSHELL_HOME 'config.toml' }
    return Join-Path $env:USERPROFILE '.hintshell\config.toml'
}

$script:HS_ConfigCache = $null

# Minimal key reader: the same file the Rust side writes, so one edit styles every surface.
function script:Get-HSConfig {
    if ($null -ne $script:HS_ConfigCache) { return $script:HS_ConfigCache }

    $config = @{
        BorderColor = 'purple'
        MaxVisible  = $script:HS_VIEWPORT_SIZE
        GhostText   = $true
    }
    $path = Get-HSConfigPath
    if (Test-Path -LiteralPath $path) {
        try {
            foreach ($line in (Get-Content -LiteralPath $path -ErrorAction Stop)) {
                $text = $line.Trim()
                if ($text -match '^border_color\s*=\s*"([^"]+)"') { $config.BorderColor = $Matches[1].ToLowerInvariant() }
                elseif ($text -match '^max_visible\s*=\s*(\d+)') { $config.MaxVisible = [int]$Matches[1] }
                elseif ($text -match '^ghost_text\s*=\s*(true|false)') { $config.GhostText = ($Matches[1] -eq 'true') }
            }
        } catch {
            # Unreadable config must not cost the user their suggestions.
        }
    }
    # Same clamp the Rust overlay applies, so max_visible means the same thing everywhere.
    $config.MaxVisible = [Math]::Max(3, [Math]::Min(10, $config.MaxVisible))
    $script:HS_ConfigCache = $config
    return $config
}

function script:Reset-HSConfigCache { $script:HS_ConfigCache = $null }

# Mirrors HintShellConfig::border_ansi_code() with 256-colour codes: Conhost does not
# always understand 24-bit escapes, and a broken frame is worse than a plain one.
function script:Get-HSAccentCode {
    param([string]$Name)
    switch ($Name) {
        'blue'                                            { '75' }
        'cyan'                                            { '51' }
        'green'                                           { '48' }
        { $_ -in 'yellow', 'gold' }                       { '220' }
        'orange'                                          { '208' }
        { $_ -in 'pink', 'rose' }                         { '212' }
        'magenta'                                         { '201' }
        { $_ -in 'minimal', 'gray', 'grey' }              { '244' }
        'red'                                             { '203' }
        { $_ -in 'apple', 'siri', 'apple-intelligence', 'glow', 'neon' } { '51' }
        { $_ -in 'rainbow', 'gemini', 'gradient', 'aurora' }             { '141' }
        default                                           { '141' }
    }
}

# User-facing provenance, identical wording to core::engine::matcher::source_label so a
# user moving between shells sees the same vocabulary instead of internal keys.
function script:Get-HSSourceLabel {
    param([string]$Source)
    switch ($Source) {
        'history-local'            { 'here' }
        { $_ -in 'history-global', 'history' } { 'history' }
        'frequent'                 { 'most used' }
        'recent'                   { 'recent' }
        'default'                  { 'starter' }
        'path'                     { 'path' }
        'git'                      { 'git' }
        { $_ -in 'npm', 'pnpm', 'yarn', 'bun' } { 'scripts' }
        'docker'                   { 'docker' }
        'ssh'                      { 'ssh hosts' }
        'zoxide'                   { 'folders' }
        'cargo'                    { 'cargo' }
        default                    { 'history' }
    }
}

# Match-quality tag, mirroring core::engine::matcher::tier_tag.
function script:Get-HSTierTag {
    param($Tier)
    switch ([int]$Tier) {
        0 { '' }
        1 { '' }
        3 { [string][char]0x2248 }
        default { '~' }
    }
}

function script:Test-HSDestructive {
    param($Suggestion)
    $risk = "$($Suggestion.risk)".Trim().ToLowerInvariant()
    # A daemon that classified the command is always believed, in both directions.
    if ($risk -eq 'destructive') { return $true }
    if ($risk -eq 'normal') { return $false }

    # Older daemons send no risk field at all, and a safety warning must not depend on which
    # build answered the request. This short list covers the commands where an accidental
    # acceptance is unrecoverable; the daemon's own list stays authoritative when present.
    $command = "$($Suggestion.command)".ToLowerInvariant()
    $fallbackPatterns = @(
        'rm -rf', 'rm -fr', 'rm -r ', 'sudo rm',
        'git reset --hard', 'git clean -f', 'git checkout --', 'git push --force', 'git push -f',
        'docker system prune', 'docker rm -f', 'terraform destroy', 'kubectl delete',
        'dd if=', 'mkfs', 'chmod -R 777', 'kill -9',
        'remove-item -recurse -force', 'clear-history'
    )
    foreach ($pattern in $fallbackPatterns) {
        if ($command.StartsWith($pattern)) { return $true }
    }
    return $false
}

# One row's explanation column: provenance, how it matched, how often it was used.
function script:Get-HSItemInfo {
    param($Suggestion)
    $parts = @()
    $parts += Get-HSSourceLabel -Source "$($Suggestion.source)"
    $tag = Get-HSTierTag -Tier $Suggestion.tier
    if ($tag) { $parts += $tag }
    if ([int]$Suggestion.frequency -gt 0) { $parts += ('{0}x' -f [int]$Suggestion.frequency) }
    $info = $parts -join ' '
    if (Test-HSDestructive -Suggestion $Suggestion) { $info = "! $info" }
    return $info
}

function script:Get-HSFooterText {
    param($Suggestion)
    $description = "$($Suggestion.description)"
    if ([string]::IsNullOrWhiteSpace($description)) {
        if ([int]$Suggestion.frequency -gt 0) {
            $description = 'used {0}x . {1}' -f [int]$Suggestion.frequency, (Get-HSSourceLabel -Source "$($Suggestion.source)")
        } else {
            $description = 'no description'
        }
    }
    if (Test-HSDestructive -Suggestion $Suggestion) {
        $description = "[!] risky . $description"
    }
    return $description
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

    $config = Get-HSConfig
    $W = [Math]::Max($script:HS_MIN_WIDTH, $winW - 2)
    # A row wider than the window wraps, and a wrapped row is not the row the clear counts.
    $W = [Math]::Min($W, [Math]::Max(8, $winW - 1))
    $compact = $W -lt $script:HS_COMPACT_WIDTH
    if (-not $compact) {
        $W = [Math]::Min($W, $script:HS_FULL_MAX_WIDTH)
    }

    $vpMax = if ($compact) {
        [Math]::Min($script:HS_COMPACT_VIEWPORT, $config.MaxVisible)
    } else {
        $config.MaxVisible
    }
    $availableRows = Get-HSAvailableRows -CursorTop $CursorTop

    return @{
        W             = $W
        Compact       = $compact
        VpMax         = $vpMax
        AvailableRows = $availableRows
        Accent        = Get-HSAccentCode -Name $config.BorderColor
        GhostText     = [bool]$config.GhostText
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
            $existing = $seen[$cleanCmd]
            $existing.frequency += [int]$s.frequency
            # A duplicate of the same command must not lose the warnings either copy had.
            if ([int]$s.tier -lt [int]$existing.tier) { $existing.tier = [int]$s.tier }
            if (Test-HSDestructive -Suggestion $s) { $existing.risk = 'destructive' }
        } else {
            $newObj = [PSCustomObject]@{
                command     = $cleanCmd
                description = $s.description
                frequency   = [int]$s.frequency
                source      = "$($s.source)"
                # tier and risk are the presentation and safety signals. They used to be
                # dropped here, so the labels and the risky marker could never reach a row.
                # An older daemon sends neither: [int] gives 0 (no tag) and Test-HSDestructive
                # falls back to local rules instead of claiming the command is safe.
                tier        = [int]$s.tier
                risk        = "$($s.risk)"
            }
            $processed += $newObj
            $seen[$cleanCmd] = $newObj
        }
    }

    # The daemon already ranked candidates by match tier (prefix > contextual > substring >
    # typo > fuzzy). Filtering to a literal prefix here discarded the typo-tolerant and
    # contextual results the engine advertises, and treated typed text as a wildcard
    # pattern, so a path containing [ ] * ? made the panel disappear for no visible reason.
    return @($processed | Select-Object -First 30)
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
        $script:HS.GhostCols    = 0
        $script:HS.IsVisible    = $false
        return
    }

    if ($script:HS.OverlayLines -le 0) {
        if ($script:HS.GhostCols -gt 0) {
            $e = [char]27
            [Console]::Write("$e[K")
            $script:HS.GhostCols = 0
        }
        return
    }

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

function script:Get-HSGhostSuffix {
    param([string]$Command, [string]$Typed)
    # Only a real prefix may be previewed inline; a fuzzy or typo match needs the visible
    # row (and its tier tag) before anyone can accept it.
    if ([string]::IsNullOrEmpty($Command) -or [string]::IsNullOrEmpty($Typed)) { return '' }
    if ($Command.Length -le $Typed.Length) { return '' }
    if (-not $Command.StartsWith($Typed, [System.StringComparison]::OrdinalIgnoreCase)) { return '' }
    return $Command.Substring($Typed.Length)
}

# Split a (possibly truncated) command into the typed part and the rest. With no literal
# run to show - typo and fuzzy candidates - nothing is highlighted rather than the wrong
# characters, which is what a plain "first N chars" split did once non-prefix results
# reached the panel.
function script:Split-HSMatchedCommand {
    param([string]$Command, [string]$Typed)
    $needle = if ($null -eq $Typed) { '' } else { $Typed.Trim() }
    if ([string]::IsNullOrEmpty($needle)) {
        return @{ Before = ''; Matched = ''; After = $Command }
    }
    $index = $Command.IndexOf($needle, [System.StringComparison]::OrdinalIgnoreCase)
    if ($index -lt 0) {
        return @{ Before = ''; Matched = ''; After = $Command }
    }
    $length = [Math]::Min($needle.Length, $Command.Length - $index)
    return @{
        Before  = $Command.Substring(0, $index)
        Matched = $Command.Substring($index, $length)
        After   = $Command.Substring($index + $length)
    }
}

function script:Write-HSGhost {
    param([array]$Suggestions, [int]$SelectedIndex, [string]$TypedSoFar, [int]$CursorLeft)
    if (-not $Suggestions -or $Suggestions.Count -eq 0) { return }

    $index = [Math]::Min([Math]::Max($SelectedIndex, 0), $Suggestions.Count - 1)
    $s = $Suggestions[$index]
    if (Test-HSDestructive -Suggestion $s) { return }

    $ghost = Get-HSGhostSuffix -Command $s.command -Typed $TypedSoFar
    if ([string]::IsNullOrEmpty($ghost)) { return }

    $winW = Get-HSConsoleInt -Name WindowWidth -Default 80
    $maxGhost = [Math]::Max(0, $winW - $CursorLeft - 2)
    $ghost = Get-HSTruncateToWidth -Text $ghost -MaxCols $maxGhost
    $cols = Get-HSDisplayWidth -Text $ghost
    if ($cols -le 0) { return }

    $e = [char]27
    # Dim attribute instead of a hard-coded gray: it stays readable on light terminals too.
    [Console]::Write("$e[2m$ghost$e[0m")
    [Console]::SetCursorPosition($CursorLeft, [Console]::CursorTop)
    $script:HS.OverlayLines = -1  # ghost mode flag
    $script:HS.GhostCols    = $cols
    $script:HS.IsVisible    = $true
}

# Next-word acceptance: inserting the whole command from an arrow key is too destructive
# for a key people use to move the cursor; one word at a time is the Fish/PSReadLine habit.
function script:Get-HSNextWordSuffix {
    param([string]$Command, [string]$Typed)
    $suffix = Get-HSGhostSuffix -Command $Command -Typed $Typed
    if ([string]::IsNullOrEmpty($suffix)) { return '' }
    $chunk = ''
    $seenWord = $false
    foreach ($ch in $suffix.ToCharArray()) {
        if ([char]::IsWhiteSpace($ch)) {
            if ($seenWord) { break }
            $chunk += $ch
        } else {
            $seenWord = $true
            $chunk += $ch
        }
    }
    return $chunk
}

# Column budget for one row: prefix(3) + command + [gap + info] + gap + scroll(1) <= W.
# Kept out of Draw-HSOverlay so the arithmetic that used to overflow narrow terminals is
# unit-testable without a console, and so the command wins space before the annotations do.
function script:Get-HSRowLayout {
    param(
        [int]$Width,
        [switch]$Compact,
        [string]$Info
    )
    $prefixW = 3
    $gapW    = 1
    $scrollW = 1

    $infoW = 0
    $cmdW  = $Width - $prefixW - $gapW - $scrollW
    if (-not $Compact -and -not [string]::IsNullOrEmpty($Info)) {
        $withInfo = $cmdW - $gapW - $script:HS_INFO_WIDTH
        if ($withInfo -ge 12) {
            $infoW = $script:HS_INFO_WIDTH
            $cmdW  = $withInfo
        }
    }
    if ($cmdW -lt 4) { $cmdW = 4 }

    $total = $prefixW + $cmdW + $gapW + $scrollW
    if ($infoW -gt 0) { $total += $gapW + $infoW }

    return @{ CmdW = $cmdW; InfoW = $infoW; Total = $total }
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

    $budget  = Get-HSLayoutBudget -CursorTop $curTop
    $W       = $budget.W
    $compact = $budget.Compact
    $vpMax   = $budget.VpMax
    $accent  = $budget.Accent
    $script:HS.ViewportSize = $vpMax

    $total  = $Suggestions.Count
    $vp     = [Math]::Min($vpMax, $total)
    $offset = $script:HS.ScrollOffset

    # Limit to actual available space below (chrome = separator + footer)
    $maxDown  = $budget.AvailableRows
    $maxItems = [Math]::Max(0, $maxDown - 2)
    $vp       = [Math]::Min($vp, $maxItems)

    # Fallback: ghost text when no space for a list
    if ($vp -le 0) {
        if ($budget.GhostText) {
            Write-HSGhost -Suggestions $Suggestions -SelectedIndex $SelectedIndex -TypedSoFar $TypedSoFar -CursorLeft $curLeft
        }
        return
    }

    $buf      = [System.Text.StringBuilder]::new()
    $lines    = 0
    $prefixW  = 3
    $gapW     = 1
    $scrollW  = 1

    # More items exist than are drawn: say so. Comparing against VpMax alone hid the
    # counter exactly when a short terminal squeezed the list, which is when users need it.
    $hasMore = $total -gt $vp
    if ($hasMore) {
        $pos  = "$($SelectedIndex + 1)/$total"
        $posW = Get-HSDisplayWidth -Text $pos
        $sepW = [Math]::Max(0, $W - $posW - 1)
        $separator = ([string][char]0x2500 * $sepW)
        $null = $buf.Append("$e[1B$e[1G$e[2K$e[38;5;${accent}m$separator$e[0m $e[2m$pos$e[0m")
    } else {
        $separator = [string][char]0x2500 * $W
        $null = $buf.Append("$e[1B$e[1G$e[2K$e[38;5;${accent}m$separator$e[0m")
    }
    $lines++

    for ($i = 0; $i -lt $vp; $i++) {
        $idx = $offset + $i
        if ($idx -ge $total) { break }

        $s    = $Suggestions[$idx]
        $cmd  = $s.command.Replace("`r","").Replace("`n"," ").Replace("`t"," ").Trim()
        $risky = Test-HSDestructive -Suggestion $s

        $info = if ($compact) { '' } else { Get-HSItemInfo -Suggestion $s }
        $layout = Get-HSRowLayout -Width $W -Compact:$compact -Info $info
        $cmdW   = $layout.CmdW

        $scrollHint = ' '
        if ($hasMore) {
            if ($i -eq 0 -and $offset -gt 0) { $scrollHint = [string][char]0x25B2 }
            elseif ($i -eq ($vp - 1) -and ($offset + $vp) -lt $total) { $scrollHint = [string][char]0x25BC }
        }

        $cmd = Get-HSTruncateToWidth -Text $cmd -MaxCols $cmdW
        $parts = Split-HSMatchedCommand -Command $cmd -Typed $TypedSoFar
        $padLen = $cmdW - (Get-HSDisplayWidth -Text $cmd)
        if ($padLen -lt 0) { $padLen = 0 }
        $pad = ' ' * $padLen

        $isSelected = $idx -eq $SelectedIndex
        # Bold marks what the user typed; dim marks what HintShell added. Unselected rows
        # keep the terminal's own foreground colour, because the previous forced bright
        # white disappeared on light themes. Risk is red AND the '!' in the info column,
        # never colour alone.
        $matchStyle = if ($risky) { "$e[1;31m" } else { "$e[1m" }
        $restStyle  = if ($isSelected) { "$e[38;5;15m" } else { "$e[2m" }
        $restEnd    = if ($isSelected) { "$e[39m" } else { "$e[22m" }
        $cmdText = "$restStyle$($parts.Before)$matchStyle$($parts.Matched)$e[22m$($parts.After)$restEnd$pad"
        $prefix  = if ($isSelected) { ' > ' } else { '   ' }

        if ($layout.InfoW -gt 0) {
            $infoPadded = Get-HSPadToWidth -Text (Get-HSTruncateToWidth -Text $info -MaxCols $layout.InfoW) -Width $layout.InfoW
            $body = "$prefix$cmdText $e[2m$infoPadded$e[22m $scrollHint"
        } else {
            $body = "$prefix$cmdText $scrollHint"
        }

        $null = $buf.Append("$e[1B$e[1G$e[2K")
        if ($isSelected) {
            $null = $buf.Append("$e[48;5;236m$e[38;5;15m$body$e[0m")
        } else {
            $null = $buf.Append("$body$e[0m")
        }
        $lines++
    }

    # Footer: what the selected command does, plus the keys that act on it.
    $selIndex = [Math]::Min([Math]::Max($SelectedIndex, 0), $total - 1)
    $descText = Get-HSFooterText -Suggestion $Suggestions[$selIndex]

    $hint = if ($compact) { '' } else { 'Tab/Right accept . Alt+Right word' }
    $hintGap = if ([string]::IsNullOrEmpty($hint)) { 0 } else { 2 }
    $hintW = Get-HSDisplayWidth -Text $hint
    $maxDescCols = [Math]::Max(4, $W - 1 - $hintGap - $hintW)
    $descText = Get-HSTruncateToWidth -Text $descText -MaxCols $maxDescCols
    $descPad = ' ' * [Math]::Max(0, $W - 1 - (Get-HSDisplayWidth -Text $descText) - $hintGap - $hintW)
    $null = $buf.Append("$e[1B$e[1G$e[2K$e[2m $descText$e[22m$descPad$e[2m$hint$e[0m")
    $lines++

    # If previous overlay had more lines than current, clear trailing lines atomically
    $oldLines = $script:HS.OverlayLines
    if ($oldLines -gt $lines -and $oldLines -ne -1) {
        $toClearExtra = [Math]::Min($oldLines - $lines, [Math]::Max(0, $maxDown - $lines))
        for ($k = 0; $k -lt $toClearExtra; $k++) {
            $null = $buf.Append("$e[1B$e[1G$e[2K")
        }
    }

    [Console]::Write($buf.ToString())

    # Restore cursor to EXACT saved position
    [Console]::SetCursorPosition($curLeft, $curTop)

    $script:HS.OverlayLines = $lines
    $script:HS.IsVisible    = $true
}

#endregion