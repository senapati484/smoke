# SMOKE Development Guide

> **Write. Run. Know.** — A PreToolUse hook for Claude Code that executes AI-generated code in a sandbox before writes are allowed.

---

## 1. Quick Start

### One-liner install (for users)

**macOS / Linux:**
```bash
curl -fsSL https://raw.githubusercontent.com/senapati484/smoke/main/install.sh | sh
```

**Windows (PowerShell):**
```powershell
iex ((New-Object System.Net.WebClient).DownloadString('https://raw.githubusercontent.com/senapati484/smoke/main/install.ps1'))
```

Both install scripts build from source, copy the binary to `~/.smoke/bin/`,
configure PATH, and offer interactive agent registration.

### Prerequisites (for developers)

| Requirement | Version |
|-------------|---------|
| Rust toolchain | edition 2021 (stable) |
| Python 3 | Any recent version (for Python sandbox) |

Install the Rust toolchain via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup default stable
```

Python is not required for building — only at runtime when the Python sandbox is invoked.

---

## 2. Build Commands

### Debug Build

```bash
cargo build
```

Output: `target/debug/smoke`

### Release Build (LTO, stripped)

```bash
cargo build --release
```

Output: `target/release/smoke`

The release profile is configured in `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

### Cross-Platform Notes

- **macOS / Linux**: Full support. Python sandbox uses `rlimit` for resource limits and `libc` for process group management.
- **Linux only**: The Python sandbox additionally applies a seccomp filter via `extrasafe` (denies fork/exec/sockets). See `src/sandbox/python.rs:71-79`.
- **Windows**: Not currently supported. `rlimit`, `extrasafe`, and `libc` calls are all Unix-specific. The `target.'cfg(target_os = "linux")'` dependency for `extrasafe` is gated, but `rlimit` and `libc` are unconditional.

### Dependency Pinning

`serde` is pinned at exactly `=1.0.220` to avoid breaking `swc_common` on newer versions. When updating dependencies, preserve this pin unless you have verified compatibility.

---

## 3. Development Workflow

### Run the CLI directly (fastest feedback)

```bash
cargo run -- test --code 'console.log("hello")' --lang js
cargo run -- test --code 'print("hello")' --lang py
cargo run -- test --code 'const x: number = 1; console.log(x);' --lang ts
```

### Run as a hook (requires registration)

Build first, then Claude Code invokes the binary automatically:

```bash
cargo build --release
```

The hook is registered in `.claude/settings.json`. When Claude Code calls `Write` or `Edit`, it launches `./target/release/smoke hook` with the hook JSON on stdin.

### Run as an MCP server

```bash
cargo run -- server
```

The MCP server listens on stdio and exposes the `smoke_verify` tool. Register in `.mcp.json`.

### Config management

```bash
cargo run -- config init       # Write .smoke.toml with defaults
cargo run -- config show       # Print active merged config
```

### Test with sample Write payloads

Piped stdin is the typical integration path for `smoke hook`:

```bash
echo '{
  "session_id": "test",
  "transcript_path": "/dev/null",
  "cwd": "'$PWD'",
  "hook_event_name": "PreToolUse",
  "tool_name": "Write",
  "tool_input": {
    "file_path": "test.js",
    "content": "console.log(2 + 2)"
  }
}' | cargo run -- hook
```

### Test with sample Edit payloads

```bash
echo '{
  "session_id": "test",
  "transcript_path": "/dev/null",
  "cwd": "'$PWD'",
  "hook_event_name": "PreToolUse",
  "tool_name": "Edit",
  "tool_input": {
    "file_path": "src/main.js",
    "old_string": "old code",
    "new_string": "new code"
  }
}' | cargo run -- hook
```

### Debugging Tips

- Add `eprintln!("SMOKE DEBUG: ...")` for diagnostic output — it appears in Claude Code's hook stderr.
- The hook JSON input is the canonical source of truth; inspect it with `serde_json::from_str` breakpoints or a `eprintln!("{}", buffer)` before parsing.
- Exit code 2 blocks the tool call in Claude Code; exit 0 allows it.
- To verify the tree-sitter syntax check independently: `cargo run -- test --code 'broken syntax ===' --lang js`

---

## 4. Project Structure

