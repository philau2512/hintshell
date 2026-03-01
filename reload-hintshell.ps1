# Script to reload HintShell module

Write-Host "🔄 Reloading HintShell..." -ForegroundColor Cyan

# 1. Stop daemon if running
Write-Host "Stopping daemon..." -ForegroundColor Yellow

# Try stopping via new CLI name
$cliPath = ".\integrations\powershell\HintShellModule\hintshell.exe"
if (Test-Path $cliPath) {
    & $cliPath stop
    Start-Sleep -Milliseconds 500
}

# 2. Remove module if loaded
Write-Host "Removing old module..." -ForegroundColor Yellow
Remove-Module HintShellModule -ErrorAction SilentlyContinue
Remove-Module ShellMindModule -ErrorAction SilentlyContinue

# 2.5 Copy default-commands.json to module directory (for daemon seeding)
$defaultsJson = ".\core\default-commands.json"
$moduleDir = ".\integrations\powershell\HintShellModule"
if (Test-Path $defaultsJson) {
    if (-not (Test-Path $moduleDir)) { New-Item -ItemType Directory -Path $moduleDir -Force }
    Copy-Item $defaultsJson $moduleDir -Force
}

# 3. Import module
Write-Host "Importing module..." -ForegroundColor Yellow
$modulePsm = Join-Path $moduleDir "HintShellModule.psm1"
if (Test-Path $modulePsm) {
    Import-Module $modulePsm -Force
} else {
    Write-Error "❌ HintShellModule.psm1 not found at $modulePsm"
    return
}

# 4. Start HintShell
Write-Host "Starting HintShell..." -ForegroundColor Yellow
if (Get-Command Start-HintShell -ErrorAction SilentlyContinue) {
    Start-HintShell
} else {
    Write-Error "❌ Start-HintShell function not found. Module import failed?"
    return
}

Write-Host "`n✅ HintShell reloaded successfully!" -ForegroundColor Green
Write-Host "Try typing: git" -ForegroundColor Gray
