# SMOKE Architecture

> **Write. Run. Know.** — A PreToolUse hook for Claude Code that executes AI-generated
> code in a sandbox before writes are allowed, and returns real stdout/stderr so the
> agent discovers bugs the same second it introduces them.

## Philosophy

SMOKE sits *between* the agent deciding to write code and the file actually being
written. It intercepts Write and Edit tool calls, runs the proposed code in an
isolated runtime, and feeds the result back into the agent's conversation so errors
are caught before they land on disk. The fundamental belief: *the feedback loop for
code correctness should be measured in milliseconds, not commit cycles.*

Three integration paths serve different deployment models:

| Path | Registration | Use Case |
|------|-------------|----------|
| `hook` (PreToolUse) | `.claude/settings.json` | Per-tool-call verification before write |
| `post-hook` (PostToolUse) | `.claude/settings.json` | Auto-run co-located tests after successful write |
| `server` (MCP) | `.mcp.json` | Explicit `smoke_verify` tool call from agent |

All three paths converge on the same sandbox implementations — there is no special
logic in any integration flavour.

## Module Dependency Graph

```
main.rs
  ├── config       (load, merge, defaults)
  ├── hook         (PreToolUse handler)
  ├── post_hook    (PostToolUse handler)
  ├── mcp          (MCP server via rmcp)
  ├── parser       (tree-sitter syntax checking, snippet extraction)
  └── sandbox
       ├── mod.rs  (shared SandboxResult type)
       ├── js.rs   (rustyscript/V8 runtime)
       └── python.rs (process-isolated Python via std::process)
```

Dependencies between modules are minimal:

```
hook ──> sandbox::js, sandbox::python, config, parser
post_hook ──> sandbox::js, sandbox::python, config
mcp ──> sandbox::js, sandbox::python, config
parser ──> tree-sitter (standalone, no SMOKE-internal deps)
config ──> toml, serde (standalone)
sandbox::js ──> rustyscript (standalone)
sandbox::python ──> tempfile, tokio::process (standalone)
```

No module depends on `main.rs`. No circular dependencies exist.

## Data Flow

```
Claude Code decides to Write/Edit a file
         │
         ▼
 .claude/settings.json matches Write|Edit
         │
         ▼
 Claude launches the hook: `smoke hook` (or `./target/release/smoke hook` for source builds)
         │
         ▼
 Reads hook JSON from stdin
   { session_id, tool_name: "Write"|"Edit",
     tool_input: { file_path, content?, old_string?, new_string? } }
         │
         ├── Not Write/Edit ──── exit 0 (allow, no-op)
         │
         ▼
 Extract file_path → detect language from extension
         │
         ├── Unknown extension? ──── exit 0 (allow, skipped)
         │
         ▼
 Load config (4-layer merge)
         │
         ▼
 ┌── Write tool ───────────────────────────────┐
 │  Extract content from tool_input            │
 │  check_syntax(content) via tree-sitter      │
 │  ┌─ ERROR? ──► exit 2 (block with syntax    │
 │  │              error location)             │
 │  └─ OK ──────► send to sandbox              │
 └─────────────────────────────────────────────┘
 ┌── Edit tool ────────────────────────────────┐
 │  Read current file from disk                │
 │  Find old_string, patch with new_string     │
 │                                             │
 │  check_syntax(patched) via tree-sitter      │
 │  ┌─ ERROR? ──► exit 2 (block)               │
 │  │                                          │
 │  └─ OK ──────► is file > max_file_lines?    │
 │                ├─ YES ─► extract_enclosing  │
 │                │         function snippet   │
 │                └─ NO  ─► send full file     │
 └─────────────────────────────────────────────┘
         │
         ▼
 ┌─ Sandbox Execution ────────────────────────┐
 │  Language detection:                       │
 │    .js/.jsx/.mjs/.cjs → JsSandbox (V8)     │
 │    .ts/.tsx/.mts/.cts → JsSandbox (V8+TS)  │
 │    .py/.pyw          → PythonSandbox       │
 │                                            │
 │  Returns SandboxResult {                   │
 │    passed, stdout, stderr,                 │
 │    execution_time_ms, language             │
 │  }                                         │
 └────────────────────────────────────────────┘
         │
         ▼
 ┌─ Decision ────────────────────────────────┐
 │  passed=true?                             │
 │    ├─ YES ──► stdout (allow with reason)  │
 │    │            exit 0                    │
 │    └─ NO  ──► stderr to hook output       │
 │                 exit 2 (block tool call)  │
 └───────────────────────────────────────────┘
```

