# Getting Started with SMOKE

**Write. Run. Know.** — SMOKE is a PreToolUse hook for Claude Code that runs
AI-generated JavaScript, TypeScript, and Python code in a sandbox *before* the
agent's file write is allowed to complete. Bugs are caught the instant they're
introduced.

This tutorial walks you from zero to a fully working SMOKE installation.

---

## 1. Quick Install

### One-liner (macOS / Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/install.sh | sh
```

Builds SMOKE from source (Rust required), installs to `~/.smoke/bin/smoke`,
configures PATH, and optionally registers hooks for Claude Code, Claude Desktop,
Windsurf, and Cline/Roo Code via an interactive menu.

### PowerShell (Windows)

```powershell
iex ((New-Object System.Net.WebClient).DownloadString('https://raw.githubusercontent.com/senapati484/smoke/main/install.ps1'))
```

Same flow for Windows — builds from source, installs to `~\.smoke\bin\smoke.exe`,
configures User PATH, and registers hooks interactively.

### From source

```bash
git clone https://github.com/senapati484/smoke.git
cd smoke
cargo build --release
```

The binary lands at `./target/release/smoke`. Use `cargo build` (debug) during
development for faster iteration; use `--release` for production hook use.

### Prerequisites

| Tool | Minimum Version | Check Command |
|------|----------------|---------------|
| **Rust toolchain** (rustc + cargo) | 1.70+ | `rustc --version && cargo --version` |
| **Python 3** (for Python sandbox) | 3.8+ | `python3 --version` |

If Rust is missing, install it first:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

> **Tip:** Use `cargo build` (debug) during development for faster iteration.
> Use `cargo build --release` for production use as a hook or MCP server.

---

## 3. Verify the Build

Run the help command to confirm the binary works and see all available
subcommands:

```bash
./target/release/smoke --help
```

You should see output similar to:

```
Write. Run. Know. — A PreToolUse hook that executes AI-generated code
before writes are allowed.

Usage: smoke [OPTIONS] <COMMAND>

Commands:
  hook       PreToolUse hook entry point — reads Claude Code's hook JSON from stdin
  post-hook  PostToolUse hook entry point — auto-runs test files after a successful write
  server     MCP server entry point — exposes smoke_verify as an MCP tool over stdio
  test       Standalone CLI for development and debugging
  config     Configuration management

Options:
  --config <PATH>  Path to a custom config file
  -h, --help       Print help
  -V, --version    Print version
```

---

## 4. Quick Sandbox Test

Use the `test` subcommand to run a snippet of code in the sandbox without
involving Claude Code. This is the fastest way to verify the sandbox works.

### JavaScript

```bash
./target/release/smoke test \
  --code "console.log('hello')" \
  --lang js
```

Expected output:

```json
{
  "passed": true,
  "stdout": "hello\n",
  "stderr": "",
  "execution_time_ms": 12,
  "language": "javascript"
}
```

### TypeScript

```bash
./target/release/smoke test \
  --code "const greet = (name: string): string => \`Hello, \${name}!\`; console.log(greet('world'));" \
  --lang ts
```

Expected output:

```json
{
  "passed": true,
  "stdout": "Hello, world!\n",
  "stderr": "",
  "execution_time_ms": 15,
  "language": "typescript"
}
```

### Python

```bash
./target/release/smoke test \
  --code "print('hello from python')" \
  --lang py
```

Expected output:

```json
{
  "passed": true,
  "stdout": "hello from python\n",
  "stderr": "",
  "execution_time_ms": 45,
  "language": "python"
}
```

### Runtime error (blocked)

SMOKE catches runtime errors and returns them in the response:

```bash
./target/release/smoke test \
  --code "throw new Error('boom')" \
  --lang js
```

Expected output shows `"passed": false` with the error in `stderr`.

---

## 5. Register as a PreToolUse Hook

The primary integration path is registering SMOKE as a **PreToolUse** hook in
Claude Code's settings. This tells Claude to invoke SMOKE every time it
attempts a `Write` or `Edit` tool call.

Open `.claude/settings.json` in your project root (create it if it doesn't
exist) and add:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "./target/release/smoke hook",
            "timeout": 10,
            "statusMessage": "SMOKE: verifying code..."
          }
        ]
      }
    ]
  }
}
```

| Field | Purpose |
|-------|---------|
| `matcher` | Regex matching the tool name — `Write` and `Edit` are Claude's file-writing tools |
| `command` | Path to the SMOKE binary with the `hook` subcommand |
| `timeout` | Maximum seconds the hook may run (10s is generous) |
| `statusMessage` | Shown in the Claude Code UI while the hook runs |

> **Alternative registration method:** Some setups prefer a shell script path
> instead of a direct binary path. A convenience wrapper is provided at
> `hooks/pre-tool-use.sh` — point `command` at that script instead if needed.

---

## 6. Test the Hook with Claude Code

Start a Claude Code session in the project directory:

```bash
claude
```

