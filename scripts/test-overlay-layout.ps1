$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Set-Location $repo

$overlay = Join-Path $repo 'integrations/powershell/HintShellModule/HintShellOverlay.ps1'
$handlers = Join-Path $repo 'integrations/powershell/HintShellModule/HintShellHandlers.ps1'

foreach ($f in @($overlay, $handlers)) {
  $tokens = $null; $errors = $null
  $null = [System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$tokens, [ref]$errors)
  if ($errors -and $errors.Count -gt 0) {
    Write-Host "FAIL $f"
    $errors | ForEach-Object { Write-Host $_.ToString() }
    exit 1
  }
  Write-Host "OK parse $f"
}

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

# Layout budget uses safe console defaults when handle invalid (CI/headless)
$budget = Get-HSLayoutBudget -CursorTop 0
if ($budget.W -lt 20) { throw "W too small $($budget.W)" }
if ($budget.VpMax -lt 1) { throw "vpMax invalid $($budget.VpMax)" }
Write-Host "budget W=$($budget.W) compact=$($budget.Compact) vp=$($budget.VpMax) rows=$($budget.AvailableRows)"
Write-Host 'UNIT OK'