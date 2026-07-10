#!/bin/bash
# SMOKE Installation Script (macOS and Linux)
# Compiles SMOKE from source, installs it in ~/.smoke/bin, and registers it in Claude Code & MCP clients.

set -e

# Color definitions
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # Reset

# Helper functions for portable colored output
log_blue() { printf "${BLUE}%b${NC}\n" "$1"; }
log_green() { printf "${GREEN}%b${NC}\n" "$1"; }
log_red() { printf "${RED}%b${NC}\n" "$1"; }
log_yellow() { printf "${YELLOW}%b${NC}\n" "$1"; }
log_plain() { printf "%b\n" "$1"; }

log_blue "=== Starting SMOKE Installation ==="

# 1. Verify cargo/rust installation
if ! command -v cargo &> /dev/null; then
    log_red "Error: Rust/Cargo is not installed."
    log_plain "Please install Rust (https://rustup.rs) first, restart your terminal, and run this script again."
    exit 1
fi

# Check if we are running from the smoke repository root
IN_REPO=false
if [ -f "Cargo.toml" ] && grep -q 'name = "smoke"' Cargo.toml; then
    IN_REPO=true
fi

TEMP_DIR=""
if [ "$IN_REPO" = false ]; then
    log_yellow "Not running inside smoke repository. Cloning smoke from GitHub to a temporary directory..."
    if ! command -v git &> /dev/null; then
        log_red "Error: git is not installed."
        log_plain "Please install git or run the installer script from inside the cloned repository."
        exit 1
    fi
    TEMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t 'smoke-install')
    git clone https://github.com/senapati484/smoke.git "$TEMP_DIR"
    ORIGINAL_DIR=$(pwd)
    cd "$TEMP_DIR"
fi

# 2. Build in release mode
printf "\n"
log_blue "1. Building SMOKE in release mode..."
cargo build --release

# 3. Create target directory
printf "\n"
log_blue "2. Creating target installation directory..."
INSTALL_DIR="$HOME/.smoke/bin"
mkdir -p "$INSTALL_DIR"

# 4. Copy binary
printf "\n"
log_blue "3. Copying SMOKE binary..."
cp target/release/smoke "$INSTALL_DIR/smoke"
chmod +x "$INSTALL_DIR/smoke"
log_green "Copied to $INSTALL_DIR/smoke"

# 5. Configure PATH
printf "\n"
log_blue "4. Configuring PATH environment variable..."
SHELL_CONFIG=""
# Detect active shell config
case "$SHELL" in
    */zsh)
        SHELL_CONFIG="$HOME/.zshrc"
        ;;
    */bash)
        SHELL_CONFIG="$HOME/.bashrc"
        ;;
    *)
        if [ -f "$HOME/.zshrc" ]; then
            SHELL_CONFIG="$HOME/.zshrc"
        elif [ -f "$HOME/.bashrc" ]; then
            SHELL_CONFIG="$HOME/.bashrc"
        else
            SHELL_CONFIG="$HOME/.profile"
        fi
        ;;
esac

# Append PATH export if not already present
if [ -f "$SHELL_CONFIG" ]; then
    if ! grep -q "\.smoke/bin" "$SHELL_CONFIG"; then
        printf '\n# SMOKE execution blocker\n' >> "$SHELL_CONFIG"
        printf 'export PATH="$HOME/.smoke/bin:$PATH"\n' >> "$SHELL_CONFIG"
        log_green "Appended path configuration to $SHELL_CONFIG"
    else
        log_plain "Path configuration already present in $SHELL_CONFIG"
    fi
else
    printf 'export PATH="$HOME/.smoke/bin:$PATH"\n' >> "$HOME/.profile"
    log_green "Created and added path configuration to $HOME/.profile"
fi

# 6. Determine which agents to configure
SELECTED_AGENTS=""

if [ -t 0 ]; then
    # Interactive menu
    echo -e "\n${BLUE}5. Choose AI tools to configure SMOKE for:${NC}"
    echo "  1) Claude Code (CLI Pre/Post-Tool Hooks) [Default]"
    echo "  2) Claude Desktop App (MCP Server)"
    echo "  3) Windsurf IDE (MCP Server)"
    echo "  4) Cline & Roo Code VS Code Extensions (MCP Server)"
    echo "  5) All of the above"
    echo "  6) Skip automatic registration"
    echo -n "Select options (e.g. 1,2 or 5) [1]: "
    read -r choice

    if [ -z "$choice" ]; then
        choice="1"
    fi

    case "$choice" in
        5)
            SELECTED_AGENTS="claude-code,claude-desktop,windsurf,cline"
            ;;
        6)
            SELECTED_AGENTS=""
            ;;
        *)
            # Parse comma-separated list
            IFS=',' read -ra ADDR <<< "$choice"
            for i in "${ADDR[@]}"; do
                case "$i" in
                    1) SELECTED_AGENTS="${SELECTED_AGENTS:+$SELECTED_AGENTS,}claude-code" ;;
                    2) SELECTED_AGENTS="${SELECTED_AGENTS:+$SELECTED_AGENTS,}claude-desktop" ;;
                    3) SELECTED_AGENTS="${SELECTED_AGENTS:+$SELECTED_AGENTS,}windsurf" ;;
                    4) SELECTED_AGENTS="${SELECTED_AGENTS:+$SELECTED_AGENTS,}cline" ;;
                esac
            done
            ;;
    esac
else
    # Non-interactive (e.g. curl | sh). Default to Claude Code hooks only.
    log_yellow "Non-interactive mode detected — defaulting to Claude Code hooks."
    log_plain "  Re-run the script interactively to configure other AI tools."
    SELECTED_AGENTS="claude-code"
fi

