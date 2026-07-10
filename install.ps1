# SMOKE Installation Script (Windows)
# Compiles SMOKE from source, installs it in ~/.smoke/bin, and registers it in Claude Code & MCP clients.

$ErrorActionPreference = "Stop"

Write-Host "=== Starting SMOKE Installation ===" -ForegroundColor Blue

# 1. Verify cargo/rust installation
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Error: Rust/Cargo is not installed." -ForegroundColor Red
    Write-Host "Please install Rust from https://rustup.rs and run this script again in a new PowerShell window."
    exit 1
}

# Check if we are running from the smoke repository root
$InRepo = $false
if (Test-Path "Cargo.toml") {
    $Content = Get-Content "Cargo.toml" -Raw
    if ($Content -like '*name = "smoke"*') {
        $InRepo = $true
    }
}

$TempDir = $null
if (-not $InRepo) {
    Write-Host "Not running inside smoke repository. Cloning smoke from GitHub to a temporary directory..." -ForegroundColor Yellow
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        Write-Host "Error: git is not installed." -ForegroundColor Red
        Write-Host "Please install git or run the installer script from inside the cloned repository."
        exit 1
    }
    
    $TempParent = [System.IO.Path]::GetTempPath()
    $TempDir = Join-Path $TempParent ([System.Guid]::NewGuid().ToString())
    New-Item -ItemType Directory -Path $TempDir -Force | Out-Null
    
    git clone https://github.com/senapati484/smoke.git $TempDir
    $OriginalDir = Get-Location
    Set-Location $TempDir
}

# 2. Build in release mode
Write-Host "`n1. Building SMOKE in release mode..." -ForegroundColor Blue
cargo build --release

