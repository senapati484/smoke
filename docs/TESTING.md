# SMOKE Testing Guide

## SandboxResult

Every sandbox execution returns a `SandboxResult` (`src/sandbox/mod.rs:12`):

| Field | Type | Description |
|---|---|---|
| `passed` | `bool` | `true` if code ran without exceptions, timeout, or non-zero exit |
| `stdout` | `string` | Captured stdout (console.log for JS, `print()` for Python) |
| `stderr` | `string` | Error message if `passed=false`, or stderr stream content |
| `execution_time_ms` | `u64` | Wall-clock time from sandbox entry to exit |
| `language` | `string` | `"javascript"`, `"typescript"`, or `"python"` |

Helper constructors: `SandboxResult::error(language, stderr, elapsed_ms)` and `SandboxResult::success(language, stdout, elapsed_ms)` (`src/sandbox/mod.rs:26-48`).

---

## `smoke test` CLI Subcommand

Standalone sandbox runner for development and debugging (`src/main.rs:50`).

```bash
smoke test --code "<code>" --lang <language>
```

**Supported languages:** `js`, `javascript`, `ts`, `typescript`, `py`, `python`

**Timeout:** Default `timeout_ms` from config (1s). Configure in `.smoke.toml` under `[limits]`.

**Examples:**

```bash
# JavaScript — prints result as JSON
smoke test --code "console.log('hello');" --lang js

# TypeScript
smoke test --code "const x: number = 42; console.log(x);" --lang ts

# Python
smoke test --code "print('hello')" --lang py

# Failure output
smoke test --code "throw new Error('fail');" --lang js
```

The result is printed as pretty-printed JSON containing all `SandboxResult` fields. Exit code is always 0 for the `test` subcommand (it prints the result regardless of `passed`). This is different from hook mode where exit code 2 blocks the tool call.

---

## PostToolUse Test Discovery

After a successful `Write` or `Edit`, the PostToolUse hook (`src/post_hook/mod.rs:54-95`) searches for a co-located test file using these conventions:

### JavaScript (`js`, `mjs`, `cjs`, `jsx`)

Same directory, in priority order:

1. `{stem}.test.{ext}` (e.g., `utils.test.js` for `utils.js`)
2. `{stem}.spec.{ext}` (e.g., `utils.spec.js` for `utils.js`)

### TypeScript (`ts`, `mts`, `cts`, `tsx`)

In priority order:

1. `{stem}.test.{ext}` in same directory (e.g., `utils.test.ts` for `utils.ts`)
2. `__tests__/{stem}.{ext}` subdirectory (e.g., `__tests__/utils.ts` for `utils.ts`)

### Python (`py`, `pyw`)

In priority order:

1. `tests/test_{stem}.py` relative to workspace root (cwd) (e.g., `tests/test_utils.py` for `utils.py`)
2. `test_{stem}.py` in same directory (e.g., `test_utils.py` for `utils.py`)

**Naming summary table:**

| Source file | Test file(s) searched |
|---|---|
| `utils.js` | `utils.test.js`, `utils.spec.js` |
| `utils.jsx` | `utils.test.jsx`, `utils.spec.jsx` |
| `utils.ts` | `utils.test.ts`, `__tests__/utils.ts` |
| `utils.tsx` | `utils.test.tsx`, `__tests__/utils.tsx` |
| `utils.py` | `tests/test_utils.py`, `test_utils.py` |
| `utils.pyw` | `tests/test_utils.pyw`, `test_utils.pyw` |

Tests run with a **hardcoded 30-second timeout** (`src/post_hook/mod.rs:111`), regardless of `config.limits.timeout_ms`. On failure, the test's stderr is printed to the agent with `SMOKE tests failed:` prefix and exit code 2.

---

## Manual Testing (Sandbox Behavior)

Use `smoke test` to verify sandbox behavior directly without hook integration:

```bash
# Verify a simple JS execution
smoke test --code "console.log('works')" --lang js

# Verify failure reporting
smoke test --code "not_valid_syntax;;;" --lang js

# Verify Python stderr capture
smoke test --code "import sys; sys.stderr.write('err')" --lang py

# Verify timeout handling (Python, 1s default)
smoke test --code "while True: pass" --lang py

# Verify timeout handling (JS, watchdog terminates V8)
smoke test --code "while(true){}" --lang js
```

