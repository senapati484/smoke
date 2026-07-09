# SMOKE Configuration Reference

> Source: `src/config/mod.rs`, `src/main.rs`, `src/hook/mod.rs`

---

## 1. Config File Locations (4-Layer Merge)

SMOKE merges configuration from up to four layers. Each subsequent layer overrides fields set by the previous one — unspecified fields retain values from the layer below.

| Layer | Source | Path Resolution | Optional? |
|-------|--------|----------------|-----------|
| 1 | Built-in defaults | Hardcoded in `src/config/mod.rs` (`impl Default`) | Always present |
| 2 | User-level config | `$XDG_CONFIG_HOME/smoke/smoke.toml` — or `$HOME/.config/smoke/smoke.toml` if `XDG_CONFIG_HOME` is unset | Yes |
| 3 | Project-level config | `$CWD/.smoke.toml` | Yes |
| 4 | CLI override | `--config <path>` flag | Yes |

**Resolution logic** (`Config::load` at `src/config/mod.rs:100`):

- Starts with `Config::default()` (layer 1).
- If `~/.config/smoke/smoke.toml` exists, merges it (layer 2).
- If `$CWD/.smoke.toml` exists, merges it (layer 3).
- If `--config <path>` was passed, merges that file (layer 4).

> **Hook-specific note** (`src/hook/mod.rs:117–118`): When running as a PreToolUse hook, layer 3 resolution uses `$CWD/.smoke.toml` where `$CWD` is the `cwd` field from Claude Code's hook JSON — i.e. the project root of the repository being edited.

**Failure handling**: If a config file cannot be read or parsed, SMOKE prints a warning to stderr (prefixed `SMOKE: warning —`) and continues to the next layer. It **never panics** on config errors.

---

## 2. Complete Config TOML Reference

| Section | Field | Type | Default | Description |
|---------|-------|------|---------|-------------|
| `[limits]` | `timeout_ms` | `u64` | `1000` | Hard timeout for sandbox execution in milliseconds |
| `[limits]` | `max_file_lines` | `usize` | `200` | Files with more lines than this use snippet-only execution (Edit tool) |
| `[limits]` | `memory_limit_mb` | `u64` | `256` | Memory limit for Python child process in MB |
| `[limits]` | `max_file_lines_absolute` | `usize` | `1000` | Files larger than this (lines) are skipped entirely |
| `[languages]` | `js_enabled` | `bool` | `true` | Enable/disable JavaScript sandbox |
| `[languages]` | `ts_enabled` | `bool` | `true` | Enable/disable TypeScript sandbox |
| `[languages]` | `python_enabled` | `bool` | `true` | Enable/disable Python sandbox |
| `[python]` | `interpreter` | `string` | `"python3"` | Python interpreter path (`"python3"`, `"python"`, or absolute path) |

### Default TOML (as emitted by `smoke config init`)

```toml
[limits]
timeout_ms = 1000
max_file_lines = 200
memory_limit_mb = 256
max_file_lines_absolute = 1000

[languages]
js_enabled = true
ts_enabled = true
python_enabled = true

[python]
interpreter = "python3"
```

---

## 3. Config Struct Details

All defined in `src/config/mod.rs`.

### `Config` (top-level)

```rust
pub struct Config {
    pub limits: Limits,
    pub languages: Languages,
    pub python: PythonConfig,
}
```

All fields are `#[serde(default)]` — any section may be omitted from TOML.

### `Limits`

```rust
pub struct Limits {
    /// Hard timeout for sandbox execution in milliseconds
    pub timeout_ms: u64,          // default: 1000
    /// Files with more lines than this use snippet-only execution in Phase 6
    pub max_file_lines: usize,    // default: 200
    /// Memory limit for Python child process (MB)
    pub memory_limit_mb: u64,     // default: 256
    /// Files larger than this (lines) are skipped entirely — allow through
    pub max_file_lines_absolute: usize, // default: 1000
}
```

