//! Phase 0.5: Configuration file support
//!
//! Load order (each level merges over the previous):
//!   1. Built-in defaults (hardcoded in this file)
//!   2. ~/.config/smoke/smoke.toml  (user-level, optional)
//!   3. .smoke.toml in cwd          (project-level, optional)
//!   4. --config <path>             (explicit CLI override)
//!
//! Config load failure does NOT crash — falls back to defaults with a
//! warning to stderr.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Top-level configuration struct.
/// All timeout and limit values throughout the codebase must come from here —
/// never from hardcoded literals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub languages: Languages,
    #[serde(default)]
    pub python: PythonConfig,
    /// Soft-prompt thresholds (anti-deletion, writing-side, clean reinforcement).
    /// Tunable so projects can dial up or down how chatty SMOKE is.
    #[serde(default)]
    pub prompts: Prompts,
    /// Hook operation mode (advisor/strict/silent).
    #[serde(default)]
    pub hook: HookConfig,
    /// Loop and repeated-failure detection settings.
    #[serde(default)]
    pub loop_detection: LoopDetectionConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HookMode {
    Advisor,
    Strict,
    Silent,
}

impl Default for HookMode {
    fn default() -> Self {
        HookMode::Advisor
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HookConfig {
    #[serde(default)]
    pub mode: HookMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopDetectionConfig {
    pub enabled: bool,
    pub warn_threshold: u32,
    pub escalate_threshold: u32,
    pub fingerprint_window_minutes: u64,
    pub state_retention_hours: u64,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            warn_threshold: 2,
            escalate_threshold: 3,
            fingerprint_window_minutes: 30,
            state_retention_hours: 24,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    /// Hard timeout for sandbox execution in milliseconds
    pub timeout_ms: u64,
    /// Files with more lines than this use snippet-only execution in Phase 6
    pub max_file_lines: usize,
    /// Memory limit for Python child process (MB)
    pub memory_limit_mb: u64,
    /// Files larger than this (lines) are skipped entirely — allow through
    pub max_file_lines_absolute: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Languages {
    pub js_enabled: bool,
    pub ts_enabled: bool,
    pub python_enabled: bool,
    pub rust_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonConfig {
    /// Python interpreter to use — "python3", "python", or absolute path
    pub interpreter: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompts {
    /// Fire the anti-deletion prompt when an Edit removes at least this many lines.
    /// Set to 0 to never fire on raw line count.
    pub deletion_lines_threshold: usize,
    /// Fire the anti-deletion prompt when an Edit removes at least this percent
    /// of the file. Set to 101 to never fire on percent.
    pub deletion_percent_threshold: usize,
    /// Fire the writing-side stdlib hint when the new code adds at least this
    /// many lines. Set to 0 to never fire.
    pub writing_size_threshold: usize,
    /// Fire the positive "clean" reinforcement when the resulting file has
    /// fewer than this many lines. Set to 0 to never fire.
    pub clean_file_line_threshold: usize,
    /// Fire the positive "clean" reinforcement only when the Edit/Write adds
    /// at most this many lines. Larger additions are not praised (they may
    /// belong to one of the other prompts anyway).
    pub clean_max_added_lines: usize,
}

// ── Defaults ────────────────────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            languages: Languages::default(),
            python: PythonConfig::default(),
            prompts: Prompts::default(),
            hook: HookConfig::default(),
            loop_detection: LoopDetectionConfig::default(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // Default 2 seconds. JS/TS warms up in 20-50ms; Python spawns
            // in 16-46ms. Rust (cargo check) can take 30+ seconds, so the
            // default is a compromise: fast for most languages, but the
            // caller can override via the hook's own timeout. Use the rust
            // path's cache to avoid re-running on unchanged files.
            timeout_ms: 2000,
            max_file_lines: 200,
            memory_limit_mb: 256,
            max_file_lines_absolute: 1000,
        }
    }
}

impl Default for Languages {
    fn default() -> Self {
        Self {
            js_enabled: true,
            ts_enabled: true,
            python_enabled: true,
            rust_enabled: true,
        }
    }
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            interpreter: "python3".to_string(),
        }
    }
}

impl Default for Prompts {
    fn default() -> Self {
        // Defaults match the constants previously hardcoded in the hook.
        // Projects that want SMOKE to be quieter can raise these; projects
        // that want more aggressive coaching can lower them.
        Self {
            deletion_lines_threshold: 50,
            deletion_percent_threshold: 30,
            writing_size_threshold: 100,
            clean_file_line_threshold: 50,
            clean_max_added_lines: 30,
        }
    }
}

// ── Loading ──────────────────────────────────────────────────────────────────

impl Config {
    /// Load configuration following the merge order.
    /// Falls back to defaults on any error — never panics.
    pub fn load(explicit_path: Option<&Path>) -> Self {
        let mut config = Config::default();

        // Layer 2: user-level config
        if let Some(user_path) = user_config_path() {
            if user_path.exists() {
                config = merge(config, load_file(&user_path));
            }
        }

        // Layer 3: project-level config
        let project_path = std::env::current_dir()
            .unwrap_or_default()
            .join(".smoke.toml");
        if project_path.exists() {
            config = merge(config, load_file(&project_path));
        }

        // Layer 4: explicit --config path
        if let Some(path) = explicit_path {
            config = merge(config, load_file(path));
        }

        config
    }
}

#[derive(Deserialize)]
struct PartialConfig {
    limits: Option<PartialLimits>,
    languages: Option<PartialLanguages>,
    python: Option<PartialPythonConfig>,
    prompts: Option<PartialPrompts>,
    hook: Option<PartialHookConfig>,
    loop_detection: Option<PartialLoopDetectionConfig>,
}

#[derive(Deserialize)]
struct PartialHookConfig {
    mode: Option<HookMode>,
}

#[derive(Deserialize)]
struct PartialLoopDetectionConfig {
    enabled: Option<bool>,
    warn_threshold: Option<u32>,
    escalate_threshold: Option<u32>,
    fingerprint_window_minutes: Option<u64>,
    state_retention_hours: Option<u64>,
}

#[derive(Deserialize)]
struct PartialLimits {
    timeout_ms: Option<u64>,
    max_file_lines: Option<usize>,
    memory_limit_mb: Option<u64>,
    max_file_lines_absolute: Option<usize>,
}

#[derive(Deserialize)]
struct PartialLanguages {
    js_enabled: Option<bool>,
    ts_enabled: Option<bool>,
    python_enabled: Option<bool>,
    rust_enabled: Option<bool>,
}

#[derive(Deserialize)]
struct PartialPythonConfig {
    interpreter: Option<String>,
}

#[derive(Deserialize)]
struct PartialPrompts {
    deletion_lines_threshold: Option<usize>,
    deletion_percent_threshold: Option<usize>,
    writing_size_threshold: Option<usize>,
    clean_file_line_threshold: Option<usize>,
    clean_max_added_lines: Option<usize>,
}

fn load_file(path: &Path) -> Option<PartialConfig> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| {
            eprintln!("SMOKE: warning — could not read config {:?}: {}", path, e);
        })
        .ok()?;

