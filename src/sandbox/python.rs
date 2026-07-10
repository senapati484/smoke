// Phase 5: Python sandbox implementation
// Uses std::process::Command ONLY — no pyo3, no in-process Python.
// The Python interpreter runs as a separate OS process with:
//   - rlimit (CPU time, memory, open files)
//   - extrasafe seccomp filter on Linux (deny fork/exec, raw sockets)
// This is process-isolated, NOT sandboxed. See README Security Model.

use crate::sandbox::SandboxResult;
use std::io::Write;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::process::Command;

pub struct PythonSandbox;

impl PythonSandbox {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(&mut self, code: &str, interpreter: &str, timeout_ms: u64) -> SandboxResult {
        let start = std::time::Instant::now();
        let timeout = Duration::from_millis(timeout_ms);

        // 1. Create a temp file to store user code
        let mut temp_file = match NamedTempFile::new() {
            Ok(f) => f,
            Err(e) => {
                return SandboxResult::error(
                    "python",
                    format!("Failed to create temporary script file: {}", e),
                    start.elapsed().as_millis() as u64,
                )
            }
        };

        if let Err(e) = temp_file.write_all(code.as_bytes()) {
            return SandboxResult::error(
                "python",
                format!("Failed to write script to temporary file: {}", e),
                start.elapsed().as_millis() as u64,
            );
        }

        let temp_path = temp_file.path().to_path_buf();

        // 2. Configure python Command
        let mut cmd = Command::new(interpreter);
        cmd.arg(&temp_path);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Configure process limits and seccomp filters on Unix
        #[cfg(unix)]
        {
            let timeout_sec = (timeout_ms / 1000 + 2) as u64;
            // 256MB virtual memory limit
            let mem_limit_bytes = 256 * 1024 * 1024;

            unsafe {
                cmd.pre_exec(move || {
                    // Start in its own process group to allow clean timeout termination
                    let _ = libc::setpgid(0, 0);

                    // Set resource limits to prevent DoS
                    let _ = rlimit::Resource::CPU.set(timeout_sec, timeout_sec);
                    let _ = rlimit::Resource::AS.set(mem_limit_bytes, mem_limit_bytes);
                    let _ = rlimit::Resource::NOFILE.set(32, 32);

                    // Apply seccomp filter on Linux to deny fork/exec and sockets
                    #[cfg(target_os = "linux")]
                    {
                        use extrasafe::builtins::SystemIO;
                        use extrasafe::SafetyContext;

                        if let Ok(context) = SafetyContext::new().enable(SystemIO::everything()) {
                            let _ = context.apply_to_current_thread();
                        }
                    }

                    Ok(())
                });
            }
        }

        // 3. Spawn child process
        #[allow(unused_mut)]
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return SandboxResult::error(
                    "python",
                    format!("Failed to launch Python interpreter '{}': {}", interpreter, e),
                    start.elapsed().as_millis() as u64,
                )
            }
        };

        let child_pid = child.id();

        // 4. Wait for execution or timeout asynchronously
        let wait_result = tokio::select! {
            res = child.wait_with_output() => Some(res),
            _ = tokio::time::sleep(timeout) => {
                #[cfg(windows)]
                {
                    let _ = child.kill().await;
                }
                None
            }
        };

        let elapsed = start.elapsed().as_millis() as u64;

        // Cleanup temp file immediately
        let _ = temp_file.close();

        // 5. Evaluate execution outcome
        match wait_result {
            Some(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                if output.status.success() {
                    SandboxResult {
                        passed: true,
                        stdout,
                        stderr: String::new(),
                        execution_time_ms: elapsed,
                        language: "python".to_string(),
                    }
                } else {
                    SandboxResult {
                        passed: false,
                        stdout,
                        stderr,
                        execution_time_ms: elapsed,
                        language: "python".to_string(),
                    }
                }
            }
            Some(Err(e)) => SandboxResult::error("python", format!("Failed to read output: {}", e), elapsed),
            None => {
                // Timeout occurred: kill the child process group
                #[cfg(unix)]
                {
                    if let Some(pid) = child_pid {
                        let pgid = -(pid as libc::pid_t);
                        unsafe {
                            // Send SIGTERM to process group
                            let _ = libc::kill(pgid, libc::SIGTERM);
                        }
                        // Wait 500ms (non-blocking)
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        unsafe {
                            // Force kill process group if still running
                            let _ = libc::kill(pgid, libc::SIGKILL);
                        }
                    }
                }

                SandboxResult::error(
                    "python",
                    format!("Execution timed out after {}ms", timeout_ms),
                    elapsed,
                )
            }
        }
    }
}