```
smoke/
├── Cargo.toml              # Dependencies, release profile
├── .claude/
│   └── settings.json       # Hook registration (PreToolUse + PostToolUse)
├── .mcp.json               # MCP server registration
├── README.md               # Project overview, quick start, security model
├── docs/
│   ├── ARCHITECTURE.md     # Deep architecture, data flow, module responsibilities
│   ├── CONFIGURATION.md    # Config reference, 4-layer merge, tuning
│   └── DEVELOPMENT.md      # You are here
└── src/
    ├── main.rs             # CLI entry point, subcommand dispatch
    ├── config/
    │   └── mod.rs          # 4-layer config loading, PartialConfig merge, defaults
    ├── hook/
    │   └── mod.rs          # PreToolUse handler — reads hook JSON, dispatches sandbox
    ├── post_hook/
    │   └── mod.rs          # PostToolUse handler — test discovery and execution
    ├── mcp/
    │   └── mod.rs          # MCP server — exposes smoke_verify tool over stdio
    ├── parser/
    │   └── mod.rs          # tree-sitter syntax checking, snippet extraction
    └── sandbox/
        ├── mod.rs          # Shared SandboxResult type
        ├── js.rs           # JS/TS sandbox (rustyscript/V8)
        └── python.rs       # Python sandbox (process-isolated)
```

---

## 5. Key Design Decisions

### 5.1 Why V8 (via rustyscript/deno_core) for JS/TS instead of a subprocess

V8 is embedded in-process via `rustyscript` (a safe wrapper around `deno_core`). This gives us:

- **True sandbox**: V8 is initialized with no filesystem, network, or environment extensions — executed code *cannot* escape.
- **Low latency**: ~5ms startup per invocation vs ~50-100ms for spawning a Node.js subprocess.
- **Deterministic timeout**: A watchdog thread calls `V8::TerminateExecution` on the isolate handle to break infinite loops synchronously.
- **TypeScript transpilation**: Deno's built-in TypeScript support handles `.ts` files transparently via `Module::new("index.ts", ...)`.

A subprocess approach (spawning `node` or `deno` for each invocation) would lose all three advantages: no fs/net restrictions without wrappers, higher overhead, and no reliable way to kill infinite synchronous loops.

### 5.2 Why process-isolated Python instead of pyo3

Python runs as a separate OS process via `std::process::Command`. No `pyo3`, no in-process CPython embedding.

- **Isolation boundary**: The OS process boundary is the isolation mechanism. A crash in Python cannot segfault the SMOKE process. Memory leaks in Python are reclaimed by the OS when the child process exits.
- **Resource limits**: `rlimit` enforces CPU time, address space (256 MB), and open file descriptors in the child process via `pre_exec`. On Linux, `extrasafe` applies a seccomp filter to deny fork/exec/sockets.
- **No GIL contention**: The Python runtime runs independently — no blocking the Rust async runtime with GIL acquisition.
- **Startup cost accepted**: ~50-100ms per invocation is acceptable for a hook that fires per-tool-call.

`pyo3` would require linking CPython into the binary, increasing build complexity and exposing the host process to Python runtime crashes. The isolation boundary would be in-process (GIL, subinterpreters), which is weaker than an OS process boundary.

### 5.3 Why 4-layer config with PartialConfig merge

Config is loaded from up to four sources, each overridding the previous:

```
Layer 1: Built-in defaults (hardcoded in Config::default())
Layer 2: ~/.config/smoke/smoke.toml (user-level, optional)
Layer 3: .smoke.toml in project root (project-level, optional)
Layer 4: --config <path> (explicit CLI override)
```

The merge uses `PartialConfig` structs where every field is `Option<T>`. Each layer only overrides what it explicitly sets. This means:

- An empty config file changes nothing.
- A single-line `timeout_ms = 5000` only overrides that key.
- Missing or malformed files are logged to stderr and skipped — the system never panics on bad config.
- The hook specifically resolves the project-level config using the `cwd` field from Claude Code's hook JSON, ensuring per-project settings are respected.

Without this pattern, every config file would need to specify every field (rigid) or SMOKE would need to deserialize into a flat struct with `#[serde(default)]` on every field, which doesn't support layered overrides (a missing file would reset all values to defaults, losing the previous layer's overrides).

### 5.4 Why tree-sitter for syntax checking (not a full LSP)

tree-sitter provides a Concrete Syntax Tree (CST) for fast parsing:

