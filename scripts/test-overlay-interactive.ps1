$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
Set-Location $repo

# This is intentionally an operator-driven test. PowerShell cannot safely synthesize
# real terminal keystrokes from this script, so each behavior is confirmed manually.
if (-not [Environment]::UserInteractive -or -not $Host.UI.RawUI -or [Console]::IsInputRedirected -or [Console]::IsOutputRedirected) {
    Write-Host 'FAIL interactive terminal required (run this from a real pwsh console).' -ForegroundColor Red
    exit 2
}

$moduleRoot = Join-Path $repo 'integrations/powershell/HintShellModule'
$manifest = Join-Path $moduleRoot 'HintShellModule.psd1'
$moduleFile = Join-Path $moduleRoot 'HintShellModule.psm1'
$sourceFiles = @(
    $moduleFile,
    (Join-Path $moduleRoot 'HintShellDaemon.ps1'),
    (Join-Path $moduleRoot 'HintShellOverlay.ps1'),
    (Join-Path $moduleRoot 'HintShellHandlers.ps1')
)
$timeoutMs = 1200
$moduleName = 'HintShellModule'
$oldHintshellHome = $env:HINTSHELL_HOME
$oldHintshellDataHome = $env:HINTSHELL_DATA_HOME
$tempHome = Join-Path ([IO.Path]::GetTempPath()) ("hintshell-overlay-regression-{0}" -f ([guid]::NewGuid().ToString('N')))
$tempDataHome = Join-Path $tempHome 'data'
$results = [System.Collections.Generic.List[object]]::new()
$imported = $false

function Read-HSOperatorLine {
    Write-Host 'Enter the requested test input below, then press Enter. The command is captured only and is not executed by this harness.' -ForegroundColor DarkGray
    return [Microsoft.PowerShell.PSConsoleReadLine]::ReadLine($Host.Runspace, $ExecutionContext, $null)
}

function Add-OperatorResult {
    param([string]$Name, [string]$Instruction)
    Write-Host "`n[$Name]" -ForegroundColor Cyan
    Write-Host $Instruction
    $capturedLine = Read-HSOperatorLine
    Write-Host "Captured input: <$capturedLine>" -ForegroundColor DarkGray
    do {
        $answer = (Read-Host 'Result: P=pass, F=fail, S=skip').Trim().ToUpperInvariant()
    } while ($answer -notin @('P', 'F', 'S'))
    $results.Add([pscustomobject]@{ Name = $Name; Result = $answer })
}

