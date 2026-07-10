// Phase 4.5: PostToolUse hook handler
// Fires AFTER a successful Write/Edit. Looks for a co-located test file
// and runs it through the sandbox. Reuses JsSandbox and PythonSandbox directly.

use crate::config::Config;
use crate::sandbox::js::JsSandbox;
use crate::sandbox::python::PythonSandbox;
use serde::Deserialize;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct PostHookInput {
    #[allow(dead_code)]
    session_id: String,
    hook_event_name: String,
    tool_name: String,
    tool_input: serde_json::Value,
    tool_response: ToolResponse,
}

#[derive(Debug, Deserialize)]
struct ToolResponse {
    success: bool,
}

pub async fn run() -> anyhow::Result<()> {
    // 1. Read stdin
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    // 2. Parse input JSON
    let input: PostHookInput = match serde_json::from_str(&buffer) {
        Ok(v) => v,
        Err(_) => std::process::exit(0),
    };

    // 3. Verify event and success state
    if input.hook_event_name != "PostToolUse" || !input.tool_response.success {
        std::process::exit(0);
    }

    if input.tool_name != "Write" && input.tool_name != "Edit" {
        std::process::exit(0);
    }

    // 4. Extract file path
    let file_path = input.tool_input.get("file_path").and_then(|v| v.as_str());
    let file_path = match file_path {
        Some(p) => p,
        None => std::process::exit(0),
    };

    // 5. Look for co-located test file
    let path = Path::new(file_path);
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    let mut test_path: Option<PathBuf> = None;

    match ext.to_lowercase().as_str() {
        "js" | "mjs" | "cjs" | "jsx" => {
            // look for stem.test.ext or stem.spec.ext
            let p1 = parent.join(format!("{}.test.{}", stem, ext));
            let p2 = parent.join(format!("{}.spec.{}", stem, ext));
            if p1.exists() {
                test_path = Some(p1);
            } else if p2.exists() {
                test_path = Some(p2);
            }
        }
        "ts" | "mts" | "cts" | "tsx" => {
            // look for stem.test.ext or __tests__/stem.ext
            let p1 = parent.join(format!("{}.test.{}", stem, ext));
            let p2 = parent.join("__tests__").join(format!("{}.{}", stem, ext));
            if p1.exists() {
                test_path = Some(p1);
            } else if p2.exists() {
                test_path = Some(p2);
            }
        }
        "py" | "pyw" => {
            // look for tests/test_stem.py or test_stem.py in same directory
            // Note: tests/test_stem.py is relative to workspace root (cwd)
            let p1 = Path::new("tests").join(format!("test_{}.{}", stem, ext));
            let p2 = parent.join(format!("test_{}.{}", stem, ext));
            if p1.exists() {
                test_path = Some(p1);
            } else if p2.exists() {
                test_path = Some(p2);
            }
        }
        "rs" | "rust" => {
            // Run cargo test inside workspace matching this file stem
            if let Some(toml_path) = find_cargo_toml(path) {
                let cargo_dir = toml_path.parent().unwrap_or(Path::new(""));
                let mut cmd = std::process::Command::new("cargo");
                cmd.arg("test")
                   .arg("--")
                   .arg(stem)
                   .current_dir(cargo_dir);
                let status = match cmd.status() {
                    Ok(s) => s,
                    Err(_) => std::process::exit(0),
                };
                if status.success() {
                    let check_msg = format!("\x1b[32m[SMOKE] Cargo tests passed for {} ✓\x1b[0m", stem);
                    crate::sandbox::print_to_terminal(&check_msg);
                    std::process::exit(0);
                } else {
                    eprintln!("SMOKE tests failed: cargo test failed for {}", stem);
                    std::process::exit(2);
                }
            } else {
                std::process::exit(0);
            }
        }
        _ => {}
    }

    let test_path = match test_path {
        Some(p) => p,
        None => std::process::exit(0), // No test file found, exit 0 silently
    };

    // 6. Read test file content
    let test_content = match std::fs::read_to_string(&test_path) {
        Ok(c) => c,
        Err(_) => std::process::exit(0),
    };

    // 7. Load config
    let cfg = Config::load(None);
    // Timeout for PostToolUse tests is 30s (30000ms)
    let timeout_ms = 30000;

    // 8. Run the sandbox
    let result = match ext.to_lowercase().as_str() {
        "js" | "mjs" | "cjs" | "jsx" => {
            let mut sandbox = JsSandbox::new()?;
            sandbox.execute(&test_content, false, timeout_ms)
        }
        "ts" | "mts" | "cts" | "tsx" => {
            let mut sandbox = JsSandbox::new()?;
            sandbox.execute(&test_content, true, timeout_ms)
        }
        "py" | "pyw" => {
            let mut sandbox = PythonSandbox::new();
            sandbox.execute(&test_content, &cfg.python.interpreter, timeout_ms).await
        }
        _ => std::process::exit(0),
    };

    // 9. Evaluate test outcome
    if result.passed {
        let test_name = test_path.file_name().and_then(|f| f.to_str()).unwrap_or(stem);
        let check_msg = format!("\x1b[32m[SMOKE] Tests passed: {} ({}ms) ✓\x1b[0m", test_name, result.execution_time_ms);
        crate::sandbox::print_to_terminal(&check_msg);
        std::process::exit(0);
    } else {
        eprintln!("SMOKE tests failed:\n{}", result.stderr.trim());
        std::process::exit(2);
    }
}

fn find_cargo_toml(start_path: &Path) -> Option<PathBuf> {
    let mut current = if start_path.is_file() {
        start_path.parent()?
    } else {
        start_path
    };

    loop {
        let candidate = current.join("Cargo.toml");
        if candidate.exists() {
            return Some(candidate);
        }
        if let Some(parent) = current.parent() {
            current = parent;
        } else {
            break;
        }
    }
    None
}