    toml::from_str::<PartialConfig>(&content)
        .map_err(|e| {
            eprintln!("SMOKE: warning — invalid config {:?}: {}", path, e);
        })
        .ok()
}

fn merge(mut base: Config, overlay: Option<PartialConfig>) -> Config {
    if let Some(over) = overlay {
        if let Some(limits) = over.limits {
            if let Some(v) = limits.timeout_ms { base.limits.timeout_ms = v; }
            if let Some(v) = limits.max_file_lines { base.limits.max_file_lines = v; }
            if let Some(v) = limits.memory_limit_mb { base.limits.memory_limit_mb = v; }
            if let Some(v) = limits.max_file_lines_absolute { base.limits.max_file_lines_absolute = v; }
        }
        if let Some(langs) = over.languages {
            if let Some(v) = langs.js_enabled { base.languages.js_enabled = v; }
            if let Some(v) = langs.ts_enabled { base.languages.ts_enabled = v; }
            if let Some(v) = langs.python_enabled { base.languages.python_enabled = v; }
            if let Some(v) = langs.rust_enabled { base.languages.rust_enabled = v; }
        }
        if let Some(py) = over.python {
            if let Some(v) = py.interpreter { base.python.interpreter = v; }
        }
        if let Some(prompts) = over.prompts {
            if let Some(v) = prompts.deletion_lines_threshold { base.prompts.deletion_lines_threshold = v; }
            if let Some(v) = prompts.deletion_percent_threshold { base.prompts.deletion_percent_threshold = v; }
            if let Some(v) = prompts.writing_size_threshold { base.prompts.writing_size_threshold = v; }
            if let Some(v) = prompts.clean_file_line_threshold { base.prompts.clean_file_line_threshold = v; }
            if let Some(v) = prompts.clean_max_added_lines { base.prompts.clean_max_added_lines = v; }
        }
        if let Some(hook) = over.hook {
            if let Some(v) = hook.mode { base.hook.mode = v; }
        }
        if let Some(loop_det) = over.loop_detection {
            if let Some(v) = loop_det.enabled { base.loop_detection.enabled = v; }
            if let Some(v) = loop_det.warn_threshold { base.loop_detection.warn_threshold = v; }
            if let Some(v) = loop_det.escalate_threshold { base.loop_detection.escalate_threshold = v; }
            if let Some(v) = loop_det.fingerprint_window_minutes { base.loop_detection.fingerprint_window_minutes = v; }
            if let Some(v) = loop_det.state_retention_hours { base.loop_detection.state_retention_hours = v; }
        }
    }
    base
}

fn user_config_path() -> Option<PathBuf> {
    dirs_path().map(|d| d.join("smoke").join("smoke.toml"))
}

fn dirs_path() -> Option<PathBuf> {
    // ~/.config on Linux/macOS, %APPDATA% on Windows
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config"))
        })
}

