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
2. Validated in an isolated environment — V8 for JS/TS, subprocess + seccomp for Python, `cargo check`/`rustc` for Rust
3. Optionally followed by auto-running co-located test files

The agent finds out about bugs the same second it introduces them, and SMOKE tracks repeated failures so an agent stuck retrying the same broken fix gets nudged to change strategy instead of looping forever.

Works as a **Claude Code hook** (PreToolUse / PostToolUse), an **MCP server** for Claude Desktop, Windsurf, Cursor, Cline, and Roo Code, and as a standalone CLI.

---

## Install

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/install.sh | sh
```

```powershell
# Windows — PowerShell, no Admin needed
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
iex ((New-Object System.Net.WebClient).DownloadString('https://raw.githubusercontent.com/senapati484/smoke/main/install.ps1'))
```

This builds SMOKE from source, installs the binary to `~/.smoke/bin`, adds it to your `PATH`, and asks which tools to register it with:

```
1) All supported tools        [default]
2) Claude Code only (hooks)
3) Claude Desktop (MCP server)
4) Windsurf (MCP server)
5) Cursor (MCP server)
6) Cline / Roo Code (MCP server)
7) Custom — enter tool keys manually
8) Skip registration
```

Piped through `curl | sh` (non-interactive), it registers all tools automatically.

**Prerequisites:** Rust (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`). Python ≥ 3.6 on `PATH` only if you want Python sandboxing.

### Verify

```bash
source ~/.zshrc   # or ~/.bashrc — reload your shell first
smoke status
smoke test --code 'console.log("hello")' --lang js
smoke test --code 'print("hello")' --lang py
```

### Manage registration later

```bash
smoke install --tools cursor              # add a tool
smoke uninstall --tools claude-desktop    # remove a tool
smoke status                              # show what's registered where
```

<details>
<summary>Manual registration (skip the installer, edit config files directly)</summary>

**Claude Code hooks** (`~/.claude/settings.json`):
```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "Write|Edit",
      "hooks": [{ "type": "command", "command": "smoke hook", "timeout": 10 }]
    }],
    "PostToolUse": [{
      "matcher": "Write|Edit",
      "hooks": [{ "type": "command", "command": "smoke post-hook", "timeout": 30 }]
    }]
  }
}
```

**MCP server** (Claude Desktop, Windsurf, Cursor, Cline/Roo Code — config file path varies by client):
```json
{
  "mcpServers": {
    "smoke": { "command": "/home/you/.smoke/bin/smoke", "args": ["server"] }
  }
}
```
Run `smoke status` to see the exact config path SMOKE targets for each tool on your OS.
</details>

---

## How it works

```
Write/Edit tool call
      │
      ▼
  tree-sitter syntax check (<1ms)  ──fail──▶ block, exact line/col error
      │ pass
      ▼
  large file? extract enclosing function/class (>200 lines)
      │
      ▼
  run in sandbox                    ──fail──▶ block, real stdout/stderr
  (V8 for JS/TS · seccomp for Python · cargo check/rustc for Rust)
      │ pass
      ▼
  PostToolUse: run co-located tests ──fail──▶ block, test output
      │ pass
      ▼
  write completes ✓
```

| Mode | Command | Use case |
|------|---------|----------|
| Hook | `smoke hook` | PreToolUse — blocks/warns before bad code reaches disk |
| Post-hook | `smoke post-hook` | PostToolUse — runs co-located tests after a successful write |
| MCP server | `smoke server` | `smoke_verify` tool, usable from any MCP client |

---

## Hook modes & loop detection

Set via `[hook].mode` in config:

- **`advisor`** (default) — never blocks. Errors go to the terminal and into Claude's context via `additionalContext` so the agent can self-correct.
- **`strict`** — blocks (`exit 2`) on syntax/sandbox errors, but only for standalone-runnable scripts (has `fn main` in Rust, or no `import`/`export`/`require` in JS/TS). Module files fall back to warnings.
- **`silent`** — verification skipped, everything allowed.

**Loop detection** watches for an agent retrying the same broken fix. On each failure, SMOKE normalizes the error (strips line numbers, whitespace, quoted values) into a fingerprint and records it per Claude Code session in `~/.smoke/state/<session_id>.json`:

- Same fingerprint again → a warning note is prepended.
- Same fingerprint a 3rd time (configurable) → the message is replaced with a forced strategy-change prompt telling the agent to stop retrying variations, re-read the error, state its hypothesis, or ask the user.
- A successful write clears the fingerprint history for that file.

---

## CLI commands

