# Integrating SMOKE with Terminal & Editor AIs

SMOKE serves as a unified gatekeeper between AI assistants writing code and your filesystem. It intercepts writes or provides validation tools to catch syntax and runtime bugs immediately.

Depending on the AI client you use, SMOKE integrates in one of three ways:

1. **Implicit Hooks (Claude Code)**: Intercepts `Write` and `Edit` tool calls automatically to verify code before it touches disk.
2. **Explicit MCP Tools (Cursor, Windsurf, Cline, Roo Code, Continue)**: Exposes a `smoke_verify` tool that agents can invoke to run code in V8 or seccomp sandboxes.
3. **Universal Git Hooks (Aider, CLI editors)**: Validates staged files during `git commit`, aborting the commit if buggy code is introduced, prompting the AI to self-correct.

---

## Support Matrix

| AI Assistant | Integration Method | Configuration Path | Loop Detection | Key Features |
|---|---|---|---|---|
| **Claude Code** | Native CLI Hooks | `.claude/settings.json` | Yes (Session-scoped) | Intercepts writes, Test auto-run, Anti-deletion guard |
| **Cursor** | MCP Server | Cursor Settings (GUI) | Client-managed | Sandbox-execute JS/TS/Python/Rust snippets |
| **Windsurf** | MCP Server | `~/.codeium/windsurf/mcp_config.json` | Client-managed | Fully-integrated IDE verification |
| **Cline / Roo Code** | MCP Server | `cline_mcp_settings.json` | Client-managed | Realtime tool verification |
| **Continue** | MCP Server | `~/.continue/config.json` | Client-managed | Inline and sidebar code verification |
| **Aider** | Git Pre-commit Hook | `.git/hooks/pre-commit` | Git-abort triggered | Blocks bad commits; Aider auto-fixes commit failures |

---

## 1. Claude Code Integration (Hooks)

Claude Code supports native hook commands in `.claude/settings.json`. SMOKE registers both `PreToolUse` (blocking/warning) and `PostToolUse` (testing) hooks.

### Configuration
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "smoke hook",
            "timeout": 10,
            "statusMessage": "SMOKE: verifying code..."
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "smoke post-hook",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

### How it works:
- **PreToolUse**: When Claude attempts to write or edit a file, the code is syntax-checked and sandbox-executed. If validation fails, SMOKE blocks the write (exiting with code 2) and returns the stderr to Claude.
- **PostToolUse**: Once a file is written successfully, SMOKE searches for co-located test files (e.g., `math.ts` -> `math.test.ts`) and runs them. If tests fail, it reports the failure to Claude so it can fix the code.
- **Loop Detection**: SMOKE tracks repeated errors by fingerprint. If the agent gets stuck in a loop retrying the same incorrect fix, SMOKE escalates by replacing the error with a strategy-change prompt (e.g., forcing it to re-state its hypothesis or ask the user for help).

---

## 2. Model Context Protocol (MCP) Integration

For editor-based AIs, SMOKE runs as an MCP server. It exposes a single tool: **`smoke_verify`**.

### Tool Schema
- **Name**: `smoke_verify`
- **Arguments**:
  - `code` (string): The code content to execute.
  - `language` (string): `"javascript"`, `"typescript"`, `"python"`, or `"rust"`.
- **Return**: `{ passed: boolean, stdout: string, stderr: string, execution_time_ms: u64, language: string }`

### Cursor Setup
1. Open Cursor Settings -> **Features** -> **MCP**.
2. Click **+ Add New MCP Server**.
3. Configure:
   - **Name**: `smoke`
   - **Type**: `stdio`
   - **Command**: `smoke server` (or the absolute path `/Users/YOUR_USER/.smoke/bin/smoke server`)
4. Click **Save**.

### Windsurf Setup
Add SMOKE to your global Windsurf MCP configuration in `~/.codeium/windsurf/mcp_config.json`:
```json
{
  "mcpServers": {
    "smoke": {
      "command": "smoke",
      "args": ["server"]
    }
  }
}
```

### Cline / Roo Code Setup
Add SMOKE to the Cline/Roo Code configuration at:
`~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json` (macOS)
`%APPDATA%\Code\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json` (Windows)
```json
{
  "mcpServers": {
    "smoke": {
      "command": "smoke",
      "args": ["server"],
      "disabled": false
    }
  }
}
```

### Continue Setup
Add SMOKE to your Continue config in `~/.continue/config.json`:
```json
{
  "mcpServers": [
    {
      "name": "smoke",
      "command": "smoke",
      "args": ["server"]
    }
  ]
}
```

---

## 3. Git Hooks Integration (Aider & Command Line AIs)

Aider and similar terminal-based AIs commit code to git automatically as they work. You can integrate SMOKE directly into git's commit pipeline using a **`pre-commit` hook**.

When Aider attempts to commit its changes, the hook will run SMOKE on all staged files. If syntax or runtime checks fail, the commit is aborted. Aider detects the commit failure, reads the stderr error output, and immediately attempts to fix the code.

### Git Hook Setup

Create or edit `.git/hooks/pre-commit` in your project repository:

```bash
#!/bin/bash
# .git/hooks/pre-commit
# Intercepts git commits and runs SMOKE verification on all staged files.

# Find all staged files matching supported extensions
staged_files=$(git diff --cached --name-only --diff-filter=ACM | grep -E '\.(js|ts|py|rs)$')

if [ -z "$staged_files" ]; then
    exit 0
fi

echo -e "\x1b[34m[SMOKE] Verifying staged files...\x1b[0m"

for file in $staged_files; do
    # Read the staged content of the file
    content=$(git show :"$file")
    
    # Determine language flag
    ext="${file##*.}"
    case "$ext" in
        js) lang="js" ;;
        ts) lang="ts" ;;
        py) lang="py" ;;
        rs) lang="rs" ;;
        *) continue ;;
    esac
    
    echo -e "  Checking \x1b[36m$file\x1b[0m ($lang)..."
    
    # Execute SMOKE check using the CLI test command
    # If the check fails, exit with the error
    result=$(smoke test --code "$content" --lang "$lang")
    passed=$(echo "$result" | grep -o '"passed":\s*true')
    
    if [ -z "$passed" ]; then
        stderr_output=$(echo "$result" | sed -n 's/.*"stderr":\s*"\([^"]*\)".*/\1/p')
        echo -e "\x1b[31m[SMOKE] Verification failed for $file:\x1b[0m"
        echo -e "  \x1b[33m$stderr_output\x1b[0m"
        echo -e "\x1b[31mCommit aborted. Please fix the errors above.\x1b[0m"
        exit 1
    fi
done

echo -e "\x1b[32m[SMOKE] All staged files verified successfully ✓\x1b[0m"
exit 0
```

### Make the hook executable:
```bash
chmod +x .git/hooks/pre-commit
```

### Workflow with Aider:
1. You ask Aider: *"Refactor the file-loading logic in loader.py"*
2. Aider writes the edits and attempts to commit the change: `git commit -m "Refactor file-loading logic"`
3. The git `pre-commit` hook triggers.
4. SMOKE verifies `loader.py`. If it finds a `SyntaxError` or `NameError`, it prints the error and exits with code 1.
5. The commit is aborted. Aider receives the abort notification along with the traceback error, and says: *"It seems the commit failed due to a NameError. Let me correct that by importing the module..."*
6. Aider applies the fix, successfully commits, and completes the task cleanly.
