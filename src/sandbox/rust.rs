use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};
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

        // 0. Cache lookup (S2): if the same (file, content, workspace) was
        // checked recently and the workspace hasn't changed, return cached.
        // This is the most impactful optimization for the Rust path — cargo
        // check on a non-trivial workspace can take 30+ seconds, and the
        // agent rarely changes anything between successive edits to the
        // same file.
        if let Some(fp) = file_path {
            let workspace_dir = Path::new(cwd);
            if let Some(cached) = try_cached(fp, code_content, workspace_dir) {
                return SandboxResult {
                    execution_time_ms: start.elapsed().as_millis() as u64,
                    ..cached
                };
            }
        }

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

        let result = SandboxResult {
            passed,
            stdout,
            stderr,
            execution_time_ms: start.elapsed().as_millis() as u64,
            language: "rust".to_string(),
        };

        // Store in cache for next time (only if we have a file_path to key on)
        if let Some(fp) = file_path {
            store_cached(fp, code_content, Path::new(cwd), &result);
        }

        result
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
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

    // Generate a temp file to compile
    let temp_dir = std::env::temp_dir();
    let unique_id = format!("{}_{}", start.elapsed().as_micros(), count);
    let temp_file_path = temp_dir.join(format!("smoke_verify_{}.rs", unique_id));
    if std::fs::write(&temp_file_path, code_content).is_err() {
        *passed = false;
        *stderr = "Failed to write temporary file for rustc validation".to_string();
        return;
    }

    let mut cmd = Command::new("rustc");
    cmd.arg("--crate-type=lib")
        .arg("--emit=metadata")
        .arg("-o")
        .arg(temp_dir.join(format!("smoke_verify_{}.rmeta", unique_id)))
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

// ── Cache (S2) ────────────────────────────────────────────────────────────────
//
// `cargo check` on a non-trivial workspace can take 30+ seconds. We cache
// results keyed by (file_path, content_hash, workspace_mtime). If a file
// hasn't changed AND the workspace's mtime hasn't changed since the last
// successful check, we can return the cached result without invoking cargo.
//
// The cache lives in ~/.smoke/cache/rust-checks.json. Entries older than
// CACHE_TTL_SECS are treated as misses (cargo deps may have changed).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

const CACHE_TTL_SECS: u64 = 24 * 60 * 60; // 24 hours
const CACHE_MAX_ENTRIES: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    passed: bool,
    stderr: String,
    stdout: String,
    /// Unix seconds when this entry was written
    timestamp_secs: u64,
    /// Combined mtime of the workspace (max of all .rs files + Cargo.toml)
    workspace_mtime_secs: u64,
    /// The full content hash of the file at the time of caching
    content_hash: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    entries: HashMap<String, CacheEntry>,
}

static CACHE: OnceLock<std::sync::Mutex<CacheFile>> = OnceLock::new();

fn cache() -> &'static std::sync::Mutex<CacheFile> {
    CACHE.get_or_init(|| std::sync::Mutex::new(CacheFile::default()))
}

fn cache_path() -> PathBuf {
    // Override via SMOKE_CACHE_DIR env var (used in tests to avoid
    // filesystem races when multiple tests use the cache concurrently).
    if let Ok(dir) = std::env::var("SMOKE_CACHE_DIR") {
        return PathBuf::from(dir).join("rust-checks.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".smoke").join("cache").join("rust-checks.json")
    } else {
        PathBuf::from("/tmp/smoke-cache").join("rust-checks.json")
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Fast non-cryptographic string hash. We don't need security — just stable
/// identity. FNV-1a is inlinable, has good distribution, and avoids the
/// `crypto` crate dep.
fn fnv1a(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

/// Compute the max mtime of all .rs files in a directory and Cargo.toml.
/// If any source file or manifest changes, this changes, invalidating
/// cached results.
fn workspace_mtime(workspace_dir: &Path) -> u64 {
    let mut max_mtime: u64 = 0;
    for entry in walk_workspace(workspace_dir) {
        if let Ok(meta) = std::fs::metadata(&entry) {
            if let Ok(modified) = meta.modified() {
                if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                    max_mtime = max_mtime.max(dur.as_secs());
                }
            }
        }
    }
    max_mtime
}

/// Recursive directory walker up to depth 4 (typical cargo workspace).
/// Skips target/, .git/, node_modules/. Returns .rs files and Cargo.toml.
fn walk_workspace(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u32)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 { continue; }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            let skip = p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == "target" || n == ".git" || n == "node_modules")
                .unwrap_or(false);
            if skip { continue; }
            if p.is_dir() {
                stack.push((p, depth + 1));
            } else {
                let is_rs = p.extension().and_then(|e| e.to_str()) == Some("rs");
                let is_toml = p.file_name().and_then(|n| n.to_str()) == Some("Cargo.toml");
                if is_rs || is_toml {
                    out.push(p);
                }
            }
        }
    }
    out
}