### PostToolUse Flow

```
Write/Edit succeeds in Claude Code
         │
         ▼
 Claude launches: ./target/release/smoke post-hook
         │
         ▼
 Reads PostToolUse JSON from stdin
   { tool_name, tool_input: { file_path }, tool_response: { success: true } }
         │
         ▼
 Derived path → language → look for co-located test file:
   .js/.jsx  → ${stem}.test.${ext} or ${stem}.spec.${ext}
   .ts/.tsx  → ${stem}.test.${ext} or __tests__/${stem}.${ext}
   .py/.pyw  → tests/test_${stem}.${ext} or test_${stem}.${ext}
         │
         ├── No test found? ──► exit 0 (silent)
         │
         ▼
 Read test file → sandbox execute (30s timeout)
         │
         ├── passed? ──► exit 0
         └── failed? ──► stderr → exit 2 (block)
```

### MCP Flow

```
Claude Code decides to call smoke_verify tool
         │
         ▼
 Reads .mcp.json → connects via stdio transport to `smoke server`
         │
         ▼
 MCP tool call: smoke_verify { code, language }
         │
         ▼
 Same sandbox dispatch as hook path
         │
         ▼
 Returns SandboxResult as JSON tool response
```

## Module Responsibilities

### `config` — Configuration Loading & 4-Layer Merge

Config is a single `Config` struct loaded once at startup and shared across all
subcommands. The load order implements layered overrides:

```
Layer 1: Built-in defaults (hardcoded in config/mod.rs)
    timeout_ms: 1000, max_file_lines: 200,
    memory_limit_mb: 256, max_file_lines_absolute: 1000,
    all languages enabled, interpreter: "python3"

Layer 2: ~/.config/smoke/smoke.toml (user-level, optional)
    Merged onto Layer 1 with field-level override (None = keep previous)

Layer 3: .smoke.toml in cwd (project-level, optional)
    Merged onto Layer 2

Layer 4: --config <path> (explicit CLI, optional)
    Merged onto Layer 3
```

The merge strategy uses `PartialConfig` structs — every field is `Option<T>`, so
each layer only overrides what it explicitly sets. A missing or malformed file logs
a warning to stderr and is silently skipped (the system never panics on bad config).

Settings exposed:
- **limits.timeout_ms** — hard sandbox timeout (default 1000ms)
- **limits.max_file_lines** — threshold for snippet-only execution (default 200)
- **limits.max_file_lines_absolute** — skip files larger than this (default 1000)
- **limits.memory_limit_mb** — Python process memory limit (default 256)
- **languages.{js,ts,python}_enabled** — per-language toggles
- **python.interpreter** — path to Python binary (default "python3")

### `src/hook` — PreToolUse Handler

This is the primary integration path. Reads Claude Code's hook JSON from stdin,
extracts the tool name and input, detects the language from the file extension, runs
the sandbox, and produces a structured JSON response back to Claude Code.

Key design decisions:
- Unknown file extensions produce `allow` but log a reason to the agent — SMOKE is
  conservative about blocking non-code files.
- Parse errors on stdin JSON produce `exit 0` (allow) — SMOKE must never break
  Claude Code's tool workflow due to its own parser failing.
- Edit tools reconstruct the full file content by reading the current file from disk
  and applying the `old_string` → `new_string` patch.
- Files > `max_file_lines_absolute` (default 1000) are skipped entirely.
- Exit code 2 blocks the tool call in Claude Code, displaying stderr to the agent.

### `src/post_hook` — PostToolUse Handler

Fires after a successful Write or Edit. Its sole job is test discovery and execution.
It looks for test files co-located with the written source file using
language-specific naming conventions, then runs them through the same sandbox with a
30-second timeout.

