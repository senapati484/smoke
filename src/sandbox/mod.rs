pub mod js;
pub mod python;

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
    /// "javascript", "typescript", or "python"
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
