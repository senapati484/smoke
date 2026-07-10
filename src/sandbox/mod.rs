pub mod js;
pub mod python;
pub mod rust;

use serde::{Deserialize, Serialize};
use rmcp::schemars;
use schemars::JsonSchema;

/// Shared result type returned by all sandboxes.
/// JS and Python sandboxes return this same struct — the fields are identical,
/// the isolation mechanisms are not. Never unify the sandbox implementations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxResult {
    /// true = code ran without exceptions or timeout
    pub passed: bool,
    /// captured stdout (console.log for JS, print() for Python)
    pub stdout: String,
    /// error message if passed=false, or stderr output
    pub stderr: String,
    /// wall-clock time from sandbox entry to exit
    pub execution_time_ms: u64,
    /// "javascript", "typescript", "python", or "rust"
    pub language: String,
}

impl SandboxResult {
    pub fn error(language: impl Into<String>, stderr: impl Into<String>, elapsed_ms: u64) -> Self {
        Self {
            passed: false,
            stdout: String::new(),
            stderr: stderr.into(),
            execution_time_ms: elapsed_ms,
            language: language.into(),
        }
    }

    #[allow(dead_code)]
    pub fn success(
        language: impl Into<String>,
        stdout: impl Into<String>,
        elapsed_ms: u64,
    ) -> Self {
        Self {
            passed: true,
            stdout: stdout.into(),
            stderr: String::new(),
            execution_time_ms: elapsed_ms,
            language: language.into(),
        }
    }
}

/// Write a message directly to the controlling terminal (/dev/tty or CONOUT$),
/// bypassing any stdout/stderr redirections. Falls back to stderr if the TTY is not available.
pub fn print_to_terminal(msg: &str) {
    let mut written = false;

    #[cfg(unix)]
    {
        if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            use std::io::Write;
            if writeln!(file, "{}", msg).is_ok() {
                written = true;
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(mut file) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            use std::io::Write;
            if writeln!(file, "{}", msg).is_ok() {
                written = true;
            }
        }
    }

    if !written {
        eprintln!("{}", msg);
    }
}