This is intentionally decoupled from the PreToolUse path — test failures don't block
the original write; they surface as a separate notification.

### `src/mcp` — MCP Server

Exposes `smoke_verify` as an MCP tool over stdio using the `rmcp` crate (v0.16.0).
This is the production integration path for projects that prefer the MCP protocol
over the hook system. The MCP server reuses `JsSandbox` and `PythonSandbox`
directly — no sandbox logic duplication.

Registered in `.mcp.json` as a stdio transport running `smoke server`.

### `src/parser` — Syntax Checking & Snippet Extraction

Uses tree-sitter with grammars for JavaScript, TypeScript, and Python. Two
functions:

1. **`check_syntax(code, language_id)`** — Parses code into a CST. If the root node
   has errors, it walks down to find the lowest error node and returns a
   human-readable `"Syntax error at line N, column M"` message. Returns `None` on
   valid code or unsupported languages.

2. **`extract_enclosing_function(code, edit_start, language_id)`** — Given a byte
   offset into a file, walks up the CST to find the enclosing function/class/arrow
   definition and returns just that node's source text. This enables snippet-only
   execution for large files: instead of running the entire file (which may be slow
   or have side effects), SMOKE runs only the affected function.

### `src/sandbox` — Code Execution

The sandbox module defines the shared `SandboxResult` type and two implementations.
The result type is identical for both backends; the isolation mechanisms are not.

```rust
pub struct SandboxResult {
    pub passed: bool,
    pub stdout: String,
    pub stderr: String,
    pub execution_time_ms: u64,
    pub language: String,
}
```

#### `src/sandbox/js.rs` — JavaScript/TypeScript Sandbox (V8)

Uses `rustyscript` (a safe wrapper around `deno_core`/V8). The V8 runtime is
initialized with **no extensions** — no filesystem, no network, no environment
variable access. This provides a true sandbox: executed code can only compute and use
memory.

**Watchdog thread for infinite loops:**

```ascii
Main thread:        V8 runtime executing user code
                        │
Watchdog thread:         │
  polls `finished`       │
  every 10ms             │
                        │
  timeout reached?       │
    ├─ finished? ──► return (clean exit)
    └─ not finished? ──► isolate_handle.terminate_execution()
                              │
                              ▼
                        V8 throws "ExecutionTerminated"
                        caught in eval error handling
```

V8's synchronous execution model means `tokio::time::timeout` alone cannot
interrupt an infinite loop (`while(true){}`). The watchdog thread runs in a
separate OS thread and calls `V8::TerminateExecution` on the isolate handle, which
forces V8 to throw an exception at the next safe point.

Console output is captured by injecting a preamble that replaces `console.log`,
`console.error`, `console.warn`, and `console.info` with wrappers that push to
`globalThis._smoke_logs` and `globalThis._smoke_errs`. After execution, these arrays
are retrieved via `runtime.eval`.

TypeScript is handled by loading the code as a Deno module (`Module::new("index.ts", ...)`),
which triggers Deno's built-in TypeScript transpilation.

#### `src/sandbox/python.rs` — Python Sandbox (Process-Isolated)

Python runs as a separate OS process via `std::process::Command`. No in-process
Python binding (no pyo3) — the isolation boundary is the OS process boundary.

**Process limits applied in `pre_exec`:**

| Limit | Value | Mechanism |
|-------|-------|-----------|
| CPU time | timeout_ms/1000 + 2 seconds | `rlimit::Resource::CPU` |
| Address space | 256 MB | `rlimit::Resource::AS` |
| Open files | 32 max | `rlimit::Resource::NOFILE` |
| Process group | New `setpgid(0,0)` | `libc::setpgid` |

**Linux-only: seccomp filter via `extrasafe`:**

Denies `fork`, `exec`, and raw socket operations. The filter is applied in
`pre_exec` so it runs in the child process after fork but before `exec`.

**Timeout handling:**

Uses `tokio::select!` between `child.wait_with_output()` and
`tokio::time::sleep(timeout)`. On timeout:
1. Kill the process group with SIGTERM
2. Sleep 500ms
3. Force-kill the process group with SIGKILL