Then ask Claude to generate code in any supported language:

```
Write a JavaScript function that takes an array of numbers and returns the sum
```

When Claude calls `Write` or `Edit`, you'll see the status message
`"SMOKE: verifying code..."` appear. Behind the scenes:

1. SMOKE receives the hook JSON from Claude Code via stdin
2. It detects the language from the file extension
3. tree-sitter performs a syntax check
4. The code executes in the sandbox (V8 for JS/TS, subprocess for Python)
5. If the code runs cleanly — the write proceeds
6. If there's a syntax error or runtime failure — the write is blocked and
   the error is shown to Claude so it can self-correct

Try asking Claude to write buggy code to see blocking in action:

```
Write a JavaScript function with a syntax error (missing a closing brace)
```

SMOKE blocks the write with `exit 2`, and Claude receives the syntax error
location in its conversation — it will typically fix the code on the next
attempt.

---

## 7. MCP Server Integration

SMOKE can also run as an **MCP server** over stdio. This is useful for
projects that prefer the MCP protocol or want to invoke `smoke_verify`
as an explicit tool from any MCP client.

Open `.mcp.json` in your project root and add:

```json
{
  "mcpServers": {
    "smoke": {
      "type": "stdio",
      "command": "./target/release/smoke",
      "args": ["server"]
    }
  }
}
```

After restarting Claude Code, the `smoke_verify` tool becomes available. It
accepts two arguments:

| Argument | Type | Description |
|----------|------|-------------|
| `code` | `string` | The code snippet to execute |
| `language` | `string` | One of `"javascript"`, `"typescript"`, `"python"` |

You (or Claude Code) can call it explicitly to verify arbitrary code snippets
without writing a file.

Both the hook path and the MCP path converge on the same sandbox
implementations — there is no difference in behaviour.

---

## 8. Generate a Local Config File

SMOKE uses a layered configuration system (built-in defaults → user-level →
project-level → CLI override). Generate a project-level config file with all
default values and inline documentation:

```bash
./target/release/smoke config init
```

This creates `.smoke.toml` in the current directory with contents like:

```toml
# smoke.toml — SMOKE configuration file
# Generated by: smoke config init

[limits]
# Hard timeout for sandbox execution (milliseconds)
timeout_ms = 1000

# Files with more lines than this use snippet-only execution
max_file_lines = 200

# Memory limit for Python child process (MB)
memory_limit_mb = 256

# Files larger than this (lines) are skipped entirely
max_file_lines_absolute = 1000

[languages]
js_enabled = true
ts_enabled = true
python_enabled = true

[python]
interpreter = "python3"
```

Edit this file to adjust timeouts, disable languages, or point to a different
Python interpreter.

---

## 9. View the Active Configuration

To see the currently active configuration (with all merge layers applied):

```bash
./target/release/smoke config show
```

This prints the full resolved config as TOML. Try changing a value in
`.smoke.toml` and re-running `smoke config show` to see it reflected.

---

## 10. PostToolUse — Auto-Running Tests

SMOKE includes a **PostToolUse** hook that automatically discovers and runs
co-located test files after a successful `Write` or `Edit`. This is
complementary to the PreToolUse check — it verifies correctness rather than
syntax.

Add it to your `.claude/settings.json` alongside the PreToolUse hook:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit",
        "hooks": [
          {
            "type": "command",
            "command": "./target/release/smoke hook",
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
            "command": "./target/release/smoke post-hook",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

The post-hook looks for test files using language-specific naming conventions:

| Source File | Test File Candidates |
|-------------|---------------------|
| `util.js` | `util.test.js`, `util.spec.js` |
| `util.ts` | `util.test.ts`, `__tests__/util.ts` |
| `util.py` | `tests/test_util.py`, `test_util.py` |

If a test is found, it runs in the sandbox with a 30-second timeout. A passing
test exits silently (`exit 0`). A failing test surfaces the test output to
Claude Code (`exit 2`).

---

## Recap

| Step | What You Did | Key File |
|------|-------------|----------|
| 1 | Installed prerequisites | — |
| 2 | Built SMOKE | `target/release/smoke` |
| 3 | Verified the CLI | `smoke --help` |
| 4 | Ran sandbox tests | `smoke test --code ... --lang ...` |
| 5 | Registered PreToolUse hook | `.claude/settings.json` |
| 6 | Tested with Claude Code | — |
| 7 | Registered MCP server | `.mcp.json` |
| 8 | Generated config | `.smoke.toml` |
| 9 | Viewed config | `smoke config show` |
| 10 | Registered PostToolUse hook | `.claude/settings.json` |

You now have a fully working SMOKE setup. Every time Claude Code writes or
edits a code file, SMOKE will verify it in a sandbox before the file touches
disk — and run co-located tests after the write succeeds.

For more details, see the [ARCHITECTURE.md](ARCHITECTURE.md) and
[CONFIGURATION.md](CONFIGURATION.md) documents.
