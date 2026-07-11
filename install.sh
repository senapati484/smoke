#!/bin/bash
# SMOKE Installation Script (macOS / Linux)
# Builds SMOKE from source, installs it to ~/.smoke/bin, then delegates
# all AI tool registration to `smoke install` — no Python or Node required.

set -e

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

log_blue()   { printf "${BLUE}%b${NC}\n" "$1"; }
log_green()  { printf "${GREEN}%b${NC}\n" "$1"; }
log_red()    { printf "${RED}%b${NC}\n" "$1"; }
log_yellow() { printf "${YELLOW}%b${NC}\n" "$1"; }
log_plain()  { printf "%b\n" "$1"; }

printf "${BLUE}╔══════════════════════════════════════╗${NC}\n"
printf "${BLUE}║        SMOKE Installer                ║${NC}\n"
printf "${BLUE}╚══════════════════════════════════════╝${NC}\n\n"

# ── Step 1: Check Rust / Cargo ────────────────────────────────────────────────
log_blue "1. Checking prerequisites..."
if ! command -v cargo &>/dev/null; then
    log_red "Error: Rust/Cargo is not installed."
    log_plain "Install Rust from https://rustup.rs, restart your terminal, and try again."
    exit 1
fi
log_green "   Cargo $(cargo --version) found."

# ── Step 2: Clone repo if not running inside it ───────────────────────────────
TEMP_DIR=""
ORIGINAL_DIR="$(pwd)"
IN_REPO=false

if [ -f "Cargo.toml" ] && grep -q 'name = "smoke"' Cargo.toml 2>/dev/null; then
    IN_REPO=true
fi

if [ "$IN_REPO" = false ]; then
    log_blue "\n2. Cloning SMOKE from GitHub..."
    if ! command -v git &>/dev/null; then
        log_red "Error: git is not installed."
        log_plain "Install git or run this script from inside the cloned smoke repository."
        exit 1
    fi
    TEMP_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t 'smoke-install')
    git clone https://github.com/senapati484/smoke.git "$TEMP_DIR"
    cd "$TEMP_DIR"
else
    log_green "   Running inside smoke repository — skipping clone."
fi

# ── Step 3: Build ─────────────────────────────────────────────────────────────
printf "\n"
log_blue "3. Building SMOKE in release mode (this takes ~2 min on first run)..."
cargo build --release

# ── Step 4: Install binary ────────────────────────────────────────────────────
printf "\n"
log_blue "4. Installing binary to ~/.smoke/bin..."
INSTALL_DIR="$HOME/.smoke/bin"
mkdir -p "$INSTALL_DIR"
cp target/release/smoke "$INSTALL_DIR/smoke"
chmod +x "$INSTALL_DIR/smoke"

# macOS: codesign to prevent AMFI/SIGKILL
if [ "$(uname)" = "Darwin" ] && command -v codesign &>/dev/null; then
    codesign --force --sign - "$INSTALL_DIR/smoke" 2>/dev/null || true
fi
log_green "   Installed to $INSTALL_DIR/smoke"

# ── Step 5: Add to PATH ───────────────────────────────────────────────────────
printf "\n"
log_blue "5. Configuring PATH..."
SHELL_CONFIG=""
case "$SHELL" in
    */zsh)  SHELL_CONFIG="$HOME/.zshrc" ;;
    */bash) SHELL_CONFIG="$HOME/.bashrc" ;;
    *)
        if   [ -f "$HOME/.zshrc" ];   then SHELL_CONFIG="$HOME/.zshrc"
        elif [ -f "$HOME/.bashrc" ];  then SHELL_CONFIG="$HOME/.bashrc"
        else SHELL_CONFIG="$HOME/.profile"; fi ;;
esac

if [ -f "$SHELL_CONFIG" ] && grep -q '\.smoke/bin' "$SHELL_CONFIG"; then
    log_plain "   PATH already configured in $SHELL_CONFIG"
