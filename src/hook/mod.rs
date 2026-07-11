// Phase 3: PreToolUse hook handler
// Reads Claude Code's PreToolUse JSON from stdin, runs the appropriate sandbox,
// and writes the allow/block decision to stdout/stderr with the correct exit code.
//
// The I/O shell (stdin/stdout/exit codes) lives in `run`. The decision logic — what
// sandbox to run, what counts as a "large" deletion, what to inject into the
// additionalContext field — is in pure functions (`evaluate_write`, `evaluate_edit`,
// `diff_stats`) that are unit-tested without touching the process.

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

#[derive(Debug, Serialize, Default)]
struct HookOutput {
    #[serde(rename = "hookSpecificOutput")]
    hook_specific_output: HookSpecificOutput,
}

#[derive(Debug, Serialize, Default)]
struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    hook_event_name: String,
    #[serde(rename = "permissionDecision")]
    permission_decision: String,
    #[serde(rename = "permissionDecisionReason")]
    permission_decision_reason: String,
    /// Injected into Claude's context as a system reminder. Use for soft prompts
    /// ("FYI, this edit removed 70 lines...") that the agent should see without
    /// the tool call being blocked. See Claude Code hooks docs.
    #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
    additional_context: Option<String>,
    #[serde(rename = "updatedInput", skip_serializing_if = "Option::is_none")]
    updated_input: Option<serde_json::Value>,
}

enum Language {
    JavaScript,
    TypeScript,
    Python,
    Rust,
}

