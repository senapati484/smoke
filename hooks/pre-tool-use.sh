#!/bin/sh
# hooks/pre-tool-use.sh
# Shell wrapper for projects that need a script path instead of a binary path.
# For most projects, use .claude/settings.json with "command": "./target/release/smoke hook"
exec "$(dirname "$0")/../target/release/smoke" hook
