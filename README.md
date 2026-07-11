<p align="center">
  <img width="250" height="250" src="./public/smoke.png" alt="SMOKE logo">
</p>

<h1 align="center">SMOKE</h1>

<p align="center"><em>Write. Run. Know.</em></p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.81+-111111?style=flat-square" alt="Rust 1.81+">
  <img src="https://img.shields.io/badge/license-Apache%202.0-111111?style=flat-square" alt="Apache 2.0">
  <img src="https://img.shields.io/badge/status-early%20development-111111?style=flat-square" alt="Status">
  <img src="https://img.shields.io/badge/sandbox-V8%20%7C%20seccomp-111111?style=flat-square" alt="Sandbox">
</p>

---

SMOKE sits **between the agent deciding to write code and the file actually being written**. Every `.js`, `.ts`, `.tsx`, `.py`, and `.rs` file is:

1. Syntax-checked by tree-sitter (< 1 ms)
2. Validated in an isolated environment — V8 for JS/TS, subprocess + seccomp for Python, cargo check or rustc for Rust
3. Optionally followed by auto-running co-located test files

The agent finds out about bugs the same second it introduces them — not three tool calls later when you notice the app is broken.

Works as a **Claude Code hook** (PreToolUse / PostToolUse), an **MCP server** for Claude Desktop, Windsurf, Cline, Roo Code, and as a standalone CLI.

---

## Prerequisites

SMOKE builds from source. You need Rust installed:

```bash
# macOS / Linux — install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env

# Windows — download and run rustup-init.exe from https://rustup.rs
```

Python (any version ≥ 3.6) must be on your `PATH` for Python sandboxing to work. It is not required for JS/TS.

---

## Installation

### macOS / Linux — one-liner

```bash
curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/install.sh | sh
```

> **Note:** When piped through `curl | sh`, the script runs non-interactively and defaults to configuring **Claude Code hooks only**. To configure other AI tools (Claude Desktop, Windsurf, Cline, Roo Code), clone the repo and run the script directly:

```bash
git clone https://github.com/senapati484/smoke.git
cd smoke
chmod +x install.sh
./install.sh
```

### Windows — PowerShell

```powershell
# Run as regular user (no Admin needed)
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
iex ((New-Object System.Net.WebClient).DownloadString('https://raw.githubusercontent.com/senapati484/smoke/main/install.ps1'))
```

Or for full interactive setup:

```powershell
git clone https://github.com/senapati484/smoke.git
cd smoke
.\install.ps1
```

### What the installer does

Both installers perform the same steps:

| Step | Action |
|------|--------|
| 1 | Checks that `cargo` is on your PATH |
| 2 | Runs `cargo build --release` |
| 3 | Copies binary to `~/.smoke/bin/smoke` (or `~\.smoke\bin\smoke.exe` on Windows) |
| 4 | Adds `~/.smoke/bin` to your shell's `PATH` (`.zshrc` / `.bashrc` / User PATH registry on Windows) |
| 5 | Interactively asks which AI tools to register (see below) |

### AI tool registration — interactive menu

When run interactively, the installer presents:

```
5. Choose AI tools to configure SMOKE for:
  1) Claude Code (CLI Pre/Post-Tool Hooks)  [Default]
  2) Claude Desktop App (MCP Server)
  3) Windsurf IDE (MCP Server)
  4) Cline & Roo Code VS Code Extensions (MCP Server)
  5) All of the above
  6) Skip automatic registration
Select options (e.g. 1,2 or 5) [1]:
```

You can select a single option, a comma-separated list (e.g. `1,3`), or `5` to configure everything at once.

### Post-install verification

After the installer finishes, reload your shell and verify:

```bash
# macOS / Linux
source ~/.zshrc         # or ~/.bashrc depending on your shell

# Verify the sandbox
smoke test --code 'console.log("hello")' --lang js
smoke test --code 'print("hello")' --lang py
```

If `smoke` is not found after reloading, check that `~/.smoke/bin` is in your `PATH`:

```bash
echo $PATH | grep smoke
```

---

## Manual registration

If you prefer to register SMOKE yourself (or use it in a CI environment), use the JSON snippets below.

### Claude Code — hooks (`~/.claude/settings.json`)