This two-phase kill handles both cooperative and uncooperative processes.

## Config Merge Strategy (4 Layers)

```
Default values ─► ~/.config/smoke/smoke.toml ─► .smoke.toml ─► --config <path>
(hardcoded)         (optional, merges)           (optional,    (optional,
                                                  merges)       merges)
```

The merge is implemented via `PartialConfig` structs where every field is
`Option<T>`. Each layer starts from the previous layer's result and only overrides
fields that are `Some(...)` in the current layer. This means:
- An empty config file is valid — it changes nothing.
- A single-line file like `timeout_ms = 5000` only overrides that key.
- User-level and project-level files are independently optional.

Loading must succeed: any `read_to_string` or `toml::from_str` failure is logged to
stderr and the layer is skipped. The system never crashes on bad config.

## Sandbox Design

### V8 (JS/TS) — True Sandbox

| Property | Status |
|----------|--------|
| Filesystem | ❌ Denied — no fs extensions loaded |
| Network | ❌ Denied — no net extensions loaded |
| Environment | ❌ Denied — no env extensions loaded |
| Memory | ✅ Available — limited only by OS/heap |
| Infinite loops | ✅ Watchdog thread terminates V8 isolate |
| Syntax errors | ✅ Pre-checked by tree-sitter before execution |
| Runtime errors | ✅ Caught, returned in SandboxResult.stderr |
| Timeout | ✅ rustyscript native timeout + watchdog |

### Python — Process Isolation (Partial)

| Property | Status |
|----------|--------|
| Filesystem | ❌ Not restricted — can read/write any file the user can |
| Network | ⚠️ Linux: seccomp blocks raw sockets; macOS: not restricted |
| Fork/exec | ⚠️ Linux: partially denied via seccomp; macOS: not restricted |
| Memory | ✅ rlimit AS set to 256MB |
| CPU time | ✅ rlimit CPU set to timeout + 2s |
| Open files | ✅ rlimit NOFILE set to 32 |
| Infinite loops | ✅ OS-level SIGTERM/SIGKILL |
| Syntax errors | ✅ Pre-checked by tree-sitter before execution |
| Runtime errors | ✅ Caught via exit code + stderr |
| Timeout | ✅ tokio::select! with configurable timeout + process group kill |

The Python sandbox is explicitly **not a security sandbox**. It prevents resource
exhaustion and accidental damage but is not hardened against malicious code. On
Linux, the seccomp filter adds partial protection; on macOS, there is no equivalent
syscall filtering.

## Syntax Checking Pipeline

All code goes through tree-sitter before reaching the sandbox:

```
Code string
    │
    ▼
Parser::set_language(grammar)  ──►  get_ts_language(language_id)
    │                              "js"  → tree_sitter_javascript
    │                              "ts"  → tree_sitter_typescript
    │                              "py"  → tree_sitter_python
    ▼
parser.parse(code)
    │
    ▼
root_node.has_error()?
    ├── NO  ──► return None (no error, proceed)
    │
    └── YES
          │
          ▼
        find_error_node(root)  ──► walks children recursively
          │                         returns deepest error node
          ▼
        "Syntax error at line N, column M"
        ──► exit 2 (block tool call)
```

This catches malformed code before it ever reaches the sandbox, giving the agent an
instant rejection without the overhead of spawning a runtime.

## Edit Tool Handling (File Patching)

The Edit tool only sends the changed fragment (`old_string` + `new_string`), not the
full file. SMOKE reconstructs the full file content:

```
1. Read current file from disk (using cwd + file_path)
2. Find old_string in file content
3. Patched = content[..idx] + new_string + content[idx + old_string.len()..]
4. check_syntax(patched)  — block on error
5. If patched > max_file_lines (200):
     extract_enclosing_function(patched, edit_start, lang)
     ┌─ success? ──► run snippet only
     └─ fallback ──► run full patched content
```

Snippet extraction walks the tree-sitter CST upward from the byte offset of the edit.
If it finds a function, class, arrow function, or method definition, it extracts just
that node's source. This is logged as `"SMOKE: large file — snippet only"` in the allow reason.

