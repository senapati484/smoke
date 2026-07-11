// Phase 1: JS/TS sandbox implementation
// Uses rustyscript (wraps deno_core / V8) — sandboxed by default.
// DO NOT add extensions that grant fs/net/env access.
//
// Performance: the V8 runtime is persistent across calls (S1). A single
// long-lived worker thread owns the Runtime; the hook sends execution
// requests via a channel. This avoids re-initializing V8 (60-70ms) on
// every verification — the biggest single latency source for JS/TS edits.

use crate::sandbox::SandboxResult;
use rustyscript::{Runtime, RuntimeOptions, Module};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender, Receiver};
use std::time::Duration;

// ── Persistent V8 worker (S1) ─────────────────────────────────────────────────

/// Request sent to the persistent V8 worker thread.
struct ExecuteRequest {
    code: String,
    is_typescript: bool,
    timeout_ms: u64,
    /// Channel to send the result back. The worker holds the receiver, the
    /// caller holds the sender.
    response_tx: Sender<SandboxResult>,
}

/// Handle to the persistent V8 worker. One per process. Lazily created on
/// the first `JsSandbox::execute` call. The worker thread is daemon so it
/// doesn't block process exit if the hook is killed.
struct V8Worker {
    sender: Sender<ExecuteRequest>,
    /// Set to true when a fatal error (worker panic, etc.) is detected. We
    /// then fall back to creating a fresh V8 runtime inline.
    broken: Arc<AtomicBool>,
}

impl V8Worker {
    fn global() -> &'static Arc<V8Worker> {
        use std::sync::OnceLock;
        static WORKER: OnceLock<Arc<V8Worker>> = OnceLock::new();
        WORKER.get_or_init(|| {
            let (tx, rx) = channel::<ExecuteRequest>();
            let broken = Arc::new(AtomicBool::new(false));
            let broken_clone = broken.clone();
            std::thread::Builder::new()
                .name("smoke-v8-worker".into())
                .spawn(move || v8_worker_loop(rx, broken_clone))
                .expect("failed to spawn V8 worker thread");
            Arc::new(V8Worker { sender: tx, broken })
        })
    }
}

fn v8_worker_loop(rx: Receiver<ExecuteRequest>, broken: Arc<AtomicBool>) {
    // Initialize V8 once for the lifetime of the worker.
    let start = std::time::Instant::now();
    let mut runtime = match Runtime::new(RuntimeOptions::default()) {
        Ok(r) => r,
        Err(e) => {
            // Couldn't init V8 — mark the worker broken so future calls
            // fall back to the slow path.
            eprintln!("SMOKE: V8 worker failed to initialize: {}", e);
            broken.store(true, Ordering::SeqCst);
            return;
        }
    };
    eprintln!("SMOKE: V8 worker initialized in {}ms (persistent for hook lifetime)", start.elapsed().as_millis());

    while let Ok(req) = rx.recv() {
        let (should_exit, result) = run_against_runtime(&mut runtime, &req);
        // Send the result back to the caller. If the caller has dropped
        // the receiver (e.g. they hit their own timeout), the send fails
        // silently — that's fine.
        let _ = req.response_tx.send(result);
        if should_exit {
            broken.store(true, Ordering::SeqCst);
            return;
        }
    }
}

