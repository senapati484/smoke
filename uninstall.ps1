# SMOKE Uninstaller (Windows / PowerShell)
# Removes SMOKE hooks/MCP entries from all AI tool configs, deletes the
# binary, and cleans up the PATH.
#
# Run:
#   Set-ExecutionPolicy RemoteSigned -Scope CurrentUser
#   .\uninstall.ps1

$ErrorActionPreference = "Stop"

Write-Host "╔══════════════════════════════════════╗" -ForegroundColor Blue
Write-Host "║       SMOKE Uninstaller              ║" -ForegroundColor Blue
Write-Host "╚══════════════════════════════════════╝" -ForegroundColor Blue
Write-Host ""

$InstallDir  = Join-Path $Home ".smoke\bin"
$Binary      = Join-Path $InstallDir "smoke.exe"
$SmokeDir    = Join-Path $Home ".smoke"

# ── Step 1: Remove tool registrations ────────────────────────────────────────
Write-Host "1. Removing SMOKE from all AI tool configurations..." -ForegroundColor Blue

if (Test-Path $Binary) {
    & $Binary uninstall --tools all
} else {
    Write-Host "   Binary not found at $Binary — skipping config cleanup." -ForegroundColor Yellow
    Write-Host "   You may need to manually remove SMOKE entries from:"
    Write-Host "     • ~\.claude\settings.json                       (Claude Code)"
    Write-Host "     • %APPDATA%\Claude\claude_desktop_config.json   (Claude Desktop)"
    Write-Host "     • ~\.codeium\windsurf\mcp_config.json           (Windsurf)"
    Write-Host "     • ~\.cursor\mcp.json                            (Cursor)"
}

# ── Step 2: Remove binary and data directory ──────────────────────────────────
Write-Host "`n2. Removing SMOKE binary and data directory..." -ForegroundColor Blue

if (Test-Path $SmokeDir) {
    Remove-Item $SmokeDir -Recurse -Force
    Write-Host "   Removed $SmokeDir" -ForegroundColor Green
} else {
    Write-Host "   $SmokeDir not found — already removed." -ForegroundColor Yellow
}

# ── Step 3: Remove PATH entry ─────────────────────────────────────────────────
Write-Host "`n3. Cleaning up PATH..." -ForegroundColor Blue

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -like "*$InstallDir*") {
    $NewPath = ($UserPath -split ';' | Where-Object { $_ -ne $InstallDir }) -join ';'
    $NewPath = $NewPath -replace ';;', ';'
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "   Removed $InstallDir from User PATH." -ForegroundColor Green
} else {
    Write-Host "   PATH entry not found — already removed." -ForegroundColor Yellow
}

# ── Done ──────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "╔══════════════════════════════════════╗" -ForegroundColor Green
Write-Host "║     SMOKE uninstalled successfully!  ║" -ForegroundColor Green
Write-Host "╚══════════════════════════════════════╝" -ForegroundColor Green
Write-Host ""
Write-Host "SMOKE has been fully removed."
Write-Host "Open a new terminal window for PATH changes to take effect."
Write-Host ""
Write-Host "To reinstall later:"
Write-Host "  irm https://raw.githubusercontent.com/senapati484/smoke/main/install.ps1 | iex" -ForegroundColor Cyan
Write-Host ""