# 3. Create target directory
Write-Host "`n2. Creating target installation directory..." -ForegroundColor Blue
$InstallDir = Join-Path $Home ".smoke\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# 4. Copy binary
Write-Host "`n3. Copying SMOKE binary..." -ForegroundColor Blue
$SrcBinary = "target\release\smoke.exe"
if (-not (Test-Path $SrcBinary)) {
    Write-Host "Error: Compiled binary not found at $SrcBinary" -ForegroundColor Red
    if ($TempDir) {
        Set-Location $OriginalDir
        Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
    exit 1
}
Copy-Item $SrcBinary $InstallDir\smoke.exe -Force
Write-Host "Copied to $InstallDir\smoke.exe" -ForegroundColor Green

# 5. Configure PATH
Write-Host "`n4. Configuring PATH environment variable..." -ForegroundColor Blue
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    $NewUserPath = "$UserPath;$InstallDir"
    # Clean up double semicolons if any
    $NewUserPath = $NewUserPath -replace ';;', ';'
    [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
    Write-Host "Added $InstallDir to User PATH." -ForegroundColor Green
    Write-Host "Note: You will need to open a NEW terminal window for PATH changes to take effect." -ForegroundColor Yellow
} else {
    Write-Host "PATH already contains $InstallDir."
}

# 6. Determine which agents to configure
$SelectedAgents = @()

if ([Environment]::UserInteractive) {
    Write-Host "`n5. Choose AI tools to configure SMOKE for:" -ForegroundColor Blue
    Write-Host "  1) Claude Code (CLI Pre/Post-Tool Hooks) [Default]"
    Write-Host "  2) Claude Desktop App (MCP Server)"
    Write-Host "  3) Windsurf IDE (MCP Server)"
    Write-Host "  4) Cline & Roo Code VS Code Extensions (MCP Server)"
    Write-Host "  5) All of the above"
    Write-Host "  6) Skip automatic registration"
    $choice = Read-Host "Select options (e.g. 1,2 or 5) [1]"
    
    if ([string]::IsNullOrWhiteSpace($choice)) {
        $choice = "1"
    }

    if ($choice -eq "5") {
        $SelectedAgents = @("claude-code", "claude-desktop", "windsurf", "cline")
    } elseif ($choice -eq "6") {
        $SelectedAgents = @()
    } else {
        $parts = $choice -split ','
        foreach ($p in $parts) {
            $p = $p.Trim()
            if ($p -eq "1") { $SelectedAgents += "claude-code" }
            if ($p -eq "2") { $SelectedAgents += "claude-desktop" }
            if ($p -eq "3") { $SelectedAgents += "windsurf" }
            if ($p -eq "4") { $SelectedAgents += "cline" }
        }
    }
} else {
    $SelectedAgents = @("claude-code")
}

# Helper to register MCP servers in target JSON paths
function Register-McpServer {
    param(
        [string]$Path,
        [string]$BinaryPath,
        [hashtable]$ExtraKeys = $null
    )
    $ResolvedPath = [System.IO.Path]::GetFullPath($Path.Replace("~", $Home))
    
    if (Test-Path $ResolvedPath) {
        try {
            $Data = Get-Content $ResolvedPath -Raw | ConvertFrom-Json
        } catch {
            $Data = [PSCustomObject]@{ }
        }
    } else {
        $Data = [PSCustomObject]@{ }
    }

    if (-not $Data.PSObject.Properties['mcpServers']) {
        $Data | Add-Member -MemberType NoteProperty -Name "mcpServers" -Value ([PSCustomObject]@{ }) -Force
    }

    $ServerConfig = [PSCustomObject]@{
        command = $BinaryPath
        args = @("server")
    }
    if ($ExtraKeys) {
        foreach ($k in $ExtraKeys.Keys) {
            $ServerConfig | Add-Member -MemberType NoteProperty -Name $k -Value $ExtraKeys[$k] -Force
        }
    }

    $Data.mcpServers | Add-Member -MemberType NoteProperty -Name "smoke" -Value $ServerConfig -Force

    $ParentDir = Split-Path $ResolvedPath -Parent
    if (-not (Test-Path $ParentDir)) {
        New-Item -ItemType Directory -Path $ParentDir -Force | Out-Null
    }

    $JsonStr = ConvertTo-Json $Data -Depth 100
    [System.IO.File]::WriteAllText($ResolvedPath, $JsonStr)
    Write-Host "Registered SMOKE MCP server in $ResolvedPath" -ForegroundColor Green
}

# Run selected updates
if ($SelectedAgents.Count -gt 0) {
    Write-Host "`n6. Registering SMOKE configuration..." -ForegroundColor Blue
    $BinaryPath = Join-Path $Home ".smoke\bin\smoke.exe"

    if ($SelectedAgents -contains "claude-code") {
        try {
            $SettingsPath = Join-Path $Home ".claude\settings.json"
            if (Test-Path $SettingsPath) {
                try {
                    $Settings = Get-Content $SettingsPath -Raw | ConvertFrom-Json
                } catch {
                    $Settings = [PSCustomObject]@{ }
                }
            } else {
                $Settings = [PSCustomObject]@{ }
            }

            if (-not $Settings.PSObject.Properties['hooks']) {
                $Settings | Add-Member -MemberType NoteProperty -Name "hooks" -Value ([PSCustomObject]@{ })
            }
            $Hooks = $Settings.hooks

            if (-not $Hooks.PSObject.Properties['PreToolUse'] -or $Hooks.PreToolUse -eq $null) {
                $Hooks | Add-Member -MemberType NoteProperty -Name "PreToolUse" -Value @() -Force
            }
            if (-not $Hooks.PSObject.Properties['PostToolUse'] -or $Hooks.PostToolUse -eq $null) {
                $Hooks | Add-Member -MemberType NoteProperty -Name "PostToolUse" -Value @() -Force
            }

            $PreHookPayload = [PSCustomObject]@{
                type = "command"
                command = "$BinaryPath hook"
                timeout = 10
                statusMessage = "SMOKE: verifying code..."
            }

            $PostHookPayload = [PSCustomObject]@{
                type = "command"
                command = "$BinaryPath post-hook"
                timeout = 30
            }

            # Update PreToolUse
            $PreEntry = $Hooks.PreToolUse | Where-Object { $_.matcher -eq "Write|Edit" } | Select-Object -First 1
            if (-not $PreEntry) {
                $PreEntry = [PSCustomObject]@{
                    matcher = "Write|Edit"
                    hooks = @()
                }
                $Hooks.PreToolUse += $PreEntry
            }
            $NewPreHooks = @()
            foreach ($h in $PreEntry.hooks) {
                if (-not $h.command.Contains("smoke hook")) { $NewPreHooks += $h }
            }
            $NewPreHooks += $PreHookPayload
            $PreEntry.hooks = $NewPreHooks

            # Update PostToolUse
            $PostEntry = $Hooks.PostToolUse | Where-Object { $_.matcher -eq "Write|Edit" } | Select-Object -First 1
            if (-not $PostEntry) {
                $PostEntry = [PSCustomObject]@{
                    matcher = "Write|Edit"
                    hooks = @()
                }
                $Hooks.PostToolUse += $PostEntry
            }
            $NewPostHooks = @()
            foreach ($h in $PostEntry.hooks) {
                if (-not $h.command.Contains("smoke post-hook")) { $NewPostHooks += $h }
            }
            $NewPostHooks += $PostHookPayload
            $PostEntry.hooks = $NewPostHooks

            $ParentDir = Split-Path $SettingsPath -Parent
            if (-not (Test-Path $ParentDir)) { New-Item -ItemType Directory -Path $ParentDir -Force | Out-Null }
            $JsonStr = ConvertTo-Json $Settings -Depth 100
            [System.IO.File]::WriteAllText($SettingsPath, $JsonStr)
            Write-Host "Registered SMOKE in Claude Code hooks successfully." -ForegroundColor Green
        } catch {
            Write-Host "Failed to configure Claude Code: $_" -ForegroundColor Red
        }
    }

    if ($SelectedAgents -contains "claude-desktop") {
        try {
            $Path = Join-Path $env:APPDATA "Claude\claude_desktop_config.json"
            Register-McpServer -Path $Path -BinaryPath $BinaryPath
        } catch {
            Write-Host "Failed to configure Claude Desktop: $_" -ForegroundColor Red
        }
    }

    if ($SelectedAgents -contains "windsurf") {
        try {
            $Path = Join-Path $Home ".codeium\windsurf\mcp_config.json"
            Register-McpServer -Path $Path -BinaryPath $BinaryPath
        } catch {
            Write-Host "Failed to configure Windsurf: $_" -ForegroundColor Red
        }
    }

    if ($SelectedAgents -contains "cline") {
        try {
            $ClinePath = Join-Path $env:APPDATA "Code\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json"
            $RooPath = Join-Path $env:APPDATA "Code\User\globalStorage\rooveterinaryinc.roo-cline\settings\cline_mcp_settings.json"
            Register-McpServer -Path $ClinePath -BinaryPath $BinaryPath -ExtraKeys @{disabled = $false; alwaysAllow = @()}
            Register-McpServer -Path $RooPath -BinaryPath $BinaryPath -ExtraKeys @{disabled = $false; alwaysAllow = @()}
        } catch {
            Write-Host "Failed to configure Cline/Roo Code: $_" -ForegroundColor Red
        }
    }
}

# Clean up temp dir if we cloned
if ($TempDir -ne $null -and (Test-Path $TempDir)) {
    Write-Host "`nCleaning up temporary files..." -ForegroundColor Blue
    Set-Location $OriginalDir
    Remove-Item $TempDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "`n=== SMOKE Successfully Installed! ===" -ForegroundColor Green
Write-Host "`nNext steps:"
Write-Host "  1. Open a NEW terminal window (or run 'refreshenv' in Chocolatey/Cmder)"
Write-Host "  2. Verify JS:     " -NoNewline
Write-Host "smoke test --code 'console.log(42)' --lang js" -ForegroundColor Cyan
Write-Host "  3. Verify TS:     " -NoNewline
Write-Host "smoke test --code 'const x: number = 42; console.log(x)' --lang ts" -ForegroundColor Cyan
Write-Host "  4. Verify Python: " -NoNewline
Write-Host "smoke test --code 'print(42)' --lang py" -ForegroundColor Cyan
Write-Host "`nSMOKE is now active - it will verify AI-generated code before every file write."
Write-Host "Docs: https://github.com/senapati484/smoke"