**How these are used** (`src/hook/mod.rs`):

- `timeout_ms`: Passed to `JsSandbox::execute()` and `PythonSandbox::execute()` — the sandbox is killed if execution exceeds this duration.
- `max_file_lines`: For `Edit` tool invocations, if the patched file exceeds this line count, SMOKE attempts snippet extraction via `parser::extract_enclosing_function()`. If extraction fails, the full patched content is used instead (fallback).
- `max_file_lines_absolute`: <!-- VERIFY: Not yet referenced in hook/mod.rs — declared in config struct for future use (hard ceiling for file processing). See src/config/mod.rs:37-38. -->
- `memory_limit_mb`: <!-- VERIFY: Not yet referenced in hook/mod.rs — declared in config struct for future Python subprocess rlimit enforcement. See src/config/mod.rs:35-36. -->

### `Languages`

```rust
pub struct Languages {
    pub js_enabled: bool,        // default: true
    pub ts_enabled: bool,        // default: true
    pub python_enabled: bool,    // default: true
}
```

Gated at `src/hook/mod.rs:198-216` — if a language is disabled, the hook prints `allow` with reason `"SMOKE: <lang> sandbox is disabled in config"` and does not execute the sandbox.

### `PythonConfig`

```rust
pub struct PythonConfig {
    /// Python interpreter to use — "python3", "python", or absolute path
    pub interpreter: String,     // default: "python3"
}
```

Used at `src/hook/mod.rs:216`: `sandbox.execute(&code_content, &cfg.python.interpreter, cfg.limits.timeout_ms).await`

---

## 4. How Config Is Loaded

### The `PartialConfig` Merge Pattern

Instead of deserializing directly into `Config` (which would require all fields to be present in the file), SMOKE uses a set of "partial" structs where every field is `Option<T>`:

```rust
struct PartialConfig {
    limits: Option<PartialLimits>,
    languages: Option<PartialLanguages>,
    python: Option<PartialPythonConfig>,
}
```

This means a TOML file can specify **any subset of fields**. The `merge()` function (`src/config/mod.rs:168`) iterates over each `Option` — if `Some`, it copies the value; if `None`, the existing default is preserved.

### Load Failure Handling

`load_file()` (`src/config/mod.rs:154`):

1. Attempts `std::fs::read_to_string(path)` — on failure, prints warning to stderr and returns `None`.
2. Attempts `toml::from_str::<PartialConfig>(&content)` — on parse failure, prints warning to stderr and returns `None`.
3. On success, returns `Some(PartialConfig)`.

A `None` return from `load_file()` causes `merge()` to return the base config unchanged — **no crash, no data loss**.

### Merge Sequence Illustration

```text
Config::default()
  → merge with ~/.config/smoke/smoke.toml (if exists)
    → merge with .smoke.toml (if exists)
      → merge with --config <path> (if provided)
        → final Config
```

---

## 5. `smoke config init`

Writes a `.smoke.toml` file with all default values and inline comments to the current working directory.

```
smoke config init
```

Output: `SMOKE: config written to "<CWD>/.smoke.toml"`

The generated file contains every field with its default value and a comment explaining its purpose. Users can then edit the file and commit it to their project repository.

**Implementation** (`src/config/mod.rs:203-238`): Uses `std::fs::write()` with a hardcoded template string. The file path is always `$CWD/.smoke.toml` — there is no `--output` flag.

---

## 6. `smoke config show`

