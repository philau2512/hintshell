$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Set-Location $repo

$overlay = Join-Path $repo 'integrations/powershell/HintShellModule/HintShellOverlay.ps1'
$handlers = Join-Path $repo 'integrations/powershell/HintShellModule/HintShellHandlers.ps1'
$module = Join-Path $repo 'integrations/powershell/HintShellModule/HintShellModule.psm1'

foreach ($f in @($overlay, $handlers, $module)) {
  $tokens = $null; $errors = $null
  $null = [System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$tokens, [ref]$errors)
  if ($errors -and $errors.Count -gt 0) {
    Write-Host "FAIL $f"
    $errors | ForEach-Object { Write-Host $_.ToString() }
    exit 1
  }
  Write-Host "OK parse $f"
}

# --- source contract ---------------------------------------------------------------
# These rules are invisible in a screenshot but cheap to pin: if someone re-adds a
# client-side filter or a bright-white foreground, the review that removed it is undone.
$overlayText = Get-Content -LiteralPath $overlay -Raw
$handlerText = Get-Content -LiteralPath $handlers -Raw
$moduleText = Get-Content -LiteralPath $module -Raw
$contract = @(
  @{ Name = 'module binds RightArrow hint accept'; Text = $moduleText; MustMatch = '-Key RightArrow\s' }
  @{ Name = 'module binds Alt+RightArrow word accept'; Text = $moduleText; MustMatch = 'Alt\+RightArrow' }
  @{ Name = 'module binds Ctrl+Space re-open'; Text = $moduleText; MustMatch = 'Ctrl\+Spacebar' }
  @{ Name = 'overlay reads the shared config.toml'; Text = $overlayText; MustMatch = 'Get-HSConfig' }
  @{ Name = 'overlay does not force a bright foreground'; Text = $overlayText; MustNotMatch = '38;5;255' }
  @{ Name = 'overlay keeps the daemon ranking (no prefix filter)'; Text = $overlayText; MustNotMatch = '-like\s"\$Typed' }
  @{ Name = 'handlers do not reject non-ASCII input'; Text = $handlerText; MustNotMatch = '\[\^\\x00-\\x7F\]' }
  @{ Name = 'handlers honour a dismissed panel'; Text = $handlerText; MustMatch = 'HS\.Dismissed' }
  # A closure copies the module's $script:HS, so keypresses would move a private clone and
  # the visible panel would never change. Up/Down and Right get their mode from the key.
  @{ Name = 'no handler factory clones module state with GetNewClosure'; Text = $handlerText; MustNotMatch = '\.GetNewClosure\(\)' }
  @{ Name = 'navigation binds one shared scriptblock'; Text = $moduleText; MustNotMatch = '-Direction' }
  @{ Name = 'word-accept binds one shared scriptblock'; Text = $moduleText; MustNotMatch = 'Get-HSHintAcceptHandler -Word' }
)
foreach ($rule in $contract) {
  if ($rule.ContainsKey('MustNotMatch')) {
    if ($rule.Text -match $rule.MustNotMatch) { throw "contract broken: $($rule.Name)" }
  } elseif ($rule.Text -notmatch $rule.MustMatch) {
    throw "contract missing: $($rule.Name)"
  }
  Write-Host "OK contract $($rule.Name)"
}

# Deterministic config: the overlay now reads the same ~/.hintshell/config.toml as the
# Rust side, so the tests pin it instead of inheriting the developer's theme.
$configHome = Join-Path ([System.IO.Path]::GetTempPath()) ("hs-layout-test-" + [guid]::NewGuid().ToString('N'))
$previousHome = $env:HINTSHELL_HOME
New-Item -ItemType Directory -Path $configHome -Force | Out-Null
Set-Content -LiteralPath (Join-Path $configHome 'config.toml') -Value @(
  'border_color = "apple"',
  'max_visible = 99',
  'ghost_text = false'
)
$env:HINTSHELL_HOME = $configHome