---

## Test Examples

### JS — passing test

```js
// utils.test.js
// Asserts that add() returns the sum of two numbers
const add = (a, b) => a + b;
const result = add(2, 3);
if (result !== 5) {
  console.error(`FAIL: expected 5, got ${result}`);
} else {
  console.log('PASS: add(2, 3) = 5');
}
```

### JS — failing test

```js
// utils.test.js
const add = (a, b) => a + b;
const result = add(2, 3);
if (result !== 99) {
  console.error(`FAIL: expected 99, got ${result}`);
}
```

Writes to `console.error` cause the sandbox to mark the result as `passed: false` (`src/sandbox/js.rs:131-138`). The agent sees the error message and exit code 2.

### Python — passing test

```python
# tests/test_utils.py
def add(a, b):
    return a + b

result = add(2, 3)
assert result == 5, f"FAIL: expected 5, got {result}"
print("PASS: add(2, 3) = 5")
```

### Python — failing test

```python
# tests/test_utils.py
def add(a, b):
    return a + b

result = add(2, 3)
assert result == 99, f"FAIL: expected 99, got {result}"
```

Non-zero exit code and stderr output cause `passed: false`.

---

## What Sandboxing Prevents

| Threat | JS/TS (V8) | Python (rlimits + seccomp) |
|---|---|---|
| Filesystem reads/writes | Blocked — no `fs` extension loaded in deno_core | Blocked via rlimit NOFILE (max 32 fds), seccomp blocks `open`/`openat` on Linux |
| Network access | Blocked — no `net`/`fetch` in V8 runtime | Blocked via seccomp (sockets denied) on Linux |
| Process spawning | Blocked — no `child_process`/`exec` | Blocked via seccomp (`fork`/`exec` denied) on Linux |
| CPU exhaustion | Caught by watchdog thread calling `v8_isolate.terminate_execution()` | Caught by rlimit CPU (timeout + 2s grace) |
| Memory exhaustion | V8 process-level GC handles this | Caught by rlimit AS (256 MB) |
| Environment variable access | Blocked — `Deno.env` not available | Not explicitly blocked, but agent code doesn't need it |

**Important:** Python is process-isolated, not fully sandboxed. Logic-based escapes within the Python VM (e.g., `__subclasses__()`, frame manipulation) are not prevented. SMOKE catches bugs in agent-generated code — not adversarial code. For untrusted third-party code, use E2B or Modal.

---

## Known Limitations

| Issue | Affects | Cause |
|---|---|---|
| **Infinite loops hang the event loop** | JS/TS | Synchronous loops (`while(true){}`) block V8's event loop. The watchdog thread terminates V8 via `v8_isolate.terminate_execution()` as a fallback (`src/sandbox/js.rs:38-56`). |
| **Resource exhaustion before rlimit** | Python | Allocating memory close to 256 MB can cause the system to swap before rlimit SIGKILL fires. |
| **seccomp is Linux-only** | Python | macOS and Windows do not apply seccomp. Only rlimits and process-group isolation are used on non-Linux (`src/sandbox/python.rs:71-79`). |
| **Snippet-only testing on large files** | JS/TS/Python | Files exceeding `max_file_lines` (default 200) are extracted to the enclosing function only (`src/hook/mod.rs:179-187`). |
| **Files over `max_file_lines_absolute`** (1000) | JS/TS/Python | Skipped entirely — written without sandbox verification. |

---

## Error Output Format

When sandbox execution fails, the hook writes to stderr and exits with code 2.

**PreToolUse (Write/Edit block):**
```
SMOKE: <trimmed stderr from sandbox>
```
→ Agent sees this as a tool call rejection.

**PostToolUse (test failure):**
```
SMOKE tests failed:
<trimmed stderr from sandbox>
```
→ Agent sees this but the file is already written.

**Hook vs CLI output:**

| Mode | Output | Exit code on pass | Exit code on fail |
|---|---|---|---|
| `hook` / `post-hook` | `eprintln!("SMOKE: ...")` | 0 | 2 |
| `test` (CLI) | JSON to stdout | 0 (always) | 0 (always) |

The `test` subcommand always prints `SandboxResult` JSON and exits 0 — it's for debugging, not flow control. The hook subcommands use exit code 2 to signal Claude Code to block the tool call.
