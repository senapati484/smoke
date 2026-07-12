# SMOKE Installation Script (Windows / PowerShell)
# Builds SMOKE from source, installs it to ~/.smoke/bin, then delegates
# all AI tool registration to `smoke install` — no Python or Node required.
#
# Run from an elevated PowerShell window or from inside the cloned repo:
#   Set-ExecutionPolicy RemoteSigned -Scope CurrentUser
#   .\install.ps1

$ErrorActionPreference = "Stop"

# ── Header ────────────────────────────────────────────────────────────────────
Write-Host "╔══════════════════════════════════════╗" -ForegroundColor Blue
Write-Host "║        SMOKE Installer               ║" -ForegroundColor Blue
Write-Host "╚══════════════════════════════════════╝" -ForegroundColor Blue
Write-Host ""

# ── Step 1: Check Rust / Cargo ────────────────────────────────────────────────
Write-Host "1. Checking prerequisites..." -ForegroundColor Blue
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Error: Rust/Cargo is not installed." -ForegroundColor Red
    Write-Host "Install Rust from https://rustup.rs, open a new PowerShell window, and try again."
    exit 1
}
$cargoVersion = (cargo --version)
Write-Host "   $cargoVersion found." -ForegroundColor Green

# ── Step 2: Clone repo if not running inside it ───────────────────────────────
$InRepo = $false
if (Test-Path "Cargo.toml") {
    $content = Get-Content "Cargo.toml" -Raw
    if ($content -like '*name = "smoke"*') { $InRepo = $true }
}

$TempDir = $null
$OriginalDir = Get-Location

if (-not $InRepo) {
    Write-Host "`n2. Cloning SMOKE from GitHub..." -ForegroundColor Blue
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Host "Error: git is not installed." -ForegroundColor Red
        Write-Host "Install git or run this script from inside the cloned smoke repository."
        exit 1
    }
    $TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $TempDir -Force | Out-Null
    git clone https://github.com/senapati484/smoke.git $TempDir
    Set-Location $TempDir
} else {
    Write-Host "   Running inside smoke repository — skipping clone." -ForegroundColor Green
}

# ── Step 3: Build ─────────────────────────────────────────────────────────────
Write-Host "`n3. Building SMOKE in release mode (this takes ~2 min on first run)..." -ForegroundColor Blue
cargo build --release

# ── Step 4: Install binary ────────────────────────────────────────────────────
Write-Host "`n4. Installing binary to ~\.smoke\bin..." -ForegroundColor Blue
$InstallDir = Join-Path $Home ".smoke\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$SrcBinary = "target\release\smoke.exe"
if (-not (Test-Path $SrcBinary)) {
    Write-Host "Error: Compiled binary not found at $SrcBinary" -ForegroundColor Red
    if ($TempDir) { Set-Location $OriginalDir; Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue }
    exit 1
}

$DestBinary = Join-Path $InstallDir "smoke.exe"
Copy-Item $SrcBinary $DestBinary -Force
Write-Host "   Installed to $DestBinary" -ForegroundColor Green

# ── Step 5: Add to PATH ───────────────────────────────────────────────────────
Write-Host "`n5. Configuring PATH..." -ForegroundColor Blue
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    $NewPath = ($UserPath.TrimEnd(';') + ";$InstallDir") -replace ';;', ';'
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    Write-Host "   Added $InstallDir to User PATH." -ForegroundColor Green
    Write-Host "   Note: Open a NEW terminal window for PATH to take effect." -ForegroundColor Yellow
} else {
    Write-Host "   PATH already configured." -ForegroundColor Green
}

# Also available in this session
$env:PATH = "$InstallDir;$env:PATH"

# ── Step 6: Choose tools to register ─────────────────────────────────────────
Write-Host ""
$SelectedAgents = "all"

if ([Environment]::UserInteractive) {
    Write-Host "6. Which AI tools should SMOKE register with?" -ForegroundColor Blue
    Write-Host "   1) All supported tools              [default]" -ForegroundColor Cyan
    Write-Host "   2) Claude Code only (hooks)"
    Write-Host "   3) Claude Desktop (MCP server)"
    Write-Host "   4) Windsurf (MCP server)"
    Write-Host "   5) Cursor (MCP server)"
    Write-Host "   6) Cline / Roo Code (MCP server)"
    Write-Host "   7) Custom — enter tool keys manually"
    Write-Host "   8) Skip registration"
    Write-Host ""
    $choice = Read-Host "   Enter choice [1]"
    if ([string]::IsNullOrWhiteSpace($choice)) { $choice = "1" }

    switch ($choice) {
        "1" { $SelectedAgents = "all" }
        "2" { $SelectedAgents = "claude-code" }
        "3" { $SelectedAgents = "claude-desktop" }
        "4" { $SelectedAgents = "windsurf" }
        "5" { $SelectedAgents = "cursor" }
        "6" { $SelectedAgents = "cline" }
        "7" {
            $SelectedAgents = Read-Host "   Enter comma-separated tools (e.g. claude-code,cursor)"
        }
        "8" { $SelectedAgents = "" }
        default { $SelectedAgents = "all" }
    }
} else {
    Write-Host "   Non-interactive mode — registering all tools." -ForegroundColor Yellow
}

# ── Step 7: Register ──────────────────────────────────────────────────────────
if (-not [string]::IsNullOrWhiteSpace($SelectedAgents)) {
    Write-Host "`n7. Registering SMOKE..." -ForegroundColor Blue
    & $DestBinary install --tools $SelectedAgents
} else {
    Write-Host "   Skipping registration. Run 'smoke install' later to configure." -ForegroundColor Yellow
}

# ── Step 8: Cleanup ───────────────────────────────────────────────────────────
if ($TempDir -and (Test-Path $TempDir)) {
    Set-Location $OriginalDir
    Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}

# ── Done ──────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "╔══════════════════════════════════════╗" -ForegroundColor Green
Write-Host "║     SMOKE installed successfully!    ║" -ForegroundColor Green
Write-Host "╚══════════════════════════════════════╝" -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Open a NEW terminal window (for PATH to take effect)"
Write-Host "  2. Check status:          " -NoNewline; Write-Host "smoke status" -ForegroundColor Cyan
Write-Host "  3. Verify JS sandbox:     " -NoNewline; Write-Host "smoke test --code 'console.log(42)' --lang js" -ForegroundColor Cyan
Write-Host "  4. Verify Python sandbox: " -NoNewline; Write-Host "smoke test --code 'print(42)' --lang py" -ForegroundColor Cyan
Write-Host ""
Write-Host "Manage later:"
Write-Host "  Add a tool:    " -NoNewline; Write-Host "smoke install --tools cursor" -ForegroundColor Cyan
Write-Host "  Remove a tool: " -NoNewline; Write-Host "smoke uninstall --tools claude-desktop" -ForegroundColor Cyan
Write-Host "  Uninstall all: " -NoNewline; Write-Host ".\uninstall.ps1" -ForegroundColor Cyan
Write-Host ""
Write-Host "Docs: https://github.com/senapati484/smoke"
Write-Host ""