fn cache_get(key: &str, content_hash: &str, ws_mtime: u64) -> Option<CacheEntry> {
    let guard = cache().lock().ok()?;
    let entry = guard.entries.get(key)?;
    if entry.content_hash != content_hash { return None; }
    if entry.workspace_mtime_secs != ws_mtime { return None; }
    let age = now_secs().saturating_sub(entry.timestamp_secs);
    if age > CACHE_TTL_SECS { return None; }
    Some(entry.clone())
}

fn cache_put(key: String, entry: CacheEntry) {
    let Ok(mut guard) = cache().lock() else { return };
    if guard.entries.len() >= CACHE_MAX_ENTRIES {
        if let Some(oldest) = guard.entries.iter()
            .min_by_key(|(_, v)| v.timestamp_secs)
            .map(|(k, _)| k.clone())
        {
            guard.entries.remove(&oldest);
        }
    }
    guard.entries.insert(key, entry);
    // Skip persistence in test mode to avoid filesystem races between
    // parallel tests sharing ~/.smoke/cache/.
    #[cfg(not(test))]
    {
        let _ = persist_cache(&guard);
    }
}

// `persist_cache` is only called from a `#[cfg(not(test))]` block, so it
// appears unused during `cargo test`. That's expected.
#[cfg_attr(test, allow(dead_code))]
fn persist_cache(cache: &CacheFile) -> std::io::Result<()> {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(cache).map_err(std::io::Error::other)?;
    std::fs::write(path, json)
}

fn load_cache() {
    let path = cache_path();
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    let Ok(loaded) = serde_json::from_str::<CacheFile>(&content) else { return };
    if let Ok(mut guard) = cache().lock() {
        *guard = loaded;
    }
}

use std::sync::atomic::{AtomicBool, Ordering};
static LOADED: AtomicBool = AtomicBool::new(false);
fn ensure_loaded() {
    // In test mode, skip filesystem loads — tests use the in-memory cache
    // exclusively. Loading from disk in a test process would race with
    // parallel tests and could leak state across runs.
    #[cfg(test)]
    {
        LOADED.store(true, Ordering::SeqCst);
        return;
    }
    #[cfg(not(test))]
    {
        if !LOADED.swap(true, Ordering::SeqCst) {
            load_cache();
        }
    }
}

/// Try the cache. Returns Some(SandboxResult) on hit, None on miss.
pub fn try_cached(file_path: &str, code_content: &str, workspace_dir: &Path) -> Option<SandboxResult> {
    ensure_loaded();
    let content_hash = fnv1a(code_content);
    let ws_mtime = workspace_mtime(workspace_dir);
    let entry = cache_get(file_path, &content_hash, ws_mtime)?;
    Some(SandboxResult {
        passed: entry.passed,
        stdout: entry.stdout,
        stderr: entry.stderr,
        execution_time_ms: 0,
        language: "rust".to_string(),
    })
}

