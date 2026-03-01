# Script to reload HintShell module (dev mode)
# Copies fresh binaries from target/release/ into the local module directory

Write-Host "🔄 Reloading HintShell..." -ForegroundColor Cyan

$moduleDir = ".\integrations\powershell\HintShellModule"
$releaseBin = ".\target\release"

# 1. Stop daemon if running
Write-Host "Stopping daemon..." -ForegroundColor Yellow
$cliPath = Join-Path $moduleDir "hintshell.exe"
if (Test-Path $cliPath) {
    & $cliPath stop 2>$null
    Start-Sleep -Milliseconds 500
}
# Also kill any stray daemon processes
Get-Process hintshell-core -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# 2. Remove module if loaded
Write-Host "Removing old module..." -ForegroundColor Yellow
Remove-Module HintShellModule -ErrorAction SilentlyContinue

# 3. Copy fresh binaries + data from build output
Write-Host "Copying fresh binaries..." -ForegroundColor Yellow
if (Test-Path $releaseBin) {
    $coreBin = Join-Path $releaseBin "hintshell-core.exe"
    $cliBin = Join-Path $releaseBin "hintshell.exe"
    if (Test-Path $coreBin) { Copy-Item $coreBin $moduleDir -Force }
    if (Test-Path $cliBin) { Copy-Item $cliBin $moduleDir -Force }
}
# Copy default-commands.json
$defaultsJson = ".\core\default-commands.json"
if (Test-Path $defaultsJson) { Copy-Item $defaultsJson $moduleDir -Force }

# 3.5 Sync to ~/.hintshell/ (so new terminals get latest code)
Write-Host "Syncing to ~/.hintshell/..." -ForegroundColor Yellow
$installModuleDir = Join-Path $env:USERPROFILE ".hintshell\module"
if (Test-Path $installModuleDir) {
    Copy-Item "$moduleDir\*" $installModuleDir -Recurse -Force
}
$installRoot = Join-Path $env:USERPROFILE ".hintshell"
if (Test-Path $defaultsJson) { Copy-Item $defaultsJson $installRoot -Force }
# Remove disabled flag so dev reload always starts fresh
$disabledFile = Join-Path $installRoot ".disabled"
if (Test-Path $disabledFile) { Remove-Item $disabledFile -Force }

# 4. Import module
Write-Host "Importing module..." -ForegroundColor Yellow
$modulePsm = Join-Path $moduleDir "HintShellModule.psm1"
if (Test-Path $modulePsm) {
    Import-Module $modulePsm -Force
} else {
    Write-Error "❌ HintShellModule.psm1 not found at $modulePsm"
    return
}

# 5. Start HintShell
Write-Host "Starting HintShell..." -ForegroundColor Yellow
if (Get-Command Start-HintShell -ErrorAction SilentlyContinue) {
    Start-HintShell
} else {
    Write-Error "❌ Start-HintShell function not found. Module import failed?"
    return
}

Write-Host "`n✅ HintShell reloaded successfully!" -ForegroundColor Green
Write-Host "Try typing: git" -ForegroundColor Gray