```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Write|Edit",
      "hooks": [{
        "type": "command",
        "command": "smoke hook",
        "timeout": 10,
        "statusMessage": "SMOKE: verifying code..."
      }]
    }],
    "PostToolUse": [{
      "matcher": "Write|Edit",
      "hooks": [{
        "type": "command",
        "command": "smoke post-hook",
        "timeout": 30
      }]
    }]
  }
}
```

If `smoke` is not on your `PATH`, use the absolute path: `"/home/you/.smoke/bin/smoke hook"`.

### Claude Desktop (`claude_desktop_config.json`)

| Platform | Config file location |
|----------|---------------------|
| macOS | `~/Library/Application Support/Claude/claude_desktop_config.json` |
| Linux | `~/.config/Claude/claude_desktop_config.json` |
| Windows | `%APPDATA%\Claude\claude_desktop_config.json` |

```json
{
  "mcpServers": {
    "smoke": {
      "command": "/home/you/.smoke/bin/smoke",
      "args": ["server"]
    }
  }
}
```

### Windsurf IDE

Config file: `~/.codeium/windsurf/mcp_config.json` (macOS/Linux) or `%USERPROFILE%\.codeium\windsurf\mcp_config.json` (Windows)

```json
{
  "mcpServers": {
    "smoke": {
      "command": "/home/you/.smoke/bin/smoke",
      "args": ["server"]
    }
  }
}
```

### Cline (VS Code Extension)

Config file (macOS): `~/Library/Application Support/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`

Config file (Linux): `~/.config/Code/User/globalStorage/saoudrizwan.claude-dev/settings/cline_mcp_settings.json`

Config file (Windows): `%APPDATA%\Code\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json`

```json
{
  "mcpServers": {
    "smoke": {
      "command": "/home/you/.smoke/bin/smoke",
      "args": ["server"],
      "disabled": false,
      "alwaysAllow": []
    }
  }
}
```

### Roo Code (VS Code Extension)

Same structure as Cline, but the config file path uses `rooveterinaryinc.roo-cline` instead of `saoudrizwan.claude-dev`.

### Any other MCP client (`.mcp.json`)

```json
{
  "mcpServers": {
    "smoke": {
      "type": "stdio",
      "command": "smoke",
      "args": ["server"]
    }
  }
}
```

---

## How it works

```
Write/Edit tool call
      │
      ▼
  ┌─────────────────────┐
  │ 1. tree-sitter      │  Syntax pre-check (< 1 ms, instant)
  │    checks syntax    │
  └─────┬───────────────┘
        │ fail → block (exit 2) + exact line/col error
        │ pass
        ▼
  ┌─────────────────────┐
  │ 2. Extract snippet  │  Only for large files (> 200 lines)
  │    (optional)       │  Runs just the enclosing function/class
  └─────┬───────────────┘
        │
        ▼
  ┌─────────────────────┐
  │ 3. Run in sandbox   │  JS/TS: V8 (deno_core), no fs/net
  │                     │  Python: subprocess + rlimit + seccomp
  └─────┬───────────────┘
        │ fail → block (exit 2) + stdout/stderr
        │ pass
        ▼
  ┌─────────────────────┐
  │ 4. PostToolUse      │  Discovers & runs co-located tests
  │    auto-tests       │  *.test.*, *.spec.*, tests/test_*
  └─────┬───────────────┘
        │ fail → block + test output
        │ pass
        ▼
     Write completes ✓
```

Three integration modes — one sandbox core:

| Mode | Command | Primary use case |
|------|---------|-----------------|
| **Hook** | `smoke hook` | PreToolUse — blocks or warns about bad code before it reaches disk |
| **Post-hook** | `smoke post-hook` | PostToolUse — runs co-located tests after a successful write |
| **MCP server** | `smoke server` | `smoke_verify` tool from any MCP client |

---

## Hook Modes & Loop Detection

### Hook Modes
When integrated with Claude Code via hooks, SMOKE operates in one of three modes, defined via `mode` in the `[hook]` section of your configuration:

