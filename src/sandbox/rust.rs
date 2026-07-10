use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;
use crate::sandbox::SandboxResult;

struct TempFileSwap {
    path: PathBuf,
    original_content: Option<String>,
    existed: bool,
}

impl TempFileSwap {
    fn new(path: &Path, new_content: &str) -> std::io::Result<Self> {
        let path = path.to_path_buf();
        let existed = path.exists();
        let original_content = if existed {
            Some(std::fs::read_to_string(&path)?)
        } else {
            None
        };

        // Create parent directories if they do not exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, new_content)?;

        Ok(Self {
            path,
            original_content,
            existed,
        })
    }
}

impl Drop for TempFileSwap {
    fn drop(&mut self) {
        if self.existed {
            if let Some(ref content) = self.original_content {
                let _ = std::fs::write(&self.path, content);
            }
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub struct RustSandbox;

impl RustSandbox {
    pub fn new() -> Self {
        Self
    }

    /// Checks the Rust file using `cargo check` (if in a cargo project) or `rustc`.
    /// Does not run the code, only verifies it compiles successfully.
    pub async fn execute(
        &mut self,
        code_content: &str,
        file_path: Option<&str>,
        cwd: &str,
        timeout_ms: u64,
    ) -> SandboxResult {
        let start = std::time::Instant::now();

        // 1. Find Cargo.toml in the directory tree
        let start_dir = if let Some(fp) = file_path {
            Path::new(cwd).join(fp)
        } else {
            Path::new(cwd).to_path_buf()
        };

        let cargo_toml = find_cargo_toml(&start_dir);

        let mut passed = false;
        let mut stdout = String::new();
        let mut stderr = String::new();

        if let Some(toml_path) = cargo_toml {
            // We have a Cargo project workspace!
            // We can do a workspace-aware check by temporarily writing the file content
            if let Some(fp) = file_path {
                let target_path = Path::new(cwd).join(fp);
                
                // Safe RAII swap
                let _swap = match TempFileSwap::new(&target_path, code_content) {
                    Ok(s) => s,
                    Err(e) => {
                        return SandboxResult::error(
                            "rust",
                            format!("Failed to write temporary file for validation: {}", e),
                            start.elapsed().as_millis() as u64,
                        );
                    }
                };

                let cargo_dir = toml_path.parent().unwrap_or(Path::new(cwd));

                // Run cargo check
                let mut cmd = Command::new("cargo");
                cmd.arg("check")
                    .arg("--tests")
                    .arg("--color")
                    .arg("never")
                    .current_dir(cargo_dir)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);

                let child = match cmd.spawn() {
                    Ok(c) => c,
                    Err(e) => {
                        return SandboxResult::error(
                            "rust",
                            format!("Failed to spawn cargo check: {}", e),
                            start.elapsed().as_millis() as u64,
                        );
                    }
                };

                let timeout_dur = tokio::time::Duration::from_millis(timeout_ms);
                let wait_res = tokio::select! {
                    res = child.wait_with_output() => Some(res),
                    _ = tokio::time::sleep(timeout_dur) => None,
                };

                if let Some(Ok(out)) = wait_res {
                    stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    passed = out.status.success();
                } else {
                    stderr = "Validation timed out running cargo check".to_string();
                }
            } else {
                // Standalone mode check fallback since no file_path provided
                run_rustc_fallback(code_content, timeout_ms, &start, &mut passed, &mut stdout, &mut stderr).await;
            }
        } else {
            // No Cargo.toml found: run standalone rustc check
            run_rustc_fallback(code_content, timeout_ms, &start, &mut passed, &mut stdout, &mut stderr).await;
        }

        SandboxResult {
            passed,
            stdout,
            stderr,
            execution_time_ms: start.elapsed().as_millis() as u64,
            language: "rust".to_string(),
        }
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

async fn run_rustc_fallback(
    code_content: &str,
    timeout_ms: u64,
    start: &std::time::Instant,
    passed: &mut bool,
    _stdout: &mut String,
    stderr: &mut String,
) {
    // Generate a temp file to compile
    let temp_dir = std::env::temp_dir();
    let temp_file_path = temp_dir.join(format!("smoke_verify_{}.rs", start.elapsed().as_micros()));
    if std::fs::write(&temp_file_path, code_content).is_err() {
        *passed = false;
        *stderr = "Failed to write temporary file for rustc validation".to_string();
        return;
    }

    let mut cmd = Command::new("rustc");
    cmd.arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("-o")
        .arg(temp_dir.join(format!("smoke_verify_{}.rmeta", start.elapsed().as_micros())))
        .arg(&temp_file_path)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_file_path);
            *passed = false;
            *stderr = format!("Failed to spawn rustc: {}. Make sure Rust/rustc is installed and in your PATH.", e);
            return;
        }
    };

    let wait_res = tokio::select! {
        res = child.wait_with_output() => Some(res),
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(timeout_ms)) => None,
    };

    if let Some(Ok(out)) = wait_res {
        *stderr = String::from_utf8_lossy(&out.stderr).to_string();
        *passed = out.status.success();
    } else {
        *stderr = "Validation timed out running rustc".to_string();
    }

    let _ = std::fs::remove_file(&temp_file_path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rust_sandbox_valid() {
        let mut sandbox = RustSandbox::new();
        let code = r#"
            pub fn add(a: i32, b: i32) -> i32 {
                a + b
            }
        "#;
        let res = sandbox.execute(code, None, ".", 5000).await;
        if !res.passed {
            panic!("res.passed was false! stderr: {}", res.stderr);
        }
        assert_eq!(res.language, "rust");
    }

    #[tokio::test]
    async fn test_rust_sandbox_invalid() {
        let mut sandbox = RustSandbox::new();
        let code = r#"
            pub fn add(a: i32, b: i32) -> i32 {
                a + b; // Error: returns () instead of i32
            }
        "#;
        let res = sandbox.execute(code, None, ".", 5000).await;
        assert!(!res.passed);
        assert!(res.stderr.contains("mismatched types") || res.stderr.contains("expected `i32`"));
    }
}