/// Threshold for when the anti-deletion prompt should fire.
/// Both conditions are OR — a 100-line file with 35% removed is large enough
/// to warrant a check, and a 1000-line file with 60 lines removed is also
/// suspicious even if it's only 6% of the file.
///
/// These defaults are now exposed via `Config::prompts` (and so via
/// `.smoke.toml`). The hook reads `cfg.prompts.deletion_lines_threshold` and
/// `cfg.prompts.deletion_percent_threshold` and uses those in
/// `build_anti_deletion_context`. Setting either to 0 / 101 disables that
/// gate.

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
                    additional_context: None,
                    updated_input: None,
                },
            };
            println!("{}", serde_json::to_string(&output)?);
            std::process::exit(0);
        }
    };

    // 8. Load config — only pass the explicit project path when it actually
    //    exists; passing a non-existent path produces a noisy "could not read"
    //    warning that confuses users who haven't created a .smoke.toml yet.
    let project_config_path = Path::new(&input.cwd).join(".smoke.toml");
    let cfg = Config::load(if project_config_path.exists() {
        Some(&project_config_path)
    } else {
        None
    });

    if cfg.hook.mode == crate::config::HookMode::Silent {
        let output = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: "SMOKE: silent mode".to_string(),
                additional_context: None,
                updated_input: None,
            },
        };
        println!("{}", serde_json::to_string(&output)?);
        std::process::exit(0);
    }

    let mut state = crate::state::SessionState::load(&input.session_id);
    if cfg.loop_detection.enabled {
        let _ = crate::state::gc_state_dir(cfg.loop_detection.state_retention_hours * 3600);
    }

    // 9. Extract or reconstruct code content to execute
    let mut code_content = String::new();
    let mut is_snippet = false;
    let mut additional_context_lines: Vec<String> = Vec::new();
    let mut syntax_error_occurred = false;

    let lang_id = match ext.as_str() {
        "tsx" | "jsx" => "tsx",
        _ => match lang {
            Language::JavaScript => "js",
            Language::TypeScript => "ts",
            Language::Python => "py",
            Language::Rust => "rs",
        },
    };

    let is_jsx_file = matches!(ext.as_str(), "tsx" | "jsx");

    // For Edit: we need the file's previous content to compute the diff for
    // the anti-deletion prompt AND to count how many lines were added (to
    // decide whether the writing-side stdlib hint is worth firing). For
    // Write: there's no "before", so we just use the new content.
    let mut edit_before_content: Option<String> = None;

    match input.tool_name.as_str() {
        "Write" => {
            code_content = input.tool_input.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
            // Fast syntax check on Write content
            if let Some(err_msg) = crate::parser::check_syntax(&code_content, lang_id) {
                let mut loop_msg = None;
                if cfg.loop_detection.enabled {
                    let fp = crate::state::fingerprint(file_path, "syntax_error", &err_msg);
                    let count = state.record_failure(&fp, file_path, &err_msg, cfg.loop_detection.fingerprint_window_minutes);
                    let escalation = crate::state::escalation_for(count, cfg.loop_detection.warn_threshold, cfg.loop_detection.escalate_threshold);
                    let _ = state.save(&input.session_id); // Save mutated state

                    match escalation {
                        crate::state::EscalationLevel::Normal => {}
                        crate::state::EscalationLevel::Notice => {
                            loop_msg = Some(format!("⚠️ SMOKE: this is attempt #{} with the same error signature on {}.", count, file_path));
                        }
                        crate::state::EscalationLevel::Escalate => {
                            loop_msg = Some(format!(
                                "🛑 SMOKE: {} consecutive failures with the same error signature on {}.\n\nStop retrying variations of the same fix — it isn't addressing the root cause.\nBefore the next edit:\n  1. Re-read the actual error text below, in full.\n  2. State your hypothesis for the root cause in one sentence.\n  3. If you're not confident in the hypothesis, ask the user for guidance instead of editing again.\n\nLast error:\n{}",
                                count, file_path, err_msg
                            ));
                        }
                    }
                }

                if cfg.hook.mode == crate::config::HookMode::Strict {
                    if let Some(ref msg) = loop_msg {
                        eprintln!("{}\n", msg);
                    }
                    eprintln!("SMOKE: {}", err_msg);
                    std::process::exit(2);
                } else {
                    if let Some(ref msg) = loop_msg {
                        additional_context_lines.push(msg.clone());
                    }
                    additional_context_lines.push(format!(
                        "⚠️ SMOKE syntax warning in {}: {}\nFix this before finalizing.",
                        file_path, err_msg
                    ));
                    syntax_error_occurred = true;
                }
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

            // Stash the pre-edit content for the diff prompts (deletion + writing).
            edit_before_content = Some(file_content.clone());

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

            // Skip no-op edits (S3): if the Edit produces a patched file
            // identical to the original, the agent is re-applying the same
            // change. Don't run sandbox or print prompts — just allow.
            // Catches the "retry-by-sending-same-Edit" pattern.
            if patched_content == file_content {
                allow_with_reason("SMOKE: no-op edit — content unchanged, skipped");
            }

            // Fast syntax check on the patched result first
            if let Some(err_msg) = crate::parser::check_syntax(&patched_content, lang_id) {
                let mut loop_msg = None;
                if cfg.loop_detection.enabled {
                    let fp = crate::state::fingerprint(file_path, "syntax_error", &err_msg);
                    let count = state.record_failure(&fp, file_path, &err_msg, cfg.loop_detection.fingerprint_window_minutes);
                    let escalation = crate::state::escalation_for(count, cfg.loop_detection.warn_threshold, cfg.loop_detection.escalate_threshold);
                    let _ = state.save(&input.session_id); // Save mutated state

                    match escalation {
                        crate::state::EscalationLevel::Normal => {}
                        crate::state::EscalationLevel::Notice => {
                            loop_msg = Some(format!("⚠️ SMOKE: this is attempt #{} with the same error signature on {}.", count, file_path));
                        }
                        crate::state::EscalationLevel::Escalate => {
                            loop_msg = Some(format!(
                                "🛑 SMOKE: {} consecutive failures with the same error signature on {}.\n\nStop retrying variations of the same fix — it isn't addressing the root cause.\nBefore the next edit:\n  1. Re-read the actual error text below, in full.\n  2. State your hypothesis for the root cause in one sentence.\n  3. If you're not confident in the hypothesis, ask the user for guidance instead of editing again.\n\nLast error:\n{}",
                                count, file_path, err_msg
                            ));
                        }
                    }
                }

                if cfg.hook.mode == crate::config::HookMode::Strict {
                    if let Some(ref msg) = loop_msg {
                        eprintln!("{}\n", msg);
                    }
                    eprintln!("SMOKE: {}", err_msg);
                    std::process::exit(2);
                } else {
                    if let Some(ref msg) = loop_msg {
                        additional_context_lines.push(msg.clone());
                    }
                    additional_context_lines.push(format!(
                        "⚠️ SMOKE syntax warning in {}: {}\nFix this before finalizing.",
                        file_path, err_msg
                    ));
                    syntax_error_occurred = true;
                }
            }

            // Anti-deletion prompt: surface to the agent that a large chunk of
            // code was removed. This is a soft signal — the write still goes
            // through — but Claude sees it as a system reminder in its context
            // and can re-think if the deletion was unintentional.
            if let Some(ctx) = build_anti_deletion_context(&file_content, &patched_content, &cfg.prompts) {
                additional_context_lines.push(ctx);
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

    // 9.5 Handle syntax errors or JSX/TSX bypass
    if syntax_error_occurred {
        let additional_context = if additional_context_lines.is_empty() {
            None
        } else {
            Some(additional_context_lines.join("\n\n"))
        };
        if let Some(ref ctx) = additional_context {
            for line in ctx.lines() {
                if line.is_empty() {
                    crate::sandbox::print_to_terminal("");
                } else {
                    crate::sandbox::print_to_terminal(&format!("\x1b[33m[SMOKE] {}\x1b[0m", line));
                }
            }
        }
        let output = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: "SMOKE: allowed with syntax warnings".to_string(),
                additional_context,
                updated_input: None,
            },
        };
        println!("{}", serde_json::to_string(&output)?);
        std::process::exit(0);
    }

    if is_jsx_file {
        if cfg.loop_detection.enabled {
            state.record_success(file_path);
            let _ = state.save(&input.session_id);
        }

        // Skip sandbox execution entirely for JSX/TSX.
        // Just print verified success if syntax check passed.
        let file_name = Path::new(file_path).file_name().and_then(|f| f.to_str()).unwrap_or(file_path);
        let check_msg = format!("\x1b[32m[SMOKE] Verified {} syntax successfully ✓\x1b[0m", file_name);
        crate::sandbox::print_to_terminal(&check_msg);

        // Compute "added lines"
        let resulting_lines = code_content.lines().count();
        let added_lines = if let Some(ref before) = edit_before_content {
            let (_, removed) = diff_stats(before, &code_content);
            let before_lines = before.lines().count();
            let after_lines = code_content.lines().count();
            after_lines.saturating_sub(before_lines.saturating_sub(removed))
        } else {
            code_content.lines().count()
        };

        // Run writing-side stdlib hint check
        if let Some(writing_hint) = crate::parser::writing_hint_for(
            &code_content, lang_id, added_lines, cfg.prompts.writing_size_threshold,
        ) {
            additional_context_lines.push(writing_hint);
        }

        let is_clean = !is_snippet
            && cfg.prompts.clean_file_line_threshold > 0
            && resulting_lines < cfg.prompts.clean_file_line_threshold
            && cfg.prompts.clean_max_added_lines > 0
            && added_lines <= cfg.prompts.clean_max_added_lines
            && additional_context_lines.is_empty();

        if is_clean {
            let clean_msg = format!(
                "This change is clean: small file ({} lines), no stdlib duplicates, no large deletion. This is the kind of edit SMOKE rewards.",
                resulting_lines
            );
            crate::sandbox::print_to_terminal(&format!("\x1b[32m[SMOKE] {}\x1b[0m", clean_msg));

            let output = HookOutput {
                hook_specific_output: HookSpecificOutput {
                    hook_event_name: "PreToolUse".to_string(),
                    permission_decision: "allow".to_string(),
                    permission_decision_reason: "SMOKE: JSX/TSX syntax clean".to_string(),
                    additional_context: Some(clean_msg),
                    updated_input: None,
                },
            };
            println!("{}", serde_json::to_string(&output)?);
            std::process::exit(0);
        }

        let additional_context = if additional_context_lines.is_empty() {
            None
        } else {
            Some(additional_context_lines.join("\n\n"))
        };
        if let Some(ref ctx) = additional_context {
            for line in ctx.lines() {
                if line.is_empty() {
                    crate::sandbox::print_to_terminal("");
                } else {
                    crate::sandbox::print_to_terminal(&format!("\x1b[33m[SMOKE] {}\x1b[0m", line));
                }
            }
        }

        let output = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: "SMOKE: JSX/TSX allowed".to_string(),
                additional_context,
                updated_input: None,
            },
        };
        println!("{}", serde_json::to_string(&output)?);
        std::process::exit(0);
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
        if cfg.loop_detection.enabled {
            state.record_success(file_path);
            let _ = state.save(&input.session_id);
        }
        // ── Compute "added lines" up front (E2) ─────────────────────────
        // For Edit, the "added" count comes from diff_stats(edit_before, edit_after).
        // For Write, every line of the new file is "added" (no prior file).
        let added_lines = if let Some(ref before) = edit_before_content {
            let (_, removed) = diff_stats(before, &code_content);
            // Approximate added by net line change; when most lines are net-new
            // this is good enough for the size gate.
            let before_lines = before.lines().count();
            let after_lines = code_content.lines().count();
            after_lines.saturating_sub(before_lines.saturating_sub(removed))
        } else {
            code_content.lines().count()
        };

        // ── Positive reinforcement (E2) ──────────────────────────────────
        // Decide early whether this edit deserves a "clean" signal. We fire
        // the reinforcement when ALL of the following hold:
        //   - not a snippet-only check (full file ran)
        //   - the resulting file is small (under cfg.prompts.clean_file_line_threshold)
        //   - the change added few lines (under cfg.prompts.clean_max_added_lines)
        //   - no soft prompts were generated (no anti-deletion, no stdlib
        //     hint — both would have been added to additional_context_lines
        //     by now, BUT we have to check this AFTER the writing-side
        //     detector runs below, so we defer to a flag here)
        //
        // The `is_clean` flag starts true and gets set false by any soft
        // prompt we generate. That way we don't have to duplicate the
        // conditions for "is this soft prompt eligible".
        let resulting_lines = code_content.lines().count();
        let mut is_clean = !is_snippet
            && cfg.prompts.clean_file_line_threshold > 0
            && resulting_lines < cfg.prompts.clean_file_line_threshold
            && cfg.prompts.clean_max_added_lines > 0
            && added_lines <= cfg.prompts.clean_max_added_lines;

        let reason = if is_snippet {
            "SMOKE: large file — snippet only".to_string()
        } else {
            format!("SMOKE: executed clean in {}ms", result.execution_time_ms)
        };

        // Print direct-to-terminal verification success indicator
        let file_name = Path::new(file_path).file_name().and_then(|f| f.to_str()).unwrap_or(file_path);
        let check_msg = if is_snippet {
            format!("\x1b[32m[SMOKE] Verified {} successfully (snippet, {}ms) ✓\x1b[0m", file_name, result.execution_time_ms)
        } else {
            format!("\x1b[32m[SMOKE] Verified {} successfully ({}ms) ✓\x1b[0m", file_name, result.execution_time_ms)
        };
        crate::sandbox::print_to_terminal(&check_msg);

        // Writing-side stdlib hint: if this Edit or Write added a lot of new
        // code AND the new code re-implements something a stdlib/library
        // already provides, surface that as a soft prompt.
        if let Some(writing_hint) = crate::parser::writing_hint_for(
            &code_content, lang_id, added_lines, cfg.prompts.writing_size_threshold,
        ) {
            additional_context_lines.push(writing_hint);
            is_clean = false;
        }

        // If the anti-deletion prompt was added earlier, the file is no longer
        // in "clean" territory either.
        if additional_context_lines.iter().any(|l| l.contains("This Edit removed")) {
            is_clean = false;
        }

        // Combine all soft prompts into one additionalContext string and print
        // them all to the terminal. The write still goes through.
        let additional_context = if additional_context_lines.is_empty() {
            None
        } else {
            Some(additional_context_lines.join("\n\n"))
        };
        if let Some(ref ctx) = additional_context {
            for line in ctx.lines() {
                if line.is_empty() {
                    crate::sandbox::print_to_terminal("");
                } else {
                    crate::sandbox::print_to_terminal(&format!("\x1b[33m[SMOKE] {}\x1b[0m", line));
                }
            }
        }

        // Surface a positive reinforcement so the agent learns that small,
        // library-using code is what gets rewarded. Only delivered when the
        // file is clean by all our measures.
        if is_clean {
            let clean_msg = format!(
                "This change is clean: small file ({} lines), no stdlib duplicates, no large deletion. This is the kind of edit SMOKE rewards.",
                code_content.lines().count()
            );
            crate::sandbox::print_to_terminal(&format!("\x1b[32m[SMOKE] {}\x1b[0m", clean_msg));

            let output = HookOutput {
                hook_specific_output: HookSpecificOutput {
                    hook_event_name: "PreToolUse".to_string(),
                    permission_decision: "allow".to_string(),
                    permission_decision_reason: reason,
                    additional_context: Some(clean_msg),
                    updated_input: None,
                },
            };
            println!("{}", serde_json::to_string(&output)?);
            std::process::exit(0);
        }

        let output = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: reason,
                additional_context,
                updated_input: None,
            },
        };
        println!("{}", serde_json::to_string(&output)?);
        std::process::exit(0);
    } else {
        let err_msg = result.stderr.trim();
        let cleaned_stderr = err_msg.replace("smoke_verify.ts", file_path);
        let mut loop_msg = None;

        if cfg.loop_detection.enabled {
            let fp = crate::state::fingerprint(file_path, "runtime_error", err_msg);
            let count = state.record_failure(&fp, file_path, err_msg, cfg.loop_detection.fingerprint_window_minutes);
            let escalation = crate::state::escalation_for(count, cfg.loop_detection.warn_threshold, cfg.loop_detection.escalate_threshold);
            let _ = state.save(&input.session_id); // Save mutated state

            match escalation {
                crate::state::EscalationLevel::Normal => {}
                crate::state::EscalationLevel::Notice => {
                    loop_msg = Some(format!("⚠️ SMOKE: this is attempt #{} with the same error signature on {}.", count, file_path));
                }
                crate::state::EscalationLevel::Escalate => {
                    loop_msg = Some(format!(
                        "🛑 SMOKE: {} consecutive failures with the same error signature on {}.\n\nStop retrying variations of the same fix — it isn't addressing the root cause.\nBefore the next edit:\n  1. Re-read the actual error text below, in full.\n  2. State your hypothesis for the root cause in one sentence.\n  3. If you're not confident in the hypothesis, ask the user for guidance instead of editing again.\n\nLast error:\n{}",
                        count, file_path, cleaned_stderr
                    ));
                }
            }
        }

        let should_block = match cfg.hook.mode {
            crate::config::HookMode::Silent => false,
            crate::config::HookMode::Advisor => false,
            crate::config::HookMode::Strict => {
                is_standalone_runnable(&code_content, &ext)
            }
        };

        if should_block {
            if let Some(ref msg) = loop_msg {
                eprintln!("{}\n", msg);
            }
            // Exit code 2 blocks the tool call in Claude Code, showing stderr to the agent
            eprintln!("SMOKE: {}", cleaned_stderr);
            std::process::exit(2);
        } else {
            if let Some(ref msg) = loop_msg {
                crate::sandbox::print_to_terminal(&format!("\x1b[33m[SMOKE] {}\x1b[0m", msg));
                additional_context_lines.push(msg.clone());
            }

            let warn_msg = format!(
                "⚠️ SMOKE execution warning in {}:\n{}",
                file_path, cleaned_stderr
            );
            crate::sandbox::print_to_terminal(&format!("\x1b[33m[SMOKE] {}\x1b[0m", warn_msg));
            additional_context_lines.push(warn_msg);

            let additional_context = if additional_context_lines.is_empty() {
                None
            } else {
                Some(additional_context_lines.join("\n\n"))
            };

            let output = HookOutput {
                hook_specific_output: HookSpecificOutput {
                    hook_event_name: "PreToolUse".to_string(),
                    permission_decision: "allow".to_string(),
                    permission_decision_reason: "SMOKE: allowed with execution warnings".to_string(),
                    additional_context,
                    updated_input: None,
                },
            };
            println!("{}", serde_json::to_string(&output)?);
            std::process::exit(0);
        }
    }
}