// ── Config init command ───────────────────────────────────────────────────────

/// `smoke config init` — writes a smoke.toml with defaults and comments
pub fn write_default_config(output_path: &Path) -> Result<()> {
    let content = r#"# smoke.toml — SMOKE configuration file
# Generated by: smoke config init
# Place this file in your project root (.smoke.toml) or at
# ~/.config/smoke/smoke.toml for user-level defaults.

[limits]
# Hard timeout for sandbox execution (milliseconds).
# Default 2000ms — fits JS/TS warm path (20-50ms) and Python spawn (16-46ms).
# Rust (cargo check) on a non-trivial workspace can take 30+ seconds; the
# cache in ~/.smoke/cache/rust-checks.json avoids re-running for unchanged
# files, so this timeout only matters on the first check of a new file.
timeout_ms = 2000

# Files with more lines than this use snippet-only execution (Phase 6)
max_file_lines = 200

# Memory limit for Python child process (MB)
memory_limit_mb = 256

# Files larger than this (lines) are skipped entirely — allowed through
max_file_lines_absolute = 1000

[languages]
# Enable or disable each language sandbox
js_enabled = true
ts_enabled = true
python_enabled = true
rust_enabled = true

[python]
# Python interpreter to invoke. Override to "python" or an absolute path
# if python3 is not in your PATH.
interpreter = "python3"

[prompts]
# Soft-prompt thresholds. SMOKE delivers these via Claude Code's
# additionalContext field — they appear in the model's context as system
# reminders, never as blocks. Tune these to make SMOKE quieter (higher
# thresholds) or more aggressive (lower thresholds).
#
# Anti-deletion prompt: fires when an Edit removes at least N lines OR
# at least P percent of the file (whichever matches first). Set lines
# to 0 to disable the line-count check, or percent to 101 to disable
# the percent check.
deletion_lines_threshold   = 50
deletion_percent_threshold = 30

# Writing-side stdlib hint: fires when the Edit/Write adds at least N
# lines AND the new code matches a known "roll-your-own" pattern
# (custom debounce, deep-clone via JSON.parse/stringify, hand-rolled
# UUID, manual chunking, etc.). Set to 0 to disable.
writing_size_threshold = 100

# Positive "clean" reinforcement: fires when the resulting file is
# small (< clean_file_line_threshold) AND the Edit/Write added few
# lines (≤ clean_max_added_lines) AND no other soft prompts fired.
# Set either to 0 to disable.
clean_file_line_threshold = 50
clean_max_added_lines     = 30

[hook]
# Hook operation mode:
#   "advisor" — never block writes; surface all warnings / errors as system messages (default)
#   "strict"  — block writes on syntax/execution errors for standalone runnable files
#   "silent"  — allow all writes silently, with no warnings or messages
mode = "advisor"

[loop_detection]
# Loop / repeated-failure detection settings:
#   enabled — flag to enable/disable repeated failure monitoring (default true)
#   warn_threshold — attempt count showing a warning alert (default 2)
#   escalate_threshold — attempt count triggering forced strategy prompt (default 3)
#   fingerprint_window_minutes — failure sequence memory window in minutes (default 30)
#   state_retention_hours — time to retain session cache files (default 24)
enabled = true
warn_threshold = 2
escalate_threshold = 3
fingerprint_window_minutes = 30
state_retention_hours = 24
"#;

    std::fs::write(output_path, content)?;
    println!("SMOKE: config written to {:?}", output_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: Config::load(None) with no .smoke.toml on disk
    /// must return defaults without panicking or printing warnings.
    #[test]
    fn load_with_no_config_file_returns_defaults_silently() {
        let cfg = Config::load(None);
        // Core limit defaults are always present regardless of project overrides
        assert_eq!(cfg.limits.timeout_ms, 2000);
        assert!(cfg.languages.js_enabled);
        assert!(cfg.languages.ts_enabled);
        assert!(cfg.languages.python_enabled);
        assert_eq!(cfg.python.interpreter, "python3");
        // No panic reached = fix working correctly
    }

    /// Regression: passing a non-existent explicit path must not panic
    /// (falls back to defaults with a warning). This is the old hook behaviour
    /// that the .exists() guard now avoids in production.
    #[test]
    fn load_with_nonexistent_explicit_path_does_not_panic() {
        let bogus = std::path::Path::new("/tmp/no_such_smoke_config_xyz.toml");
        assert!(!bogus.exists(), "test assumes file is absent");
        let cfg = Config::load(Some(bogus));
        assert_eq!(cfg.limits.timeout_ms, 2000); // still default
    }

    #[test]
    fn test_hook_mode_config() {
        let cfg = Config::load(None);
        assert_eq!(cfg.hook.mode, HookMode::Advisor);

        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join(".smoke.toml");
        std::fs::write(&config_path, "[hook]\nmode = \"strict\"").unwrap();
        let cfg2 = Config::load(Some(&config_path));
        assert_eq!(cfg2.hook.mode, HookMode::Strict);
    }
}
