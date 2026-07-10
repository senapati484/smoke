// Phase 3: PreToolUse hook handler
// Reads Claude Code's PreToolUse JSON from stdin, runs the appropriate sandbox,
// and writes the allow/block decision to stdout/stderr with the correct exit code.

use crate::config::Config;
use crate::sandbox::js::JsSandbox;
use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::Path;

#[derive(Debug, Deserialize)]
struct HookInput {
    #[allow(dead_code)]
    session_id: String,
    #[allow(dead_code)]
    transcript_path: String,
    cwd: String,
    hook_event_name: String,
    tool_name: String,
    tool_input: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct HookOutput {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificOutput,
}

#[derive(Debug, Serialize)]
struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    hook_event_name: String,
    #[serde(rename = "permissionDecision")]
    permission_decision: String,
    #[serde(rename = "permissionDecisionReason")]
    permission_decision_reason: String,
    #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
    updated_input: Option<serde_json::Value>,
}

enum Language {
    JavaScript,
    TypeScript,
    Python,
    Rust,
}

pub async fn run() -> anyhow::Result<()> {
    // 1. Read stdin
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    // 2. Parse input JSON
    let input: HookInput = match serde_json::from_str(&buffer) {
        Ok(v) => v,
        Err(e) => {
            // If the input is not valid JSON, print to stderr and exit 0 (don't block the user's tool calls on parser failure)
            eprintln!("SMOKE: failed to parse stdin JSON: {}", e);
            std::process::exit(0);
        }
    };

    // 3. Ensure this is a PreToolUse hook
    if input.hook_event_name != "PreToolUse" {
        std::process::exit(0);
    }

    // 4. Decision logic: only verify Write and Edit tools
    if input.tool_name != "Write" && input.tool_name != "Edit" {
        std::process::exit(0);
    }

    // 5. Extract file path
    let file_path = match input.tool_name.as_str() {
        "Write" => input.tool_input.get("file_path").and_then(|v| v.as_str()),
        "Edit" => input.tool_input.get("file_path").and_then(|v| v.as_str()),
        _ => None,
    };

    let file_path = match file_path {
        Some(path) => path,
        None => {
            // No file path found in tool input, skip validation
            std::process::exit(0);
        }
    };

    // 6. Detect language by extension
    let ext = Path::new(file_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let lang = match ext.as_str() {
        "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
        "ts" | "mts" | "cts" | "tsx" => Some(Language::TypeScript),
        "py" | "pyw" => Some(Language::Python),
        "rs" | "rust" => Some(Language::Rust),
        _ => None,
    };

    // 7. Handle skipped/unsupported extension
    let lang = match lang {
        Some(l) => l,
        None => {
            let output = HookOutput {
                hook_specific_output: HookSpecificOutput {
                    hook_event_name: "PreToolUse".to_string(),
                    permission_decision: "allow".to_string(),
                    permission_decision_reason: format!("SMOKE: no sandbox for .{} — skipped", ext),
                    updated_input: None,
                },
            };
            println!("{}", serde_json::to_string(&output)?);
            std::process::exit(0);
        }
    };

    // 8. Load config
    let project_config_path = Path::new(&input.cwd).join(".smoke.toml");
    let cfg = Config::load(Some(&project_config_path));

    // 9. Extract or reconstruct code content to execute
    let mut code_content = String::new();
    let mut is_snippet = false;

    let lang_id = if ext == "tsx" {
        "tsx"
    } else {
        match lang {
            Language::JavaScript => "js",
            Language::TypeScript => "ts",
            Language::Python => "py",
            Language::Rust => "rs",
        }
    };

    match input.tool_name.as_str() {
        "Write" => {
            code_content = input.tool_input.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // Fast syntax check on Write content
            if let Some(err_msg) = crate::parser::check_syntax(&code_content, lang_id) {
                eprintln!("SMOKE: {}", err_msg);
                std::process::exit(2);
            }
        }
        "Edit" => {
            let old_str = input.tool_input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
            let new_str = input.tool_input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");

            // Reconstruct patched file content
            let full_path = Path::new(&input.cwd).join(file_path);
            let file_content = if full_path.exists() {
                match std::fs::read_to_string(&full_path) {
                    Ok(s) => s,
                    Err(e) => {
                        allow_with_reason(&format!("SMOKE: failed to read file — skipped verification ({})", e));
                    }
                }
            } else {
                // Defensive: file doesn't exist yet, treat as Write
                new_str.to_string()
            };

            let line_count = file_content.lines().count();
            if line_count > 1000 {
                allow_with_reason("SMOKE: file > 1000 lines — skipped verification");
            }

            let idx = match file_content.find(old_str) {
                Some(i) => i,
                None => {
                    allow_with_reason("SMOKE: could not apply edit — skipped verification");
                }
            };

            let mut patched_content = file_content[..idx].to_string();
            patched_content.push_str(new_str);
            patched_content.push_str(&file_content[idx + old_str.len()..]);

            // Fast syntax check on the patched result first
            if let Some(err_msg) = crate::parser::check_syntax(&patched_content, lang_id) {
                eprintln!("SMOKE: {}", err_msg);
                std::process::exit(2);
            }

            if line_count > cfg.limits.max_file_lines {
                // Try snippet extraction
                if let Some(snippet) = crate::parser::extract_enclosing_function(&patched_content, idx, lang_id) {
                    code_content = snippet;
                    is_snippet = true;
                } else {
                    // Fallback to full file if <= 1000 lines
                    code_content = patched_content;
                }
            } else {
                code_content = patched_content;
            }
        }
        _ => {}
    }

    // 10. Execute the sandbox
    let result = match lang {
        Language::JavaScript => {
            if !cfg.languages.js_enabled {
                allow_with_reason("SMOKE: JS sandbox is disabled in config");
            }
            let mut sandbox = JsSandbox::new()?;
            sandbox.execute(&code_content, false, cfg.limits.timeout_ms)
        }
        Language::TypeScript => {
            if !cfg.languages.ts_enabled {
                allow_with_reason("SMOKE: TS sandbox is disabled in config");
            }
            let mut sandbox = JsSandbox::new()?;
            sandbox.execute(&code_content, true, cfg.limits.timeout_ms)
        }
        Language::Python => {
            if !cfg.languages.python_enabled {
                allow_with_reason("SMOKE: Python sandbox is disabled in config");
            }
            let mut sandbox = crate::sandbox::python::PythonSandbox::new();
            sandbox.execute(&code_content, &cfg.python.interpreter, cfg.limits.timeout_ms).await
        }
        Language::Rust => {
            if !cfg.languages.rust_enabled {
                allow_with_reason("SMOKE: Rust sandbox is disabled in config");
            }
            let mut sandbox = crate::sandbox::rust::RustSandbox::new();
            sandbox.execute(&code_content, Some(file_path), &input.cwd, cfg.limits.timeout_ms).await
        }
    };

    // 11. Evaluate sandbox result
    if result.passed {
        let reason = if is_snippet {
            "SMOKE: large file — snippet only".to_string()
        } else {
            format!("SMOKE: executed clean in {}ms", result.execution_time_ms)
        };
        let output = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: reason,
                updated_input: None,
            },
        };
        println!("{}", serde_json::to_string(&output)?);
        std::process::exit(0);
    } else {
        // Exit code 2 blocks the tool call in Claude Code, showing stderr to the agent
        eprintln!("SMOKE: {}", result.stderr.trim());
        std::process::exit(2);
    }
}

fn allow_with_reason(reason: &str) -> ! {
    let output = HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PreToolUse".to_string(),
            permission_decision: "allow".to_string(),
            permission_decision_reason: reason.to_string(),
            updated_input: None,
        },
    };
    if let Ok(json) = serde_json::to_string(&output) {
        println!("{}", json);
    }
    std::process::exit(0);
}