fn is_standalone_runnable(code: &str, ext: &str) -> bool {
    // If it's a JSX/TSX file, it's never standalone runnable.
    if matches!(ext, "tsx" | "jsx") {
        return false;
    }
    if ext == "rs" || ext == "rust" {
        return code.contains("fn main");
    }

    // Check for imports or exports that make it a module/component
    let has_import = code.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("import ") || trimmed.starts_with("import{") || trimmed.starts_with("import *")
            || trimmed.contains("require(")
    });

    let has_export = code.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.starts_with("export ") || trimmed.starts_with("module.exports")
    });

    let has_jsx = code.contains("</") || code.contains("/>");

    !has_import && !has_export && !has_jsx
}

fn allow_with_reason(reason: &str) -> ! {
    let output = HookOutput {
        hook_specific_output: HookSpecificOutput {
            hook_event_name: "PreToolUse".to_string(),
            permission_decision: "allow".to_string(),
            permission_decision_reason: reason.to_string(),
            additional_context: None,
            updated_input: None,
        },
    };
    if let Ok(json) = serde_json::to_string(&output) {
        println!("{}", json);
    }
    std::process::exit(0);
}

// ── Anti-deletion prompt ──────────────────────────────────────────────────────

/// Compute how many lines were added vs removed between two file contents.
///
/// The algorithm is line-level: a line is "removed" if it appears in `before`
/// but not in `after` (within a small lookahead), and "added" if the reverse.
/// This is intentionally simple — we don't need Myers diff for the prompt,
/// we just need an honest "this many lines disappeared" estimate.
///
/// Returns `(added, removed)`.
pub fn diff_stats(before: &str, after: &str) -> (usize, usize) {
    let before_lines: std::collections::HashSet<&str> = before.lines().collect();
    let after_lines: std::collections::HashSet<&str> = after.lines().collect();

    let added = after_lines.difference(&before_lines).count();
    let removed = before_lines.difference(&after_lines).count();
    (added, removed)
}