try {
. $overlay
function script:Invoke-HSDaemon { param($Query, $Limit) @() }

$w1 = Get-HSDisplayWidth -Text 'hello'
$w2 = Get-HSDisplayWidth -Text ([string][char]0x2500 * 5)
$t1 = Get-HSTruncateToWidth -Text ('x' * 20) -MaxCols 10
$ml1 = Test-HSMultilineBuffer -Typed ("git commit -m" + [char]10 + "foo")
$ml2 = Test-HSMultilineBuffer -Typed 'git status'

if ($w1 -ne 5) { throw "width hello=$w1" }
# U+2500 box-drawing is single-column in modern terminals
if ($w2 -ne 5) { throw "width box=$w2" }
if (-not $ml1) { throw 'multiline detect failed' }
if ($ml2) { throw 'single-line false positive' }
$tw = Get-HSDisplayWidth -Text $t1
if ($tw -gt 10) { throw "trunc too wide: $tw" }

# --- configuration is shared with the Rust overlay -------------------------------
$config = Get-HSConfig
if ($config.BorderColor -ne 'apple') { throw "border_color not read: $($config.BorderColor)" }
if ($config.MaxVisible -ne 10) { throw "max_visible not clamped to 10: $($config.MaxVisible)" }
if ($config.GhostText) { throw 'ghost_text=false not honoured' }
if ((Get-HSAccentCode -Name 'apple') -ne '51') { throw 'apple accent' }
if ((Get-HSAccentCode -Name 'rainbow') -ne '141') { throw 'rainbow accent' }
if ((Get-HSAccentCode -Name 'nonsense') -ne '141') { throw 'unknown accent must fall back' }

$budget = Get-HSLayoutBudget -CursorTop 0
if ($budget.W -lt 20) { throw "W too small $($budget.W)" }
if ($budget.VpMax -lt 1) { throw "vpMax invalid $($budget.VpMax)" }
if ($budget.VpMax -gt 10) { throw "vpMax ignored config clamp: $($budget.VpMax)" }
Write-Host "budget W=$($budget.W) compact=$($budget.Compact) vp=$($budget.VpMax) rows=$($budget.AvailableRows) accent=$($budget.Accent)"

# --- one vocabulary for every surface --------------------------------------------
if ((Get-HSSourceLabel -Source 'history-local') -ne 'here') { throw 'source label here' }
if ((Get-HSSourceLabel -Source 'zoxide') -ne 'folders') { throw 'source label folders' }
if ((Get-HSSourceLabel -Source 'wat') -ne 'history') { throw 'source label fallback' }
if ((Get-HSTierTag -Tier 0) -ne '') { throw 'tier 0 must be silent' }
if ((Get-HSTierTag -Tier 3) -ne ([string][char]0x2248)) { throw 'tier 3 typo tag' }
if ((Get-HSTierTag -Tier 4) -ne '~') { throw 'tier 4 fuzzy tag' }

# --- no invented usage counts, and risky items are marked without colour only ----
$contextual = [PSCustomObject]@{ command = 'cd src/'; description = 'directory'; frequency = 0; source = 'path'; tier = 1; risk = 'normal' }
$info = Get-HSItemInfo -Suggestion $contextual
if ($info -match '1x') { throw "contextual candidate invented a count: '$info'" }
if ($info -ne 'path') { throw "unexpected info: '$info'" }

$history = [PSCustomObject]@{ command = 'git status'; description = $null; frequency = 12; source = 'history-global'; tier = 0; risk = 'normal' }
if ((Get-HSItemInfo -Suggestion $history) -ne 'history 12x') { throw 'history info' }
if ((Get-HSFooterText -Suggestion $history) -ne 'used 12x . history') { throw "footer from usage: '$(Get-HSFooterText -Suggestion $history)'" }

$risky = [PSCustomObject]@{ command = 'rm -rf build'; description = 'Force remove'; frequency = 1; source = 'history-global'; tier = 0; risk = 'destructive' }
if (-not (Test-HSDestructive -Suggestion $risky)) { throw 'destructive not detected' }
if ((Get-HSItemInfo -Suggestion $risky) -notmatch '^! ') { throw 'risky info marker missing' }
if ((Get-HSFooterText -Suggestion $risky) -notmatch '^\[!\] risky') { throw 'risky footer missing' }
# Older daemons send no risk field at all: that must read as safe, not crash.
$legacy = [PSCustomObject]@{ command = 'git status'; description = 'x'; frequency = 1; source = 'history' }
if (Test-HSDestructive -Suggestion $legacy) { throw 'missing risk field must be safe' }
# ...but a missing field must not talk the panel out of warning about real damage.
$legacyRisky = [PSCustomObject]@{ command = 'rm -rf build'; description = 'x'; frequency = 1; source = 'history' }
if (-not (Test-HSDestructive -Suggestion $legacyRisky)) { throw 'legacy daemon lost the safety fallback' }
if ((Test-HSDestructive -Suggestion $risky) -ne $true) { throw 'explicit destructive marker lost' }
$safeMarked = [PSCustomObject]@{ command = 'git status'; description = 'x'; frequency = 1; source = 'history'; tier = 0; risk = 'normal' }
if (Test-HSDestructive -Suggestion $safeMarked) { throw 'daemon said normal, panel must believe it' }

# --- daemon metadata survives the projection -----------------------------------------
# tier and risk used to be dropped when rows were rebuilt, so no row could ever show the
# match-quality tag or the risky marker even though the daemon sent both.
function script:Invoke-HSDaemon {
    param($Query, $Limit)
    @(
        [PSCustomObject]@{ command = 'rm -rf build'; description = ''; frequency = 3; source = 'history-global'; tier = 0; risk = 'destructive' },
        [PSCustomObject]@{ command = 'gti status'; description = ''; frequency = 2; source = 'history-global'; tier = 3; risk = 'normal' },
        [PSCustomObject]@{ command = 'docker system prune -af'; description = ''; frequency = 1; source = 'history-global' }
    )
}
$rows = @(Get-HSSuggestions -Typed 'x')
if ($rows.Count -ne 3) { throw "projection changed the row count: $($rows.Count)" }
if (-not (Test-HSDestructive -Suggestion $rows[0])) { throw 'daemon risk marker lost in projection' }
if ([int]$rows[1].tier -ne 3) { throw "daemon tier lost in projection: $($rows[1].tier)" }
if ((Get-HSItemInfo -Suggestion $rows[1]) -notmatch [regex]::Escape([string][char]0x2248)) { throw 'typo tag never reached the info column' }
if (-not (Test-HSDestructive -Suggestion $rows[2])) { throw 'legacy daemon row lost the safety fallback' }
if (Get-HSTierTag -Tier $rows[2].tier) { throw 'a daemon without tiers must not claim match quality' }
function script:Invoke-HSDaemon { param($Query, $Limit) @() }

$noDescription = [PSCustomObject]@{ command = 'git status'; description = ''; frequency = 0; source = 'history'; tier = 0; risk = 'normal' }
if ((Get-HSFooterText -Suggestion $noDescription) -ne 'no description') { throw 'footer fallback' }

# --- inline preview and word-wise acceptance -------------------------------------
if ((Get-HSGhostSuffix -Command 'git checkout main' -Typed 'git ') -ne 'checkout main') { throw 'ghost suffix' }
if ((Get-HSGhostSuffix -Command 'git checkout main' -Typed 'GIT ') -ne 'checkout main') { throw 'ghost suffix case' }
if ((Get-HSGhostSuffix -Command 'rm -rf build' -Typed 'ru') -ne '') { throw 'ghost must refuse non-prefix' }
# One word at a time, no trailing space: same as the live Bash overlay's Alt+Right.
if ((Get-HSNextWordSuffix -Command 'git checkout main' -Typed 'git ') -ne 'checkout') { throw "next word: '$(Get-HSNextWordSuffix -Command 'git checkout main' -Typed 'git ')'" }
if ((Get-HSNextWordSuffix -Command 'git status' -Typed 'docker') -ne '') { throw 'next word must refuse non-prefix' }

$parts = Split-HSMatchedCommand -Command 'git status' -Typed 'stat'
if ($parts.Matched -ne 'stat' -or $parts.Before -ne 'git ' -or $parts.After -ne 'us') { throw "substring match split: $($parts | ConvertTo-Json -Compress)" }
$typo = Split-HSMatchedCommand -Command 'git status' -Typed 'gti'
if ($typo.Matched -ne '') { throw 'typo candidate must not fake a literal highlight' }
if ($typo.After -ne 'git status') { throw 'typo candidate must still show the command' }

# --- row arithmetic can never exceed the terminal width -------------------------
foreach ($width in 20..100) {
  $row = Get-HSRowLayout -Width $width -Info 'history 12x'
  if ($row.Total -gt $width) { throw "row overflows at W=$width (Total=$($row.Total))" }
  $compactRow = Get-HSRowLayout -Width $width -Compact -Info 'history 12x'
  if ($compactRow.Total -gt $width) { throw "compact row overflows at W=$width" }
  if ($compactRow.InfoW -ne 0) { throw 'compact layout must drop the info column' }
}
$wide = Get-HSRowLayout -Width 70 -Info 'history 12x'
if ($wide.InfoW -eq 0) { throw 'wide layout lost the explanation column' }
if ($wide.CmdW -lt 30) { throw "command still too short at W=70: $($wide.CmdW)" }

# --- behaviour: the bound arrow key really moves the visible selection ------------------
# The regression this catches: with a closure the handler incremented SelectedIndex inside a
# copy of $script:HS, so Down did nothing the user could see.
. $handlers
$script:HSDrawSelected = $null
function script:Draw-HSOverlay { param($Suggestions, $SelectedIndex, $TypedSoFar) $script:HSDrawSelected = $SelectedIndex }

$script:HS.IsVisible = $true
$script:HS.Dismissed = $false
$script:HS.ScrollOffset = 0
$script:HS.ViewportSize = 3
$script:HS.CurrentInput = 'git '
$script:HS.Suggestions = @(1..5 | ForEach-Object { [PSCustomObject]@{ command = "git c$_"; description = ''; frequency = 0; source = 'history'; tier = 0; risk = 'normal' } })
$script:HS.SelectedIndex = 0

$down = [ConsoleKeyInfo]::new([char]0, [ConsoleKey]::DownArrow, $false, $false, $false)
$up = [ConsoleKeyInfo]::new([char]0, [ConsoleKey]::UpArrow, $false, $false, $false)
if ((Get-HSNavigationDirection -Key $down) -ne 'Down') { throw 'DownArrow mapped to the wrong direction' }
if ((Get-HSNavigationDirection -Key $up) -ne 'Up') { throw 'UpArrow mapped to the wrong direction' }

$navigation = Get-HSNavigationHandler
& $navigation $down $null
if ($script:HS.SelectedIndex -ne 1) { throw "Down did not move the shared state: $($script:HS.SelectedIndex)" }
if ($script:HSDrawSelected -ne 1) { throw "Down did not redraw the panel with the new row: $($script:HSDrawSelected)" }
& $navigation $down $null
& $navigation $up $null
if ($script:HS.SelectedIndex -ne 1) { throw "Up did not move back: $($script:HS.SelectedIndex)" }

# One scriptblock serves both arrows, exactly as the module installs it.
if ((Get-HSNavigationHandler).ToString() -notmatch 'Get-HSNavigationDirection') { throw 'navigation handler lost its key-driven direction' }

$plainRight = [ConsoleKeyInfo]::new([char]0, [ConsoleKey]::RightArrow, $false, $false, $false)
$altRight = [ConsoleKeyInfo]::new([char]0, [ConsoleKey]::RightArrow, $false, $true, $false)
if (Test-HSWordAcceptKey -Key $plainRight) { throw 'plain Right must accept the whole inline hint' }
if (-not (Test-HSWordAcceptKey -Key $altRight)) { throw 'Alt+Right must accept a single word' }
Write-Host 'OK behaviour arrow keys move the shared selection state'

Write-Host 'UNIT OK'
}
finally {
  Remove-Item -LiteralPath $configHome -Recurse -Force -ErrorAction SilentlyContinue
  $env:HINTSHELL_HOME = $previousHome
}