/// Run a single request against the given runtime. Returns (should_exit, result).
/// `should_exit = true` means the runtime is in a fatal state and the worker
/// should stop processing further requests.
fn run_against_runtime(runtime: &mut Runtime, req: &ExecuteRequest) -> (bool, SandboxResult) {
    let start = std::time::Instant::now();
    let timeout = Duration::from_millis(req.timeout_ms);
    let lang_str = if req.is_typescript { "typescript" } else { "javascript" };

    // Apply the timeout to the runtime for this request
    // (rustyscript's RuntimeOptions is set at construction; for per-request
    // timeout we use the watchdog approach below.)

    // Get the thread-safe isolate handle for the watchdog
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
            watchdog_handle.terminate_execution();
        }
    });

    // Capture console.log/info/warn/error via globalThis, same as before
    let preamble = r#"
        globalThis._smoke_logs = [];
        globalThis._smoke_errs = [];
        globalThis.console = {
          log:   (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
          error: (...args) => globalThis._smoke_errs.push(args.map(String).join(' ')),
          warn:  (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
          info:  (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
        };
    "#;

    let combined_code = format!("{}\n{}", preamble, req.code);

    let eval_result = if req.is_typescript {
        let module = Module::new("smoke_verify.ts", &combined_code);
        runtime.load_module(&module).map(|_| serde_json::Value::Null)
    } else {
        runtime.eval::<serde_json::Value>(&combined_code)
    };

    finished.store(true, Ordering::Relaxed);

    if let Err(e) = eval_result {
        let elapsed = start.elapsed().as_millis() as u64;
        let err_msg = e.to_string();
        // A V8 isolate in a broken state (e.g. after terminate_execution) may
        // not be reusable. Detect "fatal" errors that suggest the isolate is
        // hosed and signal the worker to exit.
        let is_fatal = err_msg.to_lowercase().contains("isolate")
            && (err_msg.to_lowercase().contains("terminat") || err_msg.to_lowercase().contains("abort"));
        if err_msg.to_lowercase().contains("timeout")
            || err_msg.to_lowercase().contains("timed out")
            || err_msg.to_lowercase().contains("terminated")
            || err_msg.to_lowercase().contains("execution terminated")
        {
            return (is_fatal, SandboxResult::error(
                lang_str,
                format!("Execution timed out after {}ms", req.timeout_ms),
                elapsed,
            ));
        }
        return (is_fatal, SandboxResult::error(lang_str, err_msg, elapsed));
    }

    // Retrieve captured logs
    let stdout_logs: Vec<String> = match runtime.eval::<Vec<String>>("globalThis._smoke_logs") {
        Ok(logs) => logs,
        Err(e) => {
            return (false, SandboxResult::error(
                lang_str,
                format!("Failed to retrieve stdout logs: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
    };
    let stderr_logs: Vec<String> = match runtime.eval::<Vec<String>>("globalThis._smoke_errs") {
        Ok(errs) => errs,
        Err(e) => {
            return (false, SandboxResult::error(
                lang_str,
                format!("Failed to retrieve stderr logs: {}", e),
                start.elapsed().as_millis() as u64,
            ));
        }
    };

    let elapsed = start.elapsed().as_millis() as u64;
    let stdout = stdout_logs.join("\n");
    let stderr = stderr_logs.join("\n");

    if !stderr.is_empty() {
        (false, SandboxResult {
            passed: false,
            stdout,
            stderr,
            execution_time_ms: elapsed,
            language: lang_str.to_string(),
        })
    } else {
        (false, SandboxResult {
            passed: true,
            stdout,
            stderr: String::new(),
            execution_time_ms: elapsed,
            language: lang_str.to_string(),
        })
    }
}

// ── JsSandbox public API ──────────────────────────────────────────────────────

pub struct JsSandbox;

impl JsSandbox {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// Execute JS/TS code in a persistent V8 sandbox.
    ///
    /// On the first call, a worker thread is spawned that owns the V8 runtime
    /// for the lifetime of the process. Subsequent calls reuse it. If the
    /// worker is broken (initialization failed or a fatal V8 error occurred),
    /// we fall back to creating a fresh runtime inline.
    pub fn execute(&mut self, code: &str, is_typescript: bool, timeout_ms: u64) -> SandboxResult {
        let worker = V8Worker::global();

        // Fast path: send to the worker
        if !worker.broken.load(Ordering::SeqCst) {
            let (resp_tx, resp_rx) = channel::<SandboxResult>();
            let request = ExecuteRequest {
                code: code.to_string(),
                is_typescript,
                timeout_ms,
                response_tx: resp_tx,
            };
            if worker.sender.send(request).is_ok() {
                // Wait for the response. The worker is single-threaded, so
                // requests are processed sequentially. A blocking recv here
                // is fine because we don't have other work to do.
                return match resp_rx.recv() {
                    Ok(result) => result,
                    Err(_) => {
                        // Worker died. Mark broken and fall back to slow path.
                        worker.broken.store(true, Ordering::SeqCst);
                        self.execute_fallback(code, is_typescript, timeout_ms)
                    }
                };
            }
        }

        // Slow path: worker broken or send failed. Create a fresh runtime inline.
        self.execute_fallback(code, is_typescript, timeout_ms)
    }

    /// Slow path: spawn a new V8 runtime inline. Used as fallback when the
    /// persistent worker is unavailable.
    fn execute_fallback(&self, code: &str, is_typescript: bool, timeout_ms: u64) -> SandboxResult {
        let code = code.to_string();
        let handle = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let timeout = Duration::from_millis(timeout_ms);

            let mut runtime = match Runtime::new(RuntimeOptions { timeout, ..Default::default() }) {
                Ok(r) => r,
                Err(e) => {
                    return SandboxResult::error(
                        if is_typescript { "typescript" } else { "javascript" },
                        format!("Failed to initialize V8 runtime: {}", e),
                        start.elapsed().as_millis() as u64,
                    )
                }
            };

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
                    watchdog_handle.terminate_execution();
                }
            });

            let preamble = r#"
                globalThis._smoke_logs = [];
                globalThis._smoke_errs = [];
                globalThis.console = {
                  log:   (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
                  error: (...args) => globalThis._smoke_errs.push(args.map(String).join(' ')),
                  warn:  (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
                  info:  (...args) => globalThis._smoke_logs.push(args.map(String).join(' ')),
                };
            "#;

            let combined_code = format!("{}\n{}", preamble, code);
            let lang_str = if is_typescript { "typescript" } else { "javascript" };

            let eval_result = if is_typescript {
                let module = Module::new("smoke_verify.ts", &combined_code);
                runtime.load_module(&module).map(|_| serde_json::Value::Null)
            } else {
                runtime.eval::<serde_json::Value>(&combined_code)
            };

            finished.store(true, Ordering::Relaxed);

            if let Err(e) = eval_result {
                let elapsed = start.elapsed().as_millis() as u64;
                let err_msg = e.to_string();
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
        });

        match handle.join() {
            Ok(res) => res,
            Err(_) => SandboxResult::error(
                if is_typescript { "typescript" } else { "javascript" },
                "V8 runtime crashed with thread panic",
                0,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `console.warn` and `console.info` should be treated as stdout, not stderr.
    /// A common false-positive in earlier versions: any `console.warn` call caused
    /// the sandbox to return passed=false, which blocked AI writes that used warn
    /// for progress logging. Now only `console.error` and thrown exceptions are
    /// considered fatal.
    #[test]
    fn warn_and_info_are_not_fatal() {
        let mut sb = JsSandbox::new().expect("failed to init JS sandbox");
        let code = r#"
            console.log("started");
            console.info("info message");
            console.warn("warn message");
            console.log("done");
        "#;
        let res = sb.execute(code, false, 5000);
        assert!(res.passed, "expected passed=true, got stderr: {}", res.stderr);
        assert!(res.stderr.is_empty(), "stderr should be empty, got: {}", res.stderr);
        assert!(res.stdout.contains("started"));
        assert!(res.stdout.contains("info message"));
        assert!(res.stdout.contains("warn message"));
        assert!(res.stdout.contains("done"));
    }

    /// `console.error` must still route to stderr and cause passed=false.
    /// This is the contract that protects the rest of the change — if we ever
    /// accidentally also move error to logs, this test will fail.
    #[test]
    fn error_is_still_fatal() {
        let mut sb = JsSandbox::new().expect("failed to init JS sandbox");
        let code = r#"
            console.log("ok");
            console.error("boom");
        "#;
        let res = sb.execute(code, false, 5000);
        assert!(!res.passed, "expected passed=false when console.error is called");
        assert!(res.stderr.contains("boom"), "stderr should contain 'boom', got: {}", res.stderr);
    }

    /// Thrown exceptions must still be caught and reported as failures.
    /// This is the second half of the contract — JS-level throws are the
    /// primary way an agent's bug surfaces, and we cannot regress here.
    #[test]
    fn thrown_exception_is_fatal() {
        let mut sb = JsSandbox::new().expect("failed to init JS sandbox");
        let code = r#"throw new Error("kaboom");"#;
        let res = sb.execute(code, false, 5000);
        assert!(!res.passed, "expected passed=false when code throws");
        assert!(
            res.stderr.to_lowercase().contains("kaboom")
                || res.stderr.to_lowercase().contains("error"),
            "stderr should mention the thrown error, got: {}",
            res.stderr
        );
    }

    /// Persistent V8 worker (S1): a second call on the same `JsSandbox`
    /// instance should reuse the worker's V8 runtime. We can't directly
    /// observe the runtime, but we can verify both calls produce correct
    /// results (i.e. the second call works at all — if the worker were
    /// broken, the fallback would still work but it'd be much slower).
    ///
    /// This test also exercises the cross-call state boundary: any state
    /// the first call set in globalThis should be visible to the second
    /// call. The preamble resets _smoke_logs/_smoke_errs at the start of
    /// each request, so this is safe by construction.
    #[test]
    fn v8_worker_reuse() {
        let mut sb = JsSandbox::new().expect("failed to init JS sandbox");

        // First call
        let r1 = sb.execute(r#"console.log("first");"#, false, 5000);
        assert!(r1.passed, "first call should pass: {}", r1.stderr);
        assert!(r1.stdout.contains("first"));

        // Second call on the same instance
        let r2 = sb.execute(r#"console.log("second");"#, false, 5000);
        assert!(r2.passed, "second call should pass: {}", r2.stderr);
        assert!(r2.stdout.contains("second"));

        // Cross-call state check: the second call's preamble should have
        // reset _smoke_logs, so the first call's "first" should NOT be
        // visible in the second call's stdout. (Without the reset, the
        // logs array would accumulate across calls and the assertion
        // would fail.)
        assert!(
            !r2.stdout.contains("first"),
            "second call's stdout should not contain the first call's logs, got: {}",
            r2.stdout
        );
    }
}