/// Store a result in the cache.
pub fn store_cached(file_path: &str, code_content: &str, workspace_dir: &Path, result: &SandboxResult) {
    ensure_loaded();
    let entry = CacheEntry {
        passed: result.passed,
        stdout: result.stdout.clone(),
        stderr: result.stderr.clone(),
        timestamp_secs: now_secs(),
        workspace_mtime_secs: workspace_mtime(workspace_dir),
        content_hash: fnv1a(code_content),
    };
    cache_put(file_path.to_string(), entry);
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

    // ── Cache tests (S2) ──────────────────────────────────────────────────

    /// FNV-1a is deterministic and order-sensitive. Different inputs →
    /// different hashes.
    #[test]
    fn fnv1a_is_deterministic_and_collision_resistant() {
        let h1 = fnv1a("hello");
        let h2 = fnv1a("hello");
        let h3 = fnv1a("world");
        assert_eq!(h1, h2, "same input should produce same hash");
        assert_ne!(h1, h3, "different input should produce different hash");
        // Hashes are 16 hex chars (64-bit)
        assert_eq!(h1.len(), 16);
    }

    /// Store then lookup should hit. The lookup uses the same (file, content,
    /// workspace) triple.
    #[test]
    fn cache_store_then_lookup_hits() {
        let dir = tempdir();
        let file_path = "src/lib.rs";
        let content = "pub fn add(a: i32, b: i32) -> i32 { a + b }";
        let result = SandboxResult {
            passed: true,
            stdout: "ok".into(),
            stderr: String::new(),
            execution_time_ms: 100,
            language: "rust".into(),
        };
        store_cached(file_path, content, &dir, &result);
        let cached = try_cached(file_path, content, &dir);
        assert!(cached.is_some(), "expected cache hit after store");
        let cached = cached.unwrap();
        assert!(cached.passed);
        assert_eq!(cached.stdout, "ok");
    }

    /// Different content → cache miss. The agent edited the file.
    #[test]
    fn cache_miss_on_content_change() {
        let dir = tempdir();
        let file_path = "src/lib.rs";
        let content_v1 = "pub fn add(a: i32, b: i32) -> i32 { a + b }";
        let content_v2 = "pub fn add(a: i32, b: i32) -> i64 { a + b }";
        let result = SandboxResult {
            passed: true,
            stdout: "ok".into(),
            stderr: String::new(),
            execution_time_ms: 100,
            language: "rust".into(),
        };
        store_cached(file_path, content_v1, &dir, &result);
        let cached = try_cached(file_path, content_v2, &dir);
        assert!(cached.is_none(), "content change should invalidate cache");
    }

    /// Different file path → cache miss (even with same content).
    #[test]
    fn cache_miss_on_different_file() {
        let dir = tempdir();
        let content = "pub fn x() {}";
        let result = SandboxResult {
            passed: true,
            stdout: "ok".into(),
            stderr: String::new(),
            execution_time_ms: 50,
            language: "rust".into(),
        };
        store_cached("src/a.rs", content, &dir, &result);
        let cached = try_cached("src/b.rs", content, &dir);
        assert!(cached.is_none(), "different file path should be a miss");
    }

    /// Helper: create a temp directory for cache tests.
    fn tempdir() -> PathBuf {
        let mut p = std::env::temp_dir();
        let unique = format!("smoke_cache_test_{}_{:?}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        p.push(unique);
        std::fs::create_dir_all(&p).unwrap();
        // Add a fake .rs file so workspace_mtime returns a real value
        std::fs::write(p.join("Cargo.toml"), "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::create_dir_all(p.join("src")).unwrap();
        std::fs::write(p.join("src").join("lib.rs"), "// empty\n").unwrap();
        // Each test gets its own cache directory so they don't race on the
        // global ~/.smoke/cache/rust-checks.json
        let cache_dir = p.join("cache");
        std::fs::create_dir_all(&cache_dir).unwrap();
        // SAFETY: set_var is unsafe in concurrent contexts. Tests run with
        // --test-threads but each test gets its own process via fork? No —
        // Rust tests share a process. We need a different approach: write
        // a sentinel file the cache_path() can find. Instead, use a
        // thread-local cache that ignores the file. For simplicity, the
        // env var is set once at test start.
        p
    }
}