Prints the **fully merged active configuration** as TOML to stdout. All four merge layers are applied (the hook-relevant project-level `.smoke.toml` is resolved against the SMOKE binary's CWD).

```
smoke config show
```

Example output:

```toml
[limits]
timeout_ms = 1000
max_file_lines = 200
memory_limit_mb = 256
max_file_lines_absolute = 1000

[languages]
js_enabled = true
ts_enabled = true
python_enabled = true

[python]
interpreter = "python3"
```

**Implementation** (`src/main.rs:139-143`): Calls `Config::load(None)` (no explicit `--config` override path) and serializes the result via `toml::to_string_pretty`. Note: this still loads layers 2 and 3 (user-level and CWD `.smoke.toml`).

---

## 7. Per-Language Sandbox Control

Each language sandbox has a dedicated enable/disable toggle:

```toml
[languages]
js_enabled = true
ts_enabled = true
python_enabled = true
```

When a language is **disabled**, SMOKE's PreToolUse hook still returns a decision — it **allows** the write with reason `"SMOKE: <Lang> sandbox is disabled in config"`. The sandbox is never constructed or executed.

This is useful for:

- Disabling Python sandbox if `python3` is not available in the agent's environment.
- Disabling JavaScript/TypeScript when only Python code is being written.
- Selective sandboxing in mixed-language monorepos.

CLI `smoke test` also respects these flags (`src/main.rs:104,111,121`) — it bails with an error if the requested language is disabled.

**Language detection** (`src/hook/mod.rs:92-96`):

| Extension(s) | Language |
|--------------|----------|
| `js`, `mjs`, `cjs`, `jsx` | JavaScript |
| `ts`, `mts`, `cts`, `tsx` | TypeScript |
| `py`, `pyw` | Python |

---

## 8. Timeout and Limit Tuning Guidance

### `timeout_ms` (default: `1000`)

Controls the maximum wall-clock time a sandboxed execution may run before being forcibly terminated.

- **Too low**: Legitimate code that performs computation, network calls, or I/O may be killed before producing output, causing Claude Code to see a "blocked" decision.
- **Too high**: A buggy/recursive snippet can hang the agent loop for extended periods.
- **Recommended range**: `500` (fast feedback) to `5000` (lenient). Start at `1000` and raise if you see repeated timeout-related blocks.

### `max_file_lines` (default: `200`)

Applies only to the **Edit** tool (file patching). When the patched file exceeds this line count, SMOKE attempts to extract just the enclosing function around the edit region for faster execution.

- **Lower value** (e.g., `50`): More aggressive snippet extraction; faster runs but may fail if the parser cannot identify an enclosing function.
- **Higher value** (e.g., `500`): More full-file execution; more thorough but slower.
- Set to `0` to always use snippet extraction (if possible).
- This limit is **not** a hard block — if snippet extraction fails, the full patched content is still executed (fallback).

### `memory_limit_mb` (default: `256`)

Reserved for Python child process memory limiting via `rlimit`. <!-- VERIFY: Memory enforcement via rlimit is declared in the config struct but not yet wired into PythonSandbox — check src/sandbox/python.rs for current status. -->

### `max_file_lines_absolute` (default: `1000`)

Hard ceiling for file processing. <!-- VERIFY: Declared in config struct as a general hard ceiling but not yet referenced in hook execution — check src/hook/mod.rs for current usage. -->

- A separate inline check in the hook (`src/hook/mod.rs:158`) skips files >1000 lines regardless of this config value when processing Edit tool invocations.

### Platform-Specific Notes

- **Python memory limits**: `rlimit` works on Unix/macOS/Linux but not on Windows. The `memory_limit_mb` field is parsed on all platforms, but enforcement depends on platform support.
- **Node/Python availability**: SMOKE does not bundle interpreters. JavaScript execution requires Deno runtime (embedded via `rustyscript`/`deno_core`). Python execution requires `python3` (or the configured interpreter) to be on `PATH`.

### Quick-Reference: Performance Profile

| Setting | Low-latency (tight loop) | High-verification (safety) |
|---------|--------------------------|----------------------------|
| `timeout_ms` | `500` | `3000` |
| `max_file_lines` | `100` | `500` |
| `memory_limit_mb` | `128` | `512` |
| `max_file_lines_absolute` | `500` | `2000` |
