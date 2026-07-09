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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PythonConfig {
    /// Python interpreter to use — "python3", "python", or absolute path
    pub interpreter: String,
}

// ── Defaults ────────────────────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            limits: Limits::default(),
            languages: Languages::default(),
            python: PythonConfig::default(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_ms: 1000,
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
}

#[derive(Deserialize)]
struct PartialPythonConfig {
    interpreter: Option<String>,
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
        }
        if let Some(py) = over.python {
            if let Some(v) = py.interpreter { base.python.interpreter = v; }
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
# Hard timeout for sandbox execution (milliseconds)
timeout_ms = 1000

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

[python]
# Python interpreter to invoke. Override to "python" or an absolute path
# if python3 is not in your PATH.
interpreter = "python3"
"#;

    std::fs::write(output_path, content)?;
    println!("SMOKE: config written to {:?}", output_path);
    Ok(())
}