- **Speed**: Parsing happens in microseconds, not milliseconds. A full LSP (like `typescript-language-server` or `pyright`) would add 100-500ms startup per invocation and require managing a language server process.
- **No process spawning**: tree-sitter is a Rust library compiled into the binary. No child processes, no IPC.
- **Snippet extraction**: The CST enables walking upward from a byte offset to find the enclosing function/class definition — critical for the Edit tool's snippet-only execution path. A flat regex or line-based approach would be fragile.
- **Limited scope**: SMOKE only needs syntax validity (has errors?) and scope walking (find enclosing function). It does not need type checking, diagnostics, or autocompletion — features that would justify an LSP.

### 5.5 Why fail-open by default

SMOKE's error handling philosophy: **SMOKE must never break Claude Code's tool workflow.**

| Situation | Behavior | Rationale |
|-----------|----------|-----------|
| Stdin parse failure | `exit 0` (allow) | Prevent version mismatches from blocking tool calls |
| Config file malformed | Skip + warning | Partial config is better than crashing |
| Unknown file extension | `exit 0` (allow) | SMOKE is opt-in by file type |
| Language disabled in config | `exit 0` (allow) | Config toggles are advisory |
| Edit `old_string` not found | `exit 0` (allow) | Cannot verify what cannot be reconstructed |

The only cases where SMOKE blocks (exit 2) are syntax errors and sandbox runtime failures — the exact bugs it is designed to catch.

---

## 6. Adding a New Language Sandbox

To add support for a new language (e.g., Ruby, Go, Rust):

### Step 1: Create the sandbox implementation

Create `src/sandbox/<lang>.rs` following the pattern in `js.rs` or `python.rs`:

- Define a struct (e.g., `RubySandbox`) with a `new()` constructor.
- Implement an `async fn execute(&mut self, code: &str, ...) -> SandboxResult`.
  - If sandboxed in-process: use an embedded runtime (like `rustyscript` for V8).
  - If process-isolated: use `tokio::process::Command` with resource limits.
- Use `SandboxResult::error()` and `SandboxResult::success()` helpers from `sandbox/mod.rs`.

### Step 2: Register in `src/sandbox/mod.rs`

Add `pub mod <lang>;` and re-export the struct if needed.

### Step 3: Add config fields

In `src/config/mod.rs`:

- Add a toggle to `Languages` (e.g., `ruby_enabled: bool`, default `true`).
- Add to `PartialLanguages` and the `merge()` function.
- Add to `write_default_config()` template.

### Step 4: Add CLI dispatch in `src/main.rs`

Add a match arm in `run_test()`:

```rust
"rb" | "ruby" => {
    if !cfg.languages.ruby_enabled {
        anyhow::bail!("Ruby sandbox is disabled in config");
    }
    let mut sandbox = sandbox::ruby::RubySandbox::new();
    sandbox.execute(code, ...).await
}
```

### Step 5: Add hook dispatch in `src/hook/mod.rs`

Add the extension mapping in the language detection block (around line 92) and add the sandbox call in the execution dispatch block (around line 196).

### Step 6: Add tree-sitter grammar (optional)

If you want syntax checking for the new language, add the grammar to `Cargo.toml` (e.g., `tree-sitter-ruby = "0.25"`) and add it to `get_ts_language()` in `src/parser/mod.rs`.

### Step 7: Add to PostToolUse test discovery

Add test file naming conventions in `src/post_hook/mod.rs`.

---

## 7. Testing

### CLI sandbox testing

The `smoke test` subcommand is the primary manual testing tool:

```bash
cargo run -- test --code 'console.log("hello world")' --lang js
cargo run -- test --code 'print("hello world")' --lang py
cargo run -- test --code 'const x: number = 1;' --lang ts
```

Expected output is a `SandboxResult` JSON object with `passed`, `stdout`, `stderr`, `execution_time_ms`, and `language`.

### Testing syntax errors

```bash
cargo run -- test --code 'console.log(' --lang js
```
Expected: exit 2 with syntax error at a specific line/column.

### Testing timeouts

```bash
cargo run -- test --code 'while(true){}' --lang js
```
Expected: exit 2 with timeout message (after ~1000ms default).

### Testing Python resource limits

```bash
cargo run -- test --code 'import time; time.sleep(10)' --lang py
```
Expected: exit 2 with timeout message.

### Hook integration testing

Pipe sample hook JSON:

```bash
echo '{...}' | cargo run -- hook
```

### MCP server testing

Start in one terminal:

```bash
cargo run -- server
```

In another, send an MCP request over stdio (or use an MCP client).