1. **`"advisor"` (Default)**: SMOKE never blocks writes (`exit 0`). Syntax/runtime errors are printed to the terminal in yellow and returned to Claude's context via `additionalContext` so the agent is aware and can self-correct.
2. **`"strict"`**: SMOKE blocks tool execution with exit code `2` only when syntax or sandbox run errors are found in a *standalone runnable script* (contains `fn main` in Rust, or does not contain `import`/`export`/`require` module syntax in JS/TS). For non-runnable module files (like React components), it falls back to warnings.
3. **`"silent"`**: Verification is skipped entirely and all tool calls are silently allowed.

### Loop & Repeated-Failure Detection
AI agents often get stuck in repetitive edit-retry loops (retrying variants of the same broken fix). SMOKE solves this using a file-backed, session-scoped memory:

- On a hook failure (syntax or execution error), SMOKE normalizes the error output (stripping line/column numbers, whitespaces, and quotes) to compute a stable **error fingerprint**.
- It records the error fingerprint scoped to the Claude Code `session_id` in `~/.smoke/state/<session_id>.json`.
- If the agent repeats the same error signature on a file:
  - **Attempt 2**: A warning notice is prepended reminding the agent of consecutive identical failures.
  - **Attempt 3+**: An escalation is triggered injecting a forced strategy-change prompt. The prompt forces the agent to stop editing, re-read the error in full, state its hypothesis, or ask the user for guidance.
- Once the sandbox checks pass successfully, the failure records for the target file are automatically cleared.

---

## CLI commands

