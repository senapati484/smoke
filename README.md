<p align="center">
  <img width="400" alt="SMOKE logo" src="https://github.com/user-attachments/assets/4fb65b47-b2af-4254-b49d-3928ef48d41f" />
</p>

<h1 align="center">SMOKE</h1>
<h3 align="center">Write. Run. Know.</h3>

<p align="center">
  A PreToolUse hook for Claude Code that runs AI-generated JS/TS and Python code
  in a sandbox before the agent's file write is allowed to complete.
  The agent discovers bugs the moment it introduces them.
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT" /></a>
  <img src="https://img.shields.io/badge/rust-2021-edition-brown" alt="Rust edition 2021" />
</p>

---

## Overview

SMOKE intercepts code before it reaches disk. When Claude Code calls `Write` or `Edit`, SMOKE:

1. Extracts the code from the tool call
2. Checks syntax with tree-sitter
3. Executes it in an isolated sandbox (V8 for JS/TS, process-isolated for Python)
4. Returns `allow` (exit 0) on clean execution or `block` (exit 2) with error output

Three integration paths are supported: **hook** via `.claude/settings.json`, **MCP server** via `.mcp.json`, and a **standalone CLI** for development.

## Security Model

| Language | Isolation | Capabilities |
|----------|-----------|-------------|
| **JavaScript / TypeScript** | Fully sandboxed by V8 engine | No filesystem, network, or process access |
| **Python** | Process-isolated with resource limits | CPU/memory limits, seccomp (Linux) denies fork/exec/sockets |

**Python** is NOT a full sandbox. Logic-based escapes (`__subclasses__()`, frame manipulation) within the Python VM are not prevented. SMOKE catches bugs in *agent-generated* code — not adversarial code. For untrusted third-party code, use E2B or Modal.

## Quick Start

```bash
# Build the release binary
cargo build --release

# Register the hook (adds to .claude/settings.json automatically)
# Or manually add:
# "hooks": { "PreToolUse": [{ "matcher": "Write|Edit", "hooks": [{ "type": "command", "command": "./target/release/smoke hook", "timeout": 10 }] }] }
```

## Integration Paths

| Path | Config | When to use |
|------|--------|-------------|
| PreToolUse hook | `.claude/settings.json` | Block code with syntax errors or runtime failures before it's written |
| PostToolUse hook | `.claude/settings.json` | Auto-run co-located test files after successful writes |
| MCP server | `.mcp.json` | Invoke `smoke_verify` from any MCP client |
| CLI | `smoke test --code <code> --lang <lang>` | Development, debugging, CI validation |

## Supported Languages and Extensions

- **JavaScript**: `.js`, `.mjs`, `.cjs`, `.jsx`
- **TypeScript**: `.ts`, `.mts`, `.cts`, `.tsx`
- **Python**: `.py`, `.pyw`

## Configuration

SMOKE uses a layered config system. Each level overrides the previous:

1. Built-in defaults
2. `~/.config/smoke/smoke.toml` (user-level)
3. `.smoke.toml` in project root (project-level)
4. `--config <path>` (explicit CLI override)

Generate a default config: `smoke config init`

Key defaults: 1s timeout, 256 MB Python memory limit, 200-line snippet threshold, all languages enabled.

## Architecture

```
Claude Code Agent
     │
     ├─ Write/Edit tool call
     │     │
     │     ▼
     ├─ .claude/settings.json triggers PreToolUse hook → smoke hook
     │     │
     │     ├─ Parse hook JSON from stdin
     │     ├─ Detect language from extension
     │     ├─ Extract/reconstruct code content
     │     ├─ Syntax check (tree-sitter)
     │     ├─ Sandbox execution (V8 or Python subprocess)
     │     └─ Return allow/block via exit code + stdout/stderr
     │
     │  (after successful write)
     │
     └─ PostToolUse hook → smoke post-hook
           ├─ Find co-located test file
           ├─ Run test in sandbox (30s timeout)
           └─ Return pass/fail via exit code
```

## Build

```bash
# Development build (debug)
cargo build

# Production build (LTO, stripped)
cargo build --release
```

The release binary is optimized with `opt-level=3`, LTO, and stripped symbols.

## License

MIT