## PostToolUse Test Discovery

Co-located test file conventions, by language extension:

### JavaScript (.js, .mjs, .cjs, .jsx)
```
$stem.test.$ext    # e.g. util.test.js
$stem.spec.$ext    # e.g. util.spec.js (fallback if .test doesn't exist)
```

### TypeScript (.ts, .mts, .cts, .tsx)
```
$stem.test.$ext              # e.g. util.test.ts (checked first)
__tests__/$stem.$ext         # e.g. __tests__/util.ts (fallback)
```

### Python (.py, .pyw)
```
tests/test_$stem.$ext         # e.g. tests/test_util.py (checked first, relative to cwd)
test_$stem.$ext               # e.g. test_util.py (fallback, same directory as source)
```

If no test file is found at any candidate path, the post-hook exits 0 silently.

## MCP Server Integration

The MCP server uses `rmcp` v0.16 with `features = ["server", "transport-io"]`.
It implements the `ServerHandler` trait with a single tool:

**Tool: `smoke_verify`**
- Input: `{ code: string, language: "javascript"|"typescript"|"python" }`
- Output: `SandboxResult` JSON object (`{ passed, stdout, stderr, execution_time_ms, language }`)

The server registers via `#[tool_router]` macro and serves over stdio transport.
It loads config from disk on every invocation (no cached config — respects dynamic
`.smoke.toml changes).

Registered in the project as a stdio MCP server:
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

## Error Handling Philosophy

| Situation | Behavior | Rationale |
|-----------|----------|-----------|
| Stdin parse failure | `exit 0` (allow) | SMOKE must not block Claude Code's tool pipeline due to a version mismatch or unexpected JSON shape |
| Config file not found | Defaults applied, warning to stderr | Config is optional |
| Config file malformed | Defaults applied, warning to stderr | Partial config is better than no config |
| Unknown file extension | `exit 0` (allow, with reason) | SMOK is opt-in by file type; unknown types pass through |
| Syntax error | `exit 2` (block, with location) | Malformed code should never reach disk |
| Sandbox runtime error | `exit 2` (block, with stderr) | Runtime errors should be surfaced to the agent |
| Sandbox timeout | `exit 2` (block, timeout message) | Hanging code should not land on disk |
| Language disabled in config | `exit 0` (allow, with reason) | Config toggles are advisory, not enforcement |
| Edit old_string not found | `exit 0` (allow) | Cannot verify what cannot be reconstructed |
| Edit file > 1000 lines | `exit 0` (allow, reason stated) | Arbitrary cutoff to avoid slow verification on very large files |

The `allow` path always includes a machine-readable reason in
`permissionDecisionReason` so the agent can see why verification was skipped.

## Security Model Trade-offs

| | V8 (JS/TS) | Python (process) |
|---|---|---|
| Isolation mechanism | In-process runtime with no fs/net extensions | Separate OS process via fork-exec |
| True sandbox? | ✅ Yes — code cannot reach the host system's filesystem, network, or environment | ❌ No — process-isolated but not sandboxed against intentional escape |
| Resource exhaustion | ✅ V8 isolates memory by default; watchdog for CPU | ✅ rlimits on CPU, memory, open files; two-phase kill on timeout |
| Escalation risk | ✅ Low — no `require("fs")` or `Deno.readFile`, no `--allow-` flags granted | ⚠️ Medium — Python can read/write any file the user can; seccomp on Linux partially mitigates |
| Start-up cost | ~5ms (in-process V8) | ~50-100ms (process spawn) |
| Suitability | **Trusted LLM output** — the agent's generated code, tested immediately | **Trusted LLM output** with resource guards — not for untrusted third-party code |

The fundamental trade-off: V8 was designed to be embedded and sandboxed; Python was
not. SMOKE's V8 sandbox is genuinely safe for arbitrary code execution. The Python
sandbox is safe against accidental damage (infinite loops, memory exhaustion) but not
against intentional subversion. Both are appropriate for their intended use case —
verifying AI-generated code before the agent writes it — because the adversary is an
LLM hallucinating buggy code, not a deliberate attacker.