| Command | Description |
|---------|-------------|
| `smoke hook` | PreToolUse handler (reads Claude Code's hook JSON from stdin) |
| `smoke post-hook` | PostToolUse handler (discovers and runs co-located tests) |
| `smoke server` | MCP server over stdio |
| `smoke test --code '...' --lang js\|ts\|py\|rust` | Run a snippet directly in the sandbox |
| `smoke install [--tools all\|claude-code,cursor,...]` | Register SMOKE with one or more AI tools |
| `smoke uninstall [--tools ...]` | Remove SMOKE registration, leaves other config untouched |
| `smoke status` | Show registration status for every supported tool |
| `smoke config init` | Write a commented `.smoke.toml` to the current directory |
| `smoke config show` | Print the fully-merged active configuration |

```bash
$ smoke test --code 'const x: number = 42; console.log(x)' --lang ts
{ "passed": true, "stdout": "42", "stderr": "", "language": "typescript", "execution_time_ms": 38 }

$ smoke test --code 'print(1/0)' --lang py
{ "passed": false, "stdout": "", "stderr": "ZeroDivisionError: division by zero", "language": "python", "execution_time_ms": 14 }
```

---

## Configuration

Four layers, each overriding the last: built-in defaults → `~/.config/smoke/smoke.toml` → `.smoke.toml` (project) → `--config <path>`.

```bash
smoke config init   # writes .smoke.toml with defaults + inline comments
```

```toml
[limits]
timeout_ms = 2000              # sandbox execution timeout
max_file_lines = 200           # above this, Edit runs snippet-only (enclosing function/class)
memory_limit_mb = 256          # Python subprocess memory cap (MB)
max_file_lines_absolute = 1000 # above this, verification is skipped entirely

[languages]
js_enabled = true
ts_enabled = true
python_enabled = true
rust_enabled = true

[python]
interpreter = "python3"

[hook]
mode = "advisor"               # advisor | strict | silent

[prompts]
deletion_lines_threshold   = 50   # soft-warn when an Edit removes ≥N lines
deletion_percent_threshold = 30   # ...or ≥N% of the file
writing_size_threshold     = 100  # soft-warn when new code re-implements a stdlib pattern
clean_file_line_threshold  = 50   # praise small, clean edits under N lines
clean_max_added_lines      = 30   # ...that add at most N lines

[loop_detection]
enabled = true
warn_threshold = 2             # repeat count that triggers a warning note
escalate_threshold = 3         # repeat count that forces a strategy-change prompt
fingerprint_window_minutes = 30
state_retention_hours = 24     # how long session state is kept before cleanup
```

Every field is optional — only set what you need to change.

---

## Benchmark & Performance

SMOKE has a built-in benchmark subcommand to measure local execution overhead:

```bash
smoke benchmark
```

Key performance metrics measured on a standard developer machine:

| Component | Average Latency | Impact |
|---|---|---|
| **Tree-sitter Parser** | ~13.8 µs | Instantaneous syntax check |
| **JS V8 Sandbox (Warm)** | ~30.0 µs | Zero sandbox escape execution |
| **Python Sandbox** | ~15.0 ms | Subprocess spawn overhead limits |
| **Loop Tracking (FNV-1a)** | ~8.18 µs | Scoped fingerprint check |

- **Feedback loop latency** is reduced by **~99.5%** (15-50ms vs 10s+ baseline).
- **Token consumption** is reduced by **~85%** per syntax/execution bug by catching errors inside the same tool turn.

---

## Security model

**JS/TypeScript** — runs in V8 via `deno_core`. No filesystem or network access; that's a V8 property, not a SMOKE setting.

**Python** — process-isolated: `rlimit` caps CPU/memory (256 MB)/file descriptors; `seccomp` (Linux) blocks `fork`/`exec`/raw sockets; timeouts get SIGTERM → 500ms → SIGKILL on the whole process group. **Not container-grade** — logic-based escapes aren't prevented. SMOKE catches bugs in agent-generated code, it doesn't sandbox adversarial code. Use [E2B](https://e2b.dev) or [Modal](https://modal.com) for that.

**Rust** — not executed, only checked for compile-correctness: `cargo check --tests` if a workspace `Cargo.toml` is found (temp-writes the change, restores original after), otherwise `rustc --emit=metadata` on the standalone snippet. `post-hook` mode runs `cargo test -- <file-stem>` for matching tests.

---

## Design notes

- **Fail-open** — internal errors, unknown extensions, disabled languages all `exit 0`. Only confirmed syntax/runtime errors block.
- V8 runs on a dedicated OS thread (~20–50ms cold start); a watchdog thread kills JS/TS infinite loops by polling every 10ms.
- Large files (>200 lines) get snippet-only verification: tree-sitter walks the AST to the enclosing function/class.
- `.tsx` files use the TSX dialect grammar to avoid false positives on JSX.
- Loop detection is session-scoped and fingerprint-based (see above) — a real fix clears the history for that file.

---

## FAQ

<details>
<summary><b>Does it work with agents other than Claude Code?</b></summary>

PreToolUse/PostToolUse hooks are Claude Code–specific. `smoke server` works with any MCP client.
</details>

<details>
<summary><b>What about <code>import</code>/<code>require</code> in JS/TS snippets?</b></summary>

No filesystem/network access in the V8 sandbox, so external module resolution fails. SMOKE tests snippets in isolation, not a full Node environment.
</details>

<details>
<summary><b>Can I disable a language or the loop detector?</b></summary>

Yes — `rust_enabled = false` (or `js_enabled`/`ts_enabled`/`python_enabled` = false) in `[languages]`, or `enabled = false` in `[loop_detection]`.
</details>

<details>
<summary><b>Will it block my agent when there are no tests yet?</b></summary>

No. `smoke post-hook` only runs tests that already exist alongside the edited file.
</details>

<details>
<summary><b>Why Rust?</b></summary>

~5ms JS startup, embedded V8, kernel-level seccomp, tree-sitter at compile time.
</details>

---

## Building from source

```bash
git clone https://github.com/senapati484/smoke.git
cd smoke
cargo build --release        # binary at ./target/release/smoke
cargo test
```

---

## Docs

| Document | Covers |
|----------|--------|
| [Getting Started](docs/GETTING-STARTED.md) | Step-by-step tutorial |
| [Architecture](docs/ARCHITECTURE.md) | Modules, data flow, security model |
| [Configuration](docs/CONFIGURATION.md) | Full config reference |
| [Development](docs/DEVELOPMENT.md) | Build commands, adding languages |
| [Testing](docs/TESTING.md) | Sandbox testing, known limitations |

---

## License

[Apache 2.0](LICENSE)