| Command | Description |
|---------|-------------|
| `smoke test --code '...' --lang js` | Run a snippet directly in the sandbox |
| `smoke test --code '...' --lang ts` | TypeScript variant |
| `smoke test --code '...' --lang py` | Python variant |
| `smoke hook` | PreToolUse handler (reads Claude's hook JSON from stdin) |
| `smoke post-hook` | PostToolUse handler (discovers and runs co-located tests) |
| `smoke server` | MCP server over stdio |
| `smoke config init` | Write a `.smoke.toml` with defaults to the current directory |
| `smoke config show` | Print the currently active configuration |

### Example outputs

```bash
$ smoke test --code 'const x: number = 42; console.log(x)' --lang ts
{
  "passed": true,
  "stdout": "42",
  "stderr": "",
  "language": "typescript",
  "execution_time_ms": 38
}

$ smoke test --code 'print(1/0)' --lang py
{
  "passed": false,
  "stdout": "",
  "stderr": "ZeroDivisionError: division by zero",
  "language": "python",
  "execution_time_ms": 14
}
```

---

## Configuration

SMOKE merges four config layers, each overriding the previous:

```
built-in defaults
  ← ~/.config/smoke/smoke.toml  (user-global)
  ← .smoke.toml                 (project-level)
  ← --config <path>             (explicit override)
```

Generate a project config with `smoke config init`:

```toml
[limits]
timeout_ms = 2000              # Sandbox execution timeout (ms)
max_file_lines = 200           # Files larger than this use snippet-only mode (Edit tool)
memory_limit_mb = 256          # Memory limit for Python subprocesses
max_file_lines_absolute = 1000 # Files larger than this are skipped (always allowed)

[languages]
js_enabled = true
ts_enabled = true
python_enabled = true

[python]
interpreter = "python3"        # Python interpreter binary or absolute path

[hook]
mode = "advisor"               # "advisor" (warn only), "strict" (block runnable files), "silent" (skip)

[loop_detection]
enabled = true                 # Monitor and prevent repeating agent fail loops
warn_threshold = 2             # Failures of same signature to trigger a warning note
escalate_threshold = 3         # Failures of same signature to trigger forced strategy-change prompt
fingerprint_window_minutes = 30# Time window to track repeating failures (minutes)
state_retention_hours = 24     # Hours to retain session files before cleanup (GC)
```

Every field is optional — only set what you need to change from the defaults.

---

## Security model

### JS / TypeScript

Runs inside V8 via `deno_core`. The engine itself enforces the sandbox — code has **no filesystem or network access** by default. This is a V8 property, not a SMOKE configuration.

### Python

Process-isolated with three layers of defense:

| Layer | What it does |
|-------|-------------|
| **`rlimit`** | Caps CPU time, virtual memory (256 MB), and open file descriptors (32) |
| **seccomp** (Linux only) | Blocks `fork`, `exec`, and raw socket syscalls at the kernel level |
| **Process group kill** | SIGTERM → 500 ms wait → SIGKILL on the whole process group |

**This is not a container-grade sandbox.** Logic-based escapes (`__subclasses__()`, frame manipulation) are not prevented by seccomp. SMOKE is designed to catch bugs in *agent-generated* code before they land on disk — not to sandbox adversarial code. Use [E2B](https://e2b.dev) or [Modal](https://modal.com) for that.

### Rust

Rust code is not executed during verification. Instead, it is verified for compile-correctness:
1. **Workspace-aware Check**: If a `Cargo.toml` is found in the parent directory chain of the edited file, SMOKE temporarily writes the proposed change to its target path, runs `cargo check --tests` inside the workspace to verify types/imports/correctness, and safely restores the original file content.
2. **Standalone Fallback**: If no `Cargo.toml` is found, it compiles the snippet using `rustc --emit=metadata` to check for syntax and compile errors.
3. **Workspace Tests**: In `post-hook` mode, if `Cargo.toml` is present, it auto-runs `cargo test -- <edited-file-stem>` to run tests matching the module name.

---

## Design notes

- **Fail-open**: SMOKE never blocks Claude Code's tool pipeline on internal errors. Parse failures, unknown file extensions, and disabled languages all produce `exit 0` (allow). Only confirmed syntax or runtime errors produce `exit 2` (block).
- **Thread-safe V8 execution**: Deno's V8 engine is run on a dedicated OS thread (`std::thread::spawn`) to avoid conflicts with Tokio's async runtime. Achieves ~20–50 ms cold start.
- **Watchdog**: JS/TS infinite loops are killed by a watchdog OS thread polling every 10 ms — the only reliable way to interrupt synchronous V8 loops.
- **Two-phase kill**: Python timeouts get SIGTERM → 500 ms → SIGKILL to the full process group.
- **Snippet extraction**: For large files (> 200 lines), tree-sitter walks the AST upward from the edited region to find the enclosing function or class and runs only that. Keeps verification fast on big codebases.
- **TSX support**: `.tsx` files are parsed with the TSX dialect grammar, preventing false-positive syntax errors on JSX expressions inside TypeScript.

---

## FAQ

**Does it work with agents other than Claude Code?**
The PreToolUse/PostToolUse hooks are Claude Code–specific. The `smoke server` MCP integration works with any MCP client — Claude Desktop, Windsurf, Cline, Roo Code, and others.

**What about `import` / `require` in JS/TS snippets?**
The V8 sandbox has no filesystem or network access, so `require` or `import` that reaches for external modules will fail. SMOKE tests snippets in isolation — it's not a full Node.js environment.

**Can I disable it for a specific language?**
Yes. Add `javascript = false`, `typescript = false`, or `python = false` to `.smoke.toml`.

**Will it block my agent when there are no tests yet?**
No. `smoke post-hook` only runs tests that exist alongside the edited file. No test file → no check → `exit 0`.

**Why Rust?**
~5 ms JS startup. Embedded V8. Kernel-level seccomp filtering. Tree-sitter parsing at compile time. Zero-copy config merging. Scripting the tool itself would have been a meta-problem.

**The installer says Python is not found — is that a problem?**
Only if you want Python sandboxing. JS/TS works without Python. Install Python 3 and re-run if needed.

---

## Building from source

```bash
git clone https://github.com/senapati484/smoke.git
cd smoke
cargo build --release
# Binary is at ./target/release/smoke
./target/release/smoke --help
```

Run tests:

```bash
cargo test
```

---

## Docs

| Document | What it covers |
|----------|----------------|
| [Getting Started](docs/GETTING-STARTED.md) | Step-by-step tutorial from zero to running |
| [Architecture](docs/ARCHITECTURE.md) | Deep dive into modules, data flow, security model |
| [Configuration](docs/CONFIGURATION.md) | 4-layer config merge, all fields, tuning guidance |
| [Development](docs/DEVELOPMENT.md) | Build commands, design decisions, adding languages |
| [Testing](docs/TESTING.md) | Sandbox testing, test discovery, known limitations |

---

## License

[Apache 2.0](LICENSE)
