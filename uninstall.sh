#!/bin/bash
# SMOKE Uninstaller (macOS / Linux)
# Removes SMOKE hooks/MCP entries from all AI tool configs, deletes the
# binary, and cleans up PATH entries from your shell config.
#
# Run:
#   bash <(curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/uninstall.sh)
# or inside the repo:
#   bash uninstall.sh

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

printf "${BLUE}╔══════════════════════════════════════╗${NC}\n"
printf "${BLUE}║       SMOKE Uninstaller              ║${NC}\n"
printf "${BLUE}╚══════════════════════════════════════╝${NC}\n\n"

BINARY="$HOME/.smoke/bin/smoke"
INSTALL_DIR="$HOME/.smoke"

# ── Step 1: Remove tool registrations ────────────────────────────────────────
printf "${BLUE}1. Removing SMOKE from all AI tool configurations...${NC}\n"

if [ -x "$BINARY" ]; then
    "$BINARY" uninstall --tools all
else
    printf "${YELLOW}   Binary not found at $BINARY — skipping config cleanup.${NC}\n"
    printf "${YELLOW}   You may need to manually remove SMOKE entries from:${NC}\n"
    printf "     • ~/.claude/settings.json                     (Claude Code)\n"
    printf "     • ~/Library/Application Support/Claude/claude_desktop_config.json\n"
    printf "     • ~/.codeium/windsurf/mcp_config.json\n"
    printf "     • ~/.cursor/mcp.json\n"
fi

# ── Step 2: Remove binary and data directory ──────────────────────────────────
printf "\n${BLUE}2. Removing SMOKE binary and data directory...${NC}\n"

if [ -d "$INSTALL_DIR" ]; then
    rm -rf "$INSTALL_DIR"
    printf "${GREEN}   Removed $INSTALL_DIR${NC}\n"
else
    printf "${YELLOW}   $INSTALL_DIR not found — already removed.${NC}\n"
fi

# ── Step 3: Remove PATH entry from shell config ───────────────────────────────
printf "\n${BLUE}3. Cleaning up PATH from shell config...${NC}\n"

CLEANED=false
for CONFIG in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.profile" "$HOME/.bash_profile"; do
    if [ -f "$CONFIG" ] && grep -q '\.smoke/bin' "$CONFIG"; then
        # Remove lines that reference .smoke/bin (the PATH export and its comment)
        TMPFILE="$(mktemp)"
        grep -v '\.smoke/bin' "$CONFIG" | grep -v '# SMOKE' > "$TMPFILE" || true
        mv "$TMPFILE" "$CONFIG"
        printf "${GREEN}   Cleaned $CONFIG${NC}\n"
        CLEANED=true
    fi
done

if [ "$CLEANED" = false ]; then
    printf "${YELLOW}   No PATH entries found in shell configs.${NC}\n"
fi

# ── Done ──────────────────────────────────────────────────────────────────────
printf "\n"
printf "${GREEN}╔══════════════════════════════════════╗${NC}\n"
printf "${GREEN}║     SMOKE uninstalled successfully!  ║${NC}\n"
printf "${GREEN}╚══════════════════════════════════════╝${NC}\n\n"

printf "SMOKE has been fully removed.\n"
printf "Reload your shell to apply PATH changes:  ${CYAN}exec \$SHELL${NC}\n\n"
printf "To reinstall later:\n"
printf "  ${CYAN}bash <(curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/install.sh)${NC}\n\n"