try {
    Write-Host 'HintShell interactive overlay regression test' -ForegroundColor White
    Write-Host 'Run only in a disposable interactive pwsh session. This script does not automate keystrokes.' -ForegroundColor Yellow

    foreach ($source in $sourceFiles) {
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "Missing source: $source" }
        $tokens = $null; $errors = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile($source, [ref]$tokens, [ref]$errors)
        if ($errors -and $errors.Count -gt 0) { throw "Parse failed: $source" }
        Write-Host "OK parse $source" -ForegroundColor Green
    }
    $moduleText = Get-Content -LiteralPath $moduleFile -Raw
    if ($moduleText -notmatch 'Test-HSDaemonAlive\s+-TimeoutMs') { throw 'Daemon timeout source check failed' }
    if ($timeoutMs -lt 100 -or $timeoutMs -gt 10000) { throw "Timeout sanity check failed: $timeoutMs ms" }
    Write-Host "OK source/timeout sanity (daemon timeout=$timeoutMs ms)" -ForegroundColor Green
    Write-Host 'Static source rules are pinned by scripts/test-overlay-layout.ps1; this harness only checks what a human must see.' -ForegroundColor DarkGray

    $psReadLine = Get-Module -ListAvailable -Name PSReadLine | Select-Object -First 1
    if (-not $psReadLine) { throw 'PSReadLine is not available' }
    Import-Module PSReadLine -ErrorAction Stop
    if (-not ('Microsoft.PowerShell.PSConsoleReadLine' -as [type])) { throw 'PSReadLine console type unavailable' }
    Write-Host "OK PSReadLine available/imported ($($psReadLine.Version))" -ForegroundColor Green

    New-Item -ItemType Directory -Path $tempHome -Force | Out-Null
    New-Item -ItemType Directory -Path $tempDataHome -Force | Out-Null
    $env:HINTSHELL_HOME = $tempHome
    $env:HINTSHELL_DATA_HOME = $tempDataHome
    # Deterministic config so the "4 rows / red frame" check below means something.
    @(
        'border_color = "red"',
        'max_visible = 4',
        'ghost_text = true'
    ) | Set-Content -LiteralPath (Join-Path $tempHome 'config.toml') -Encoding utf8
    Write-Host "Config under test: border_color=red max_visible=4 ghost_text=true ($tempHome)" -ForegroundColor DarkGray
    Import-Module $manifest -Force -DisableNameChecking -ErrorAction Stop
    $imported = $true
    Start-HintShell -Force -Quiet
    Write-Host 'OK HintShell module imported and startup completed.' -ForegroundColor Green

    Add-OperatorResult 'Config is honoured' 'Type `git` and count the suggestion rows. Expect at most 4 rows and a red frame, matching the config.toml written above and `hs config`.'
    Add-OperatorResult 'Slow typing' 'Type a short command one character at a time, pausing between keys. Confirm suggestions/overlay remain stable and the line stays editable.'
    Add-OperatorResult 'Fast typing' 'Type a short command quickly. Confirm the overlay does not corrupt, lag indefinitely, or consume characters.'
    Add-OperatorResult 'Single-line paste' 'Paste one short command. Confirm it is inserted as one line and does not trigger one suggestion request per pasted character.'
    Add-OperatorResult 'Multiline paste' 'Paste text containing a newline. Confirm multiline input remains usable and the overlay is suppressed or behaves safely.'
    Add-OperatorResult 'Enter with daemon available' 'Type a harmless command such as `Write-Output overlay-test`, press Enter, and confirm the line is accepted once without a stuck overlay. The harness captures it but does not execute it.'

    # --- Presentation and key-affordance matrix (the new UI contract) ---
    Add-OperatorResult 'Non-ASCII input still suggests' 'Type a command containing Vietnamese or other non-ASCII text, for example: Write-Output tài liệu. Confirm suggestions still appear for the rest of the line: the old ASCII-only guard is gone.'
    Add-OperatorResult 'Typo tolerance is visible' 'Type `gti` (a transposition of git). Confirm at least one row appears and carries the ~= typo tag rather than vanishing.'
    Add-OperatorResult 'Substring and fuzzy rows are not filtered' 'Type `com` or `sh`. Confirm rows whose command does not start with that text are still listed instead of an empty panel.'
    Add-OperatorResult 'Source labels' 'Type `git ` and read the right-hand column. Confirm labels such as here / history / most used / starter / path instead of raw source keys.'
    Add-OperatorResult 'Right arrow accepts the inline hint' 'Type `git st`, wait for the hint, then press Right once. Confirm the suffix is inserted onto the line and nothing is executed.'
    Add-OperatorResult 'Alt+Right accepts one word' 'Type `git ` with a multi-word suggestion visible, press Alt+Right. Confirm exactly one word is inserted, then repeat.'
    Add-OperatorResult 'Destructive rows are marked and refused by arrow' 'Select a risky row such as `rm -rf` or `git reset --hard` (navigate with Up/Down). Confirm it shows ! and "[!] risky", and that Right does NOT insert it; Tab still accepts it deliberately.'
    Add-OperatorResult 'Escape is sticky' 'With the panel open press Escape, then keep typing two or three characters. Confirm the panel does not reappear. Press Ctrl+Space and confirm it comes back.'
    Add-OperatorResult 'Counter and more indicator' 'Type something with many matches (for example `git`), then shrink the terminal height until only 2-3 rows fit. Confirm a visible N/total counter or more-marker so hidden matches are discoverable.'
    Add-OperatorResult 'Narrow terminal layout' 'Resize the window to about 40 columns and type `git `. Confirm no row wraps to a second line and the frame stays inside the window; the description column may disappear.'
    Add-OperatorResult 'Ghost fallback when a frame cannot fit' 'With ghost_text=true, put the cursor near the bottom so fewer than 3 rows remain. Confirm an inline grey hint still appears instead of nothing at all.'
    Add-OperatorResult 'Light theme readability' 'Switch the terminal to a light colour scheme and open the panel. Confirm every row is readable: no forced bright-white text, selected row distinguishable without colour alone.'

    Write-Host "`n[Daemon unavailable]" -ForegroundColor Cyan
    Write-Host 'The next command stops this test daemon through the module cleanup path. Do not stop unrelated HintShell daemons.'
    hs stop
    Add-OperatorResult 'Enter with daemon unavailable' 'Type a harmless command such as `Write-Output overlay-test-offline`, press Enter, and confirm the line is accepted once without a stuck overlay. The harness captures it but does not execute it.'

    Write-Host "`nRestarting module startup for cleanup-path verification..." -ForegroundColor DarkGray
    Import-Module $manifest -Force -DisableNameChecking -ErrorAction Stop
    Start-HintShell -Force -Quiet
    Write-Host 'OK module can start again after unavailable-daemon case.' -ForegroundColor Green

    $failed = @($results | Where-Object Result -eq 'F')
    $skipped = @($results | Where-Object Result -eq 'S')
    if ($failed.Count -gt 0) {
        Write-Host "`nFAIL operator checks: $($failed.Name -join ', ')" -ForegroundColor Red
        exit 1
    }
    if ($skipped.Count -gt 0) {
        Write-Host "`nFAIL incomplete operator checks (skipped): $($skipped.Name -join ', ')" -ForegroundColor Red
        exit 1
    }
    Write-Host "`nPASS all $($results.Count) interactive overlay checks" -ForegroundColor Green
    exit 0
} catch {
    Write-Host "`nFAIL $($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    if ($imported) {
        try { Stop-HintShell } catch { Write-Host "Cleanup warning: $($_.Exception.Message)" -ForegroundColor Yellow }
        Remove-Module $moduleName -Force -ErrorAction SilentlyContinue
    }
    if ($null -eq $oldHintshellHome) { Remove-Item Env:HINTSHELL_HOME -ErrorAction SilentlyContinue }
    else { $env:HINTSHELL_HOME = $oldHintshellHome }
    if ($null -eq $oldHintshellDataHome) { Remove-Item Env:HINTSHELL_DATA_HOME -ErrorAction SilentlyContinue }
    else { $env:HINTSHELL_DATA_HOME = $oldHintshellDataHome }
    if (Test-Path -LiteralPath $tempHome) { Remove-Item -LiteralPath $tempHome -Recurse -Force -ErrorAction SilentlyContinue }
}