else
    printf '\n# SMOKE\nexport PATH="$HOME/.smoke/bin:$PATH"\n' >> "$SHELL_CONFIG"
    log_green "   Added PATH to $SHELL_CONFIG"
fi

# Make binary available in the current shell session immediately
export PATH="$HOME/.smoke/bin:$PATH"

# ── Step 6: Choose tools to register ─────────────────────────────────────────
printf "\n"
SELECTED_AGENTS="all"

if [ -t 0 ]; then
    log_blue "6. Which AI tools should SMOKE register with?"
    printf "   ${CYAN}1)${NC} All supported tools              ${CYAN}[default]${NC}\n"
    printf "   ${CYAN}2)${NC} Claude Code only (hooks)\n"
    printf "   ${CYAN}3)${NC} Claude Desktop (MCP server)\n"
    printf "   ${CYAN}4)${NC} Windsurf (MCP server)\n"
    printf "   ${CYAN}5)${NC} Cursor (MCP server)\n"
    printf "   ${CYAN}6)${NC} Cline / Roo Code (MCP server)\n"
    printf "   ${CYAN}7)${NC} Custom — enter tool keys manually\n"
    printf "   ${CYAN}8)${NC} Skip registration\n"
    printf "\n   Enter choice [1]: "
    read -r choice
    choice="${choice:-1}"

    case "$choice" in
        1|"") SELECTED_AGENTS="all" ;;
        2)    SELECTED_AGENTS="claude-code" ;;
        3)    SELECTED_AGENTS="claude-desktop" ;;
        4)    SELECTED_AGENTS="windsurf" ;;
        5)    SELECTED_AGENTS="cursor" ;;
        6)    SELECTED_AGENTS="cline" ;;
        7)
            printf "   Enter comma-separated tools (e.g. claude-code,cursor): "
            read -r SELECTED_AGENTS
            ;;
        8)    SELECTED_AGENTS="" ;;
        *)    SELECTED_AGENTS="all" ;;
    esac
else
    # Non-interactive (e.g. curl | sh)
    log_yellow "   Non-interactive mode — registering all tools."
    SELECTED_AGENTS="all"
fi

# ── Step 7: Register ──────────────────────────────────────────────────────────
if [ -n "$SELECTED_AGENTS" ]; then
    printf "\n"
    log_blue "7. Registering SMOKE..."
    "$INSTALL_DIR/smoke" install --tools "$SELECTED_AGENTS"
else
    log_yellow "   Skipping registration. Run 'smoke install' later to configure."
fi

# ── Step 8: Cleanup ───────────────────────────────────────────────────────────
if [ -n "$TEMP_DIR" ] && [ -d "$TEMP_DIR" ]; then
    cd "$ORIGINAL_DIR"
    rm -rf "$TEMP_DIR"
fi

# ── Done ──────────────────────────────────────────────────────────────────────
printf "\n"
printf "${GREEN}╔══════════════════════════════════════╗${NC}\n"
printf "${GREEN}║     SMOKE installed successfully!     ║${NC}\n"
printf "${GREEN}╚══════════════════════════════════════╝${NC}\n\n"

printf "Next steps:\n"
printf "  1. Reload your shell:     ${CYAN}source %s${NC}\n" "$SHELL_CONFIG"
printf "  2. Check status:          ${CYAN}smoke status${NC}\n"
printf "  3. Verify JS sandbox:     ${CYAN}smoke test --code 'console.log(42)' --lang js${NC}\n"
printf "  4. Verify Python sandbox: ${CYAN}smoke test --code 'print(42)' --lang py${NC}\n"
printf "\n"
printf "Manage later:\n"
printf "  Add a tool:    ${CYAN}smoke install --tools cursor${NC}\n"
printf "  Remove a tool: ${CYAN}smoke uninstall --tools claude-desktop${NC}\n"
printf "  Uninstall all: ${CYAN}bash <(curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/uninstall.sh)${NC}\n"
printf "\n"
printf "Docs: https://github.com/senapati484/smoke\n\n"