---

## 8. Debugging Tips

### Reading hook JSON input

The hook reads a single JSON object from stdin. The shape is defined in `HookInput` at `src/hook/mod.rs:12-18`. To see the exact JSON being sent by Claude Code, add a temporary `eprintln!("STDIN: {}", buffer);` before the parse step.

### Testing with sample Write payloads

```bash
echo '{"session_id":"x","transcript_path":"/dev/null","cwd":"'$PWD'","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":"test.js","content":"console.log(42)"}}' | cargo run -- hook
```

### Testing with sample Edit payloads

Create a test file first:

```bash
echo 'let x = 1;' > /tmp/test_edit.js
```

Then pipe the Edit JSON:

```bash
echo '{"session_id":"x","transcript_path":"/dev/null","cwd":"'$PWD'","hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"/tmp/test_edit.js","old_string":"let x = 1;","new_string":"let x = 2;"}}' | cargo run -- hook
```

### Testing snippet extraction

Create a file > 200 lines with a function containing a known bug, then Edit a line inside that function. SMOKE should extract just the function and execute it.

### Testing language detection

The extension-to-language mapping is at `src/hook/mod.rs:92-96`:

```rust
"js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
"ts" | "mts" | "cts" | "tsx" => Some(Language::TypeScript),
"py" | "pyw" => Some(Language::Python),
```

### Console output capture (JS)

The sandbox injects a preamble that replaces `console.log/error/warn/info` with wrappers that push to `globalThis._smoke_logs` / `globalThis._smoke_errs`. If console output isn't appearing, check that the preamble is being prepended correctly.

### V8 watchdog thread

The watchdog thread in `src/sandbox/js.rs:44-56` polls every 10ms and calls `terminate_execution()` on timeout. If the watchdog isn't firing, check:
- The `AtomicBool finished` flag is being set after execution (`src/sandbox/js.rs:84`).
- The `thread_safe_handle()` is obtained before spawning the watchdog (`src/sandbox/js.rs:39`).

---

## 9. Release Profile

Configured in `Cargo.toml`:

```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

| Setting | Value | Effect |
|---------|-------|--------|
| `opt-level` | 3 | Maximum optimization (speed-focused) |
| `lto` | true | Link-time optimization across crate boundaries |
| `codegen-units` | 1 | Single codegen unit for maximum inlining |
| `strip` | true | Removes debug symbols from binary |

Build time is slower but the resulting binary is smaller and faster — important for a hook that fires on every tool call.

---

## 10. Contributing Guidelines

### Code Style

- Follow existing patterns: 4-space indentation, no trailing whitespace, comments above the code they describe.
- Use `anyhow::Result` for fallible functions. Return `SandboxResult` from sandbox implementations (never panic).
- Use `eprintln!("SMOKE: ...")` for user-facing diagnostic messages.
- Add debug output with `SMOKE DEBUG:` prefix to distinguish from error messages.
- All timeout and limit values throughout the codebase must come from `Config` — never from hardcoded literals.
- `SandboxResult` is the shared return type for all sandboxes. The fields are identical, the isolation mechanisms are not. Do not unify the sandbox implementations.

### Dependency Management

| Dependency | Policy |
|------------|--------|
| `serde` | **Pinned** at `=1.0.220` — do not bump without verifying compatibility with `swc_common` |
| `rmcp` | **Pinned** at `=0.16.0` — MCP protocol is evolving; pin to match the stable API |
| Others | Semver-compatible updates are fine (`"0.12"`, `"1"`, etc.) |

Before adding a new dependency, consider:
1. Is it essential? (e.g., no, you don't need another CLI framework — `clap` is already there.)
2. Does it pull in a heavy dependency tree? (Check with `cargo tree`.)
3. Does it require native libraries? (Avoid `openssl-sys`, `libgit2-sys`, etc.)

### Pull Request Checklist

- [ ] `cargo build` succeeds (debug)
- [ ] `cargo build --release` succeeds
- [ ] `cargo test` passes (if tests exist)
- [ ] `cargo clippy` has no new warnings
- [ ] New sandbox? Added to `smoke test` CLI dispatch, hook dispatch, config, and post-hook test discovery
- [ ] Config change? Updated `PartialConfig`, `merge()`, `write_default_config()`, and CONFIGURATION.md
- [ ] No panic paths introduced — prefer `SandboxResult::error()` over `unwrap()`/`expect()`
