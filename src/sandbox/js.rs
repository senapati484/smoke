// Phase 1: JS/TS sandbox implementation
// Uses rustyscript (wraps deno_core / V8) — sandboxed by default.
// DO NOT add extensions that grant fs/net/env access.

use crate::sandbox::SandboxResult;
use rustyscript::{Runtime, RuntimeOptions, Module};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub struct JsSandbox;

impl JsSandbox {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    pub fn execute(&mut self, code: &str, is_typescript: bool, timeout_ms: u64) -> SandboxResult {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        let mut runtime = match Runtime::new(RuntimeOptions {
            timeout,
            ..Default::default()
        }) {
            Ok(r) => r,
            Err(e) => {
                return SandboxResult::error(
                    if is_typescript { "typescript" } else { "javascript" },
                    format!("Failed to initialize V8 runtime: {}", e),
                    start.elapsed().as_millis() as u64,
                )
            }
        };

        // Get the thread-safe isolate handle from deno_runtime to support terminating
        // synchronous infinite loops (like while(true){}) which would otherwise block
        // tokio's single-threaded async select/timeout mechanism.
        let isolate_handle = runtime.deno_runtime().v8_isolate().thread_safe_handle();
        let finished = Arc::new(AtomicBool::new(false));

        let finished_clone = finished.clone();
        let watchdog_handle = isolate_handle.clone();
        std::thread::spawn(move || {
            let start_time = std::time::Instant::now();
            while start_time.elapsed() < timeout {
                if finished_clone.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            if !finished_clone.load(Ordering::Relaxed) {
                // Forcibly terminate JavaScript execution on V8 isolate
                watchdog_handle.terminate_execution();
            }
        });

        // Preamble to capture stdout/stderr from console logs safely on globalThis.
        // Needs to use globalThis because ES modules (used for TS) have module-scoped variables.
        let preamble = r#"
            globalThis._smoke_logs = [];
            globalThis._smoke_errs = [];
            globalThis.console = {
              log:   (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
              error: (...args) => globalThis._smoke_errs.push(args.map(String).join(' ')),
              warn:  (...args) => globalThis._smoke_errs.push('[warn] ' + globalThis._smoke_errs.push(args.map(String).join(' '))),
              info:  (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
            };
        "#;

        let combined_code = format!("{}\n{}", preamble, code);
        let lang_str = if is_typescript { "typescript" } else { "javascript" };

        // Evaluate the combined code
        let eval_result = if is_typescript {
            // Load as module to trigger Deno's TypeScript transpilation
            let module = Module::new("index.ts", &combined_code);
            runtime.load_module(&module).map(|_| serde_json::Value::Null)
        } else {
            runtime.eval::<serde_json::Value>(&combined_code)
        };

        // Notify watchdog thread to exit early
        finished.store(true, Ordering::Relaxed);

        if let Err(e) = eval_result {
            let elapsed = start.elapsed().as_millis() as u64;
            let err_msg = e.to_string();
            // Check for timeout or termination signals from watchdog
            if err_msg.to_lowercase().contains("timeout") 
                || err_msg.to_lowercase().contains("timed out")
                || err_msg.to_lowercase().contains("terminated")
                || err_msg.to_lowercase().contains("execution terminated")
            {
                return SandboxResult::error(
                    lang_str,
                    format!("Execution timed out after {}ms", timeout_ms),
                    elapsed,
                );
            }
            return SandboxResult::error(lang_str, err_msg, elapsed);
        }

        // Retrieve the captured console logs from the global environment
        let stdout_logs: Vec<String> = match runtime.eval::<Vec<String>>("globalThis._smoke_logs") {
            Ok(logs) => logs,
            Err(e) => {
                return SandboxResult::error(
                    lang_str,
                    format!("Failed to retrieve stdout logs: {}", e),
                    start.elapsed().as_millis() as u64,
                )
            }
        };

        let stderr_logs: Vec<String> = match runtime.eval::<Vec<String>>("globalThis._smoke_errs") {
            Ok(errs) => errs,
            Err(e) => {
                return SandboxResult::error(
                    lang_str,
                    format!("Failed to retrieve stderr logs: {}", e),
                    start.elapsed().as_millis() as u64,
                )
            }
        };

        let elapsed = start.elapsed().as_millis() as u64;
        let stdout = stdout_logs.join("\n");
        let stderr = stderr_logs.join("\n");

        if !stderr.is_empty() {
            SandboxResult {
                passed: false,
                stdout,
                stderr,
                execution_time_ms: elapsed,
                language: lang_str.to_string(),
            }
        } else {
            SandboxResult {
                passed: true,
                stdout,
                stderr: String::new(),
                execution_time_ms: elapsed,
                language: lang_str.to_string(),
            }
        }
    }
}