if [ -n "$SELECTED_AGENTS" ]; then
    printf "\n"
    log_blue "6. Registering SMOKE configuration..."
    PYTHON_CMD=""
    if command -v python3 &> /dev/null; then
        PYTHON_CMD="python3"
    elif command -v python &> /dev/null; then
        PYTHON_CMD="python"
    fi

    if [ -n "$PYTHON_CMD" ]; then
        $PYTHON_CMD -c '
import json
import os
import sys

binary_path = os.path.expanduser("~/.smoke/bin/smoke")

def update_claude_code():
    settings_path = os.path.expanduser("~/.claude/settings.json")
    if os.path.exists(settings_path):
        try:
            with open(settings_path, "r") as f:
                data = json.load(f)
        except Exception as e:
            print(f"Error reading Claude settings file: {e}", file=sys.stderr)
            data = {}
    else:
        data = {}

    if "hooks" not in data:
        data["hooks"] = {}
    hooks = data["hooks"]

    if "PreToolUse" not in hooks or not isinstance(hooks["PreToolUse"], list):
        hooks["PreToolUse"] = []
    if "PostToolUse" not in hooks or not isinstance(hooks["PostToolUse"], list):
        hooks["PostToolUse"] = []

    pre_hook_payload = {
        "type": "command",
        "command": f"{binary_path} hook",
        "timeout": 10,
        "statusMessage": "SMOKE: verifying code..."
    }

    post_hook_payload = {
        "type": "command",
        "command": f"{binary_path} post-hook",
        "timeout": 30
    }

    # Update PreToolUse
    pre_entry = None
    for item in hooks["PreToolUse"]:
        if item.get("matcher") == "Write|Edit":
            pre_entry = item
            break
    if not pre_entry:
        pre_entry = {"matcher": "Write|Edit", "hooks": []}
        hooks["PreToolUse"].append(pre_entry)
    pre_entry["hooks"] = [h for h in pre_entry["hooks"] if "smoke hook" not in h.get("command", "")]
    pre_entry["hooks"].append(pre_hook_payload)

    # Update PostToolUse
    post_entry = None
    for item in hooks["PostToolUse"]:
        if item.get("matcher") == "Write|Edit":
            post_entry = item
            break
    if not post_entry:
        post_entry = {"matcher": "Write|Edit", "hooks": []}
        hooks["PostToolUse"].append(post_entry)
    post_entry["hooks"] = [h for h in post_entry["hooks"] if "smoke post-hook" not in h.get("command", "")]
    post_entry["hooks"].append(post_hook_payload)

    os.makedirs(os.path.dirname(settings_path), exist_ok=True)
    with open(settings_path, "w") as f:
        json.dump(data, f, indent=2)
    print("Registered SMOKE in Claude Code hooks successfully.")

def update_mcp_config(config_path, extra_keys=None):
    path = os.path.expanduser(config_path)
    if os.path.exists(path):
        try:
            with open(path, "r") as f:
                data = json.load(f)
        except Exception:
            data = {}
    else:
        data = {}

    if "mcpServers" not in data or not isinstance(data["mcpServers"], dict):
        data["mcpServers"] = {}

    server_config = {
        "command": binary_path,
        "args": ["server"]
    }
    if extra_keys:
        server_config.update(extra_keys)

    data["mcpServers"]["smoke"] = server_config

    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        json.dump(data, f, indent=2)
    print(f"Registered SMOKE MCP server in {path}")

# Run selected updates
selected = sys.argv[1].split(",")

if "claude-code" in selected:
    try:
        update_claude_code()
    except Exception as e:
        print(f"Failed to configure Claude Code: {e}", file=sys.stderr)

if "claude-desktop" in selected:
    try:
        if sys.platform == "darwin":
            p = "~/Library/Application Support/Claude/claude_desktop_config.json"
        else:
            p = "~/.config/Claude/claude_desktop_config.json"
        update_mcp_config(p)
    except Exception as e:
        print(f"Failed to configure Claude Desktop: {e}", file=sys.stderr)

if "windsurf" in selected:
    try:
        update_mcp_config("~/.codeium/windsurf/mcp_config.json")
    except Exception as e:
        print(f"Failed to configure Windsurf: {e}", file=sys.stderr)

if "cline" in selected:
    try:
        # VS Code path for macOS
        vsc_paths = [
            "~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json" if sys.platform == "darwin" else "~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json",
            "~/Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/cline_mcp_settings.json" if sys.platform == "darwin" else "~/.config/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/cline_mcp_settings.json"
        ]
        for p in vsc_paths:
            # We only write if VS Code or the extension config dir exists to avoid spamming empty directories
            # but for this installer we will create the parent directory if the user selected it explicitly
            update_mcp_config(p, extra_keys={"disabled": False, "alwaysAllow": []})
    except Exception as e:
        print(f"Failed to configure Cline/Roo Code: {e}", file=sys.stderr)
' "$SELECTED_AGENTS"
    else
        log_red "Warning: Python is not installed. Skipping automatic hook registration."
    fi
fi

# Clean up temp dir if we cloned
if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
    printf "\n"
    log_blue "Cleaning up temporary files..."
    cd "$ORIGINAL_DIR"
    rm -rf "$TEMP_DIR"
fi

printf "\n"
log_green "=== SMOKE Successfully Installed! ==="
printf "\nNext steps:\n"
printf "  1. Reload your shell:  ${BLUE}source %s${NC}\n" "$SHELL_CONFIG"
printf "  2. Verify install:     ${BLUE}smoke test --code 'console.log(42)' --lang js${NC}\n"
printf "\nSMOKE is now active — it will verify AI-generated code before every file write.\n"
printf "Docs: https://github.com/senapati484/smoke\n"
