#!/bin/bash
# SMOKE Installation Script (macOS and Linux)
# Compiles SMOKE from source, installs it in ~/.smoke/bin, and registers it in Claude Code & MCP clients.

set -e

# Color definitions
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0;37m' # No Color

echo -e "${BLUE}=== Starting SMOKE Installation ===${NC}"

# 1. Verify cargo/rust installation
if ! command -v cargo &> /dev/null; then
    echo -e "${RED}Error: Rust/Cargo is not installed.${NC}"
    echo "Please install Rust (https://rustup.rs) first, restart your terminal, and run this script again."
    exit 1
fi

# 2. Build in release mode
echo -e "\n${BLUE}1. Building SMOKE in release mode...${NC}"
cargo build --release

# 3. Create target directory
echo -e "\n${BLUE}2. Creating target installation directory...${NC}"
INSTALL_DIR="$HOME/.smoke/bin"
mkdir -p "$INSTALL_DIR"

# 4. Copy binary
echo -e "\n${BLUE}3. Copying SMOKE binary...${NC}"
cp target/release/smoke "$INSTALL_DIR/smoke"
chmod +x "$INSTALL_DIR/smoke"
echo -e "${GREEN}Copied to $INSTALL_DIR/smoke${NC}"

# 5. Configure PATH
echo -e "\n${BLUE}4. Configuring PATH environment variable...${NC}"
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
        echo -e '\n# SMOKE execution blocker' >> "$SHELL_CONFIG"
        echo 'export PATH="$HOME/.smoke/bin:$PATH"' >> "$SHELL_CONFIG"
        echo -e "${GREEN}Appended path configuration to $SHELL_CONFIG${NC}"
    else
        echo -e "Path configuration already present in $SHELL_CONFIG"
    fi
else
    echo 'export PATH="$HOME/.smoke/bin:$PATH"' >> "$HOME/.profile"
    echo -e "${GREEN}Created and added path configuration to $HOME/.profile${NC}"
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
    # Non-interactive, default to Claude Code
    SELECTED_AGENTS="claude-code"
fi

if [ -n "$SELECTED_AGENTS" ]; then
    echo -e "\n${BLUE}6. Registering SMOKE configuration...${NC}"
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
        echo -e "${RED}Warning: Python is not installed. Skipping automatic hook registration.${NC}"
    fi
else
    echo -e "\nSkipping automatic agent registration."
fi

echo -e "\n${GREEN}=== SMOKE Successfully Installed! ===${NC}"
echo -e "Please run the following command to reload your shell profile:"
echo -e "  ${BLUE}source $SHELL_CONFIG${NC}"
echo -e "SMOKE is now active and will verify your code before your AI agents write files."