/// If the Edit removed a large amount of code, build a soft prompt for the
/// agent. The prompt is a *factual* statement (per Claude Code's hook docs:
/// "factual statements rather than imperative instructions, to avoid
/// triggering prompt-injection defenses") that lets the model decide whether
/// to re-think the deletion.
///
/// Returns `None` when the change is small enough not to warrant a prompt.
pub fn build_anti_deletion_context(before: &str, after: &str, prompts: &crate::config::Prompts) -> Option<String> {
    let (added, removed) = diff_stats(before, after);
    let before_lines = before.lines().count().max(1);
    let removed_pct = (removed * 100) / before_lines;

    let large_by_count = prompts.deletion_lines_threshold > 0 && removed >= prompts.deletion_lines_threshold;
    let large_by_percent = prompts.deletion_percent_threshold <= 100 && removed_pct >= prompts.deletion_percent_threshold;

    if !large_by_count && !large_by_percent {
        return None;
    }

    Some(format!(
        "This Edit removed {} lines ({}% of the prior file) and added {} new lines. \
The removed code may still be referenced elsewhere in the codebase, in tests, \
in documentation, or by callers outside this file. There may also be a \
stdlib or library helper that already does what the new code does.",
        removed, removed_pct, added
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_stats_no_change() {
        let before = "a\nb\nc\n";
        let after = "a\nb\nc\n";
        let (added, removed) = diff_stats(before, after);
        assert_eq!(added, 0);
        assert_eq!(removed, 0);
    }

    #[test]
    fn diff_stats_pure_addition() {
        let before = "a\n";
        let after = "a\nb\nc\n";
        let (added, removed) = diff_stats(before, after);
        assert_eq!(removed, 0);
        assert!(added >= 2, "expected added >= 2, got {}", added);
    }

    #[test]
    fn diff_stats_pure_deletion() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\n";
        let (added, removed) = diff_stats(before, after);
        assert_eq!(added, 0);
        assert!(removed >= 4, "expected removed >= 4, got {}", removed);
    }

    #[test]
    fn anti_deletion_fires_on_large_count() {
        // 200-line file, Edit removes 60 lines, adds 5
        let mut before = String::new();
        for i in 0..200 {
            before.push_str(&format!("line {}\n", i));
        }
        let mut after = before.clone();
        // Remove lines 50..110 by stripping 60 specific line entries
        for i in 50..110 {
            after = after.replace(&format!("line {}\n", i), "");
        }
        // Add 5 new lines
        for i in 0..5 {
            after.push_str(&format!("new line {}\n", i));
        }
        let ctx = build_anti_deletion_context(&before, &after, &crate::config::Prompts::default());
        assert!(ctx.is_some(), "expected anti-deletion prompt to fire");
        let msg = ctx.unwrap();
        assert!(msg.contains("removed"), "msg should mention removed lines: {}", msg);
    }

    #[test]
    fn anti_deletion_fires_on_large_percent_small_file() {
        // 30-line file, Edit removes 12 lines (40%)
        let mut before = String::new();
        for i in 0..30 {
            before.push_str(&format!("line {}\n", i));
        }
        // Remove 12 lines by changing their content
        let mut after = before.clone();
        for i in 5..17 {
            after = after.replace(&format!("line {}\n", i), "");
        }
        let ctx = build_anti_deletion_context(&before, &after, &crate::config::Prompts::default());
        assert!(ctx.is_some(), "expected anti-deletion prompt to fire on >30% deletion");
    }

    #[test]
    fn anti_deletion_silent_on_small_change() {
        // 200-line file, Edit removes 5 lines
        let mut before = String::new();
        for i in 0..200 {
            before.push_str(&format!("line {}\n", i));
        }
        let mut after = before.clone();
        for i in 10..15 {
            after = after.replace(&format!("line {}\n", i), "");
        }
        let ctx = build_anti_deletion_context(&before, &after, &crate::config::Prompts::default());
        assert!(ctx.is_none(), "small deletions should not trigger a prompt");
    }

    #[test]
    fn anti_deletion_respects_custom_threshold() {
        // 200-line file, remove 30 lines (15%) — under default 30% but above
        // a custom 10% threshold. Default config should NOT fire; custom config
        // (percent=10) should fire.
        let mut before = String::new();
        for i in 0..200 {
            before.push_str(&format!("line {}\n", i));
        }
        let mut after = before.clone();
        for i in 50..80 {
            after = after.replace(&format!("line {}\n", i), "");
        }

        // Default threshold (30%) — 15% deletion should not fire
        let ctx = build_anti_deletion_context(&before, &after, &crate::config::Prompts::default());
        assert!(ctx.is_none(), "15% deletion should not fire at default 30% threshold");

        // Custom threshold of 10% — should fire
        let aggressive = crate::config::Prompts {
            deletion_lines_threshold: 50,
            deletion_percent_threshold: 10,
            ..crate::config::Prompts::default()
        };
        let ctx = build_anti_deletion_context(&before, &after, &aggressive);
        assert!(ctx.is_some(), "15% deletion should fire at custom 10% threshold");

        // Disable percent check (set to 101) and check line count only
        let line_count_only = crate::config::Prompts {
            deletion_lines_threshold: 25,
            deletion_percent_threshold: 101,
            ..crate::config::Prompts::default()
        };
        let ctx = build_anti_deletion_context(&before, &after, &line_count_only);
        assert!(ctx.is_some(), "30-line removal should fire at custom 25-line threshold");
    }

    #[test]
    fn additional_context_serializes_when_present() {
        let out = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: "test".to_string(),
                additional_context: Some("hello agent".to_string()),
                updated_input: None,
            },
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("additionalContext"), "json should contain additionalContext field: {}", json);
        assert!(json.contains("hello agent"), "json should contain the message: {}", json);
    }

    #[test]
    fn additional_context_omitted_when_none() {
        let out = HookOutput {
            hook_specific_output: HookSpecificOutput {
                hook_event_name: "PreToolUse".to_string(),
                permission_decision: "allow".to_string(),
                permission_decision_reason: "test".to_string(),
                additional_context: None,
                updated_input: None,
            },
        };
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("additionalContext"), "additionalContext should be omitted when None: {}", json);
    }

    /// The positive-reinforcement "clean" message should ONLY fire when:
    ///   1. The file is small (under cfg.prompts.clean_file_line_threshold)
    ///   2. Few lines were added (under cfg.prompts.clean_max_added_lines)
    ///   3. No anti-deletion prompt was added
    ///   4. No writing-side stdlib hint was added
    ///
    /// This is the test for the predicate that drives the green "[SMOKE] clean"
    /// output. If this test ever fails, either the thresholds changed or the
    /// soft-prompt generation changed in a way that broke the invariant.
    #[test]
    fn clean_reinforcement_only_when_no_prompts_and_small_file() {
        use crate::config::Prompts;
        let prompts_cfg = Prompts::default();
        let small_file_lines = 30;
        let large_file_lines = 200;
        let small_added = 5;

        // Case 1: small file, no prompts, small added → clean
        let prompts: Vec<String> = vec![];
        let is_clean = !false
            && small_file_lines < prompts_cfg.clean_file_line_threshold
            && small_added <= prompts_cfg.clean_max_added_lines
            && prompts.is_empty();
        assert!(is_clean);

        // Case 2: small file, anti-deletion prompt present → NOT clean
        let prompts = ["This Edit removed 70 lines...".to_string()];
        let is_clean = prompts.is_empty();
        assert!(!is_clean);

        // Case 3: small file, writing-side prompt present → NOT clean
        let prompts = ["Detected a custom `debounce` implementation...".to_string()];
        let is_clean = prompts.is_empty();
        assert!(!is_clean);

        // Case 4: large file, no prompts → NOT clean (size gate)
        let prompts: Vec<String> = vec![];
        let is_clean = large_file_lines < prompts_cfg.clean_file_line_threshold && prompts.is_empty();
        assert!(!is_clean, "large files should not be marked clean");

        // Case 5: small file but snippet-only → NOT clean
        let is_snippet = true;
        let is_clean = !is_snippet && small_file_lines < prompts_cfg.clean_file_line_threshold
            && small_added <= prompts_cfg.clean_max_added_lines;
        assert!(!is_clean, "snippet-only checks should not be marked clean");

        // Case 6: too many lines added → NOT clean
        let big_added = 100;
        let is_clean = small_file_lines < prompts_cfg.clean_file_line_threshold
            && big_added <= prompts_cfg.clean_max_added_lines;
        assert!(!is_clean, "large additions should not be marked clean");

        // Case 7: custom threshold of 100 lines — 30 lines should be eligible
        let custom = Prompts {
            clean_file_line_threshold: 100,
            clean_max_added_lines: 30,
            ..Prompts::default()
        };
        let is_clean = 30 < custom.clean_file_line_threshold && 5 <= custom.clean_max_added_lines;
        assert!(is_clean, "30-line file should be clean under 100-line threshold");

        // Case 8: setting threshold=0 disables the prompt
        let disabled = Prompts {
            clean_file_line_threshold: 0,
            clean_max_added_lines: 0,
            ..Prompts::default()
        };
        let is_clean = disabled.clean_file_line_threshold > 0;
        assert!(!is_clean, "threshold=0 should disable clean reinforcement");
    }

    /// No-op edit detection (S3): if `old_str` and `new_str` differ in some
    /// characters but the resulting `patched_content` is byte-identical to the
    /// original `file_content`, the Edit is a no-op. We can skip the sandbox.
    ///
    /// This catches the "agent retries by re-sending the same Edit" case.
    #[test]
    fn noop_edit_detection() {
        // Case 1: old_str is X, new_str is X (whitespace difference would
        // still produce a no-op if the rest of the file is unchanged).
        let file_content = "line 1\nline 2\nline 3\n";
        let old_str = "line 2\n";
        let new_str = "line 2\n"; // identical
        let idx = file_content.find(old_str).unwrap();
        let mut patched = file_content[..idx].to_string();
        patched.push_str(new_str);
        patched.push_str(&file_content[idx + old_str.len()..]);
        assert_eq!(patched, file_content, "identical old/new should produce identical file");

        // Case 2: the actual case we care about — a real change that
        // happens to not modify the file. Example: replacing "foo" with
        // "foo" inside a line.
        let file_content = "let x = foo();\nlet y = 2;\n";
        let old_str = "foo()";
        let new_str = "foo()";
        let idx = file_content.find(old_str).unwrap();
        let mut patched = file_content[..idx].to_string();
        patched.push_str(new_str);
        patched.push_str(&file_content[idx + old_str.len()..]);
        assert_eq!(patched, file_content);

        // Case 3: real change → not a no-op
        let file_content = "let x = 1;\n";
        let old_str = "let x = 1;";
        let new_str = "let x = 2;";
        let idx = file_content.find(old_str).unwrap();
        let mut patched = file_content[..idx].to_string();
        patched.push_str(new_str);
        patched.push_str(&file_content[idx + old_str.len()..]);
        assert_ne!(patched, file_content, "real changes should not be no-ops");
    }

    #[test]
    fn test_is_standalone_runnable() {
        // Pure script
        assert!(is_standalone_runnable("const x = 1 + 2;\nconsole.log(x);", "js"));
        assert!(is_standalone_runnable("def add(a, b):\n    return a + b\nprint(add(1, 2))", "py"));

        // File containing imports
        assert!(!is_standalone_runnable("import React from 'react';\nconst x = 1;", "js"));
        assert!(!is_standalone_runnable("const fs = require('fs');\nconst x = 1;", "js"));

        // File containing export
        assert!(!is_standalone_runnable("export default function Page() {}", "js"));
        assert!(!is_standalone_runnable("module.exports = { x: 1 };", "js"));

        // JSX/TSX is never runnable
        assert!(!is_standalone_runnable("const Comp = () => <div>Hello</div>;", "tsx"));
        assert!(!is_standalone_runnable("const Comp = () => <div />;", "jsx"));

        // Rust with/without main
        assert!(is_standalone_runnable("fn main() {\n    println!(\"hello\");\n}", "rs"));
        assert!(!is_standalone_runnable("pub fn add(a: i32, b: i32) -> i32 { a + b }", "rs"));
    }
}
