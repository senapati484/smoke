use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Persisted once per Claude Code session at ~/.smoke/state/<session_id>.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionState {
    /// key = fingerprint, value = attempt record
    pub attempts: HashMap<String, AttemptRecord>,
    /// unix seconds, updated on every write — used for GC
    pub last_touched: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    pub count: u32,
    pub first_seen: u64,     // unix seconds
    pub last_seen: u64,      // unix seconds
    pub file_path: String,
    pub last_error_snippet: String, // truncated, for debugging / display only
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscalationLevel {
    Normal,
    Notice,
    Escalate,
}

impl SessionState {
    pub fn load(session_id: &str) -> Self {
        if session_id.trim().is_empty() {
            return Self::default();
        }
        let path = state_file_path(session_id);
        if !path.exists() {
            return Self::default();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                eprintln!("SMOKE: warning — session state file corrupt, resetting: {}", e);
                Self::default()
            }),
            Err(e) => {
                eprintln!("SMOKE: warning — could not read session state: {}", e);
                Self::default()
            }
        }
    }

    pub fn save(&self, session_id: &str) -> anyhow::Result<()> {
        if session_id.trim().is_empty() {
            return Ok(());
        }
        let path = state_file_path(session_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut temp_path = path.clone();
        temp_path.set_extension("tmp");

        let mut data = self.clone();
        data.last_touched = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let json = serde_json::to_string_pretty(&data)?;
        std::fs::write(&temp_path, json)?;
        std::fs::rename(temp_path, path)?;
        Ok(())
    }

    pub fn record_failure(&mut self, fingerprint: &str, file_path: &str, error_snippet: &str, window_minutes: u64) -> u32 {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let record = self.attempts.entry(fingerprint.to_string()).or_insert_with(|| AttemptRecord {
            count: 0,
            first_seen: now,
            last_seen: now,
            file_path: file_path.to_string(),
            last_error_snippet: String::new(),
        });

        // Check if the last attempt was within the window
        let elapsed_seconds = now.saturating_sub(record.last_seen);
        if record.count > 0 && elapsed_seconds > window_minutes * 60 {
            // Out of window, reset count
            record.count = 1;
            record.first_seen = now;
        } else {
            record.count += 1;
        }
        
        record.last_seen = now;
        
        let truncated_err = if error_snippet.len() > 300 {
            format!("{}...", &error_snippet[..300])
        } else {
            error_snippet.to_string()
        };
        record.last_error_snippet = truncated_err;

        record.count
    }

    pub fn record_success(&mut self, file_path: &str) {
        // Clear all fingerprints that match this file_path
        self.attempts.retain(|_, record| record.file_path != file_path);
    }
}

pub fn state_file_path(session_id: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    // Sanitize session_id to avoid path traversal (remove slashes, dots)
    let sanitized_id: String = session_id
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    home.join(".smoke").join("state").join(format!("{}.json", sanitized_id))
}

pub fn gc_state_dir(max_age_secs: u64) -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let state_dir = home.join(".smoke").join("state");
    if !state_dir.exists() {
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    for entry in std::fs::read_dir(state_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(metadata) = path.metadata() {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(duration) = modified.duration_since(SystemTime::UNIX_EPOCH) {
                        let age = now.saturating_sub(duration.as_secs());
                        if age > max_age_secs {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn fnv1a(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

pub fn fingerprint(file_path: &str, error_category: &str, raw_error: &str) -> String {
    let normalized = normalize_error_message(raw_error);
    let combined = format!("{}::{}::{}", file_path, error_category, normalized);
    fnv1a(&combined)
}

pub fn normalize_error_message(raw: &str) -> String {
    let mut s = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Strip :\d+:\d+
        if chars[i] == ':' && i + 1 < chars.len() && chars[i+1].is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            if j < chars.len() && chars[j] == ':' && j + 1 < chars.len() && chars[j+1].is_ascii_digit() {
                j += 1;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
                continue;
            }
        }

        // Strip :\d+
        if chars[i] == ':' && i + 1 < chars.len() && chars[i+1].is_ascii_digit() {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
            continue;
        }

        // Check if we are at "line "
        if i + 5 <= chars.len() && chars[i..i+5].iter().collect::<String>().to_lowercase() == "line " {
            let mut j = i + 5;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
            continue;
        }

        // Check if we are at "column " or "col "
        if i + 7 <= chars.len() && chars[i..i+7].iter().collect::<String>().to_lowercase() == "column " {
            let mut j = i + 7;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
            continue;
        }
        if i + 4 <= chars.len() && chars[i..i+4].iter().collect::<String>().to_lowercase() == "col " {
            let mut j = i + 4;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
            continue;
        }

        s.push(chars[i]);
        i += 1;
    }

    // Now: Strip quoted identifiers/string literals (replace '...' / "..." with placeholder)
    let mut s2 = String::new();
    let chars2: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars2.len() {
        if chars2[i] == '\'' {
            s2.push('\'');
            s2.push_str("...");
            s2.push('\'');
            i += 1;
            while i < chars2.len() && chars2[i] != '\'' {
                i += 1;
            }
            if i < chars2.len() {
                i += 1;
            }
            continue;
        }
        if chars2[i] == '"' {
            s2.push('"');
            s2.push_str("...");
            s2.push('"');
            i += 1;
            while i < chars2.len() && chars2[i] != '"' {
                i += 1;
            }
            if i < chars2.len() {
                i += 1;
            }
            continue;
        }
        s2.push(chars2[i]);
        i += 1;
    }

    // Pass 3: Strip hex memory addresses (0x[0-9a-fA-F]+)
    let s3 = strip_hex_addresses(&s2);

    // Pass 4: Strip absolute file paths (/tmp/…, /var/…, /Users/…, /home/…, C:\…)
    let s4 = strip_absolute_paths(&s3);

    // Pass 5: Strip smoke temp filenames that leak into error messages
    let s5 = strip_smoke_temp_names(&s4);

    // Pass 6: Strip Unix epoch timestamps (standalone 10-digit numbers)
    let s6 = strip_epoch_timestamps(&s5);

    // Pass 7: Strip process IDs (pid=\d+, pid \d+, [pid \d+])
    let s7 = strip_pids(&s6);

    // Pass 8: Strip numeric values in "expected N" / "got N" patterns
    let s8 = strip_expected_got_numbers(&s7);

    // Lowercase, collapse whitespace, truncate to 200 chars.
    let mut collapsed = String::new();
    let mut last_was_space = false;
    for c in s8.to_lowercase().chars() {
        if c.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
                last_was_space = true;
            }
        } else {
            collapsed.push(c);
            last_was_space = false;
        }
    }

    let trimmed = collapsed.trim().to_string();
    if trimmed.len() > 200 {
        trimmed[..200].to_string()
    } else {
        trimmed
    }
}

// ── Normalization helpers (called by normalize_error_message) ─────────────────

/// Replace `0x[0-9a-fA-F]+` with `0x...`.
fn strip_hex_addresses(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'0' && (bytes[i + 1] == b'x' || bytes[i + 1] == b'X') {
            let start = i + 2;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > start {
                // Only replace if there were actual hex digits
                out.push_str("0x...");
                i = j;
                continue;
            }
        }
        out.push(s[i..].chars().next().unwrap_or('\0'));
        i += s[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
    }
    out
}

/// Replace absolute file paths with `<path>`.
/// Handles Unix paths starting with `/tmp/`, `/var/`, `/Users/`, `/home/`, `/private/`
/// and Windows paths starting with a drive letter `C:\` or `D:\`.
fn strip_absolute_paths(s: &str) -> String {
    let prefixes: &[&str] = &["/tmp/", "/var/", "/Users/", "/home/", "/private/", "/opt/", "/root/"];
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Unix prefix match
        let remaining: String = chars[i..].iter().collect();
        let mut matched = false;
        for prefix in prefixes {
            if remaining.starts_with(prefix) {
                // Consume until whitespace, comma, colon (before digit), closing paren/bracket, or end
                let mut j = i + prefix.chars().count();
                while j < chars.len() {
                    let c = chars[j];
                    if c.is_whitespace() || c == ',' || c == ')' || c == ']' || c == '\'' || c == '"' {
                        break;
                    }
                    // Stop at `:N` (line number) — keep the colon in output
                    if c == ':' && j + 1 < chars.len() && chars[j + 1].is_ascii_digit() {
                        break;
                    }
                    j += 1;
                }
                out.push_str("<path>");
                i = j;
                matched = true;
                break;
            }
        }
        if matched {
            continue;
        }
        // Windows drive path: `C:\` or `D:\`
        if i + 2 < chars.len()
            && chars[i].is_ascii_alphabetic()
            && chars[i + 1] == ':'
            && (chars[i + 2] == '\\' || chars[i + 2] == '/')
        {
            let mut j = i + 3;
            while j < chars.len() {
                let c = chars[j];
                if c.is_whitespace() || c == ',' || c == ')' || c == ']' || c == '\'' || c == '"' {
                    break;
                }
                if c == ':' && j + 1 < chars.len() && chars[j + 1].is_ascii_digit() {
                    break;
                }
                j += 1;
            }
            out.push_str("<path>");
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Replace smoke internal temp filenames with `<tempfile>`.
/// Covers: `smoke_verify.ts`, `smoke_verify.js`, `smoke_XXXXXXXX.py`, `smoke_XXXXXXXX.rs`
fn strip_smoke_temp_names(s: &str) -> String {
    // Simple prefix-based replacement: any word starting with "smoke_verify" or "smoke_"
    // followed by alphanumeric chars and an extension we care about.
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let remaining: String = chars[i..].iter().collect();
        if remaining.starts_with("smoke_") {
            // Consume the whole filename token
            let mut j = i + 6; // past "smoke_"
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_' || chars[j] == '.') {
                j += 1;
            }
            out.push_str("<tempfile>");
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Replace standalone 10-digit Unix epoch timestamps with `<ts>`.
/// A "standalone" number is one not immediately preceded or followed by another digit.
fn strip_epoch_timestamps(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            // Count consecutive digits
            let start = i;
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let digit_count = j - start;
            // Check boundaries
            let before_ok = start == 0 || !chars[start - 1].is_ascii_digit();
            let after_ok = j >= chars.len() || !chars[j].is_ascii_digit();
            // Only replace 10-digit numbers (Unix epoch range: ~1e9 to ~2e9)
            if digit_count == 10 && before_ok && after_ok {
                // Verify it starts with 1 or 2 (valid epoch range)
                if chars[start] == '1' || chars[start] == '2' {
                    out.push_str("<ts>");
                    i = j;
                    continue;
                }
            }
            // Not an epoch timestamp — emit as-is
            for c in &chars[start..j] {
                out.push(*c);
            }
            i = j;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Replace PID patterns with `pid=<N>`.
/// Handles: `pid=\d+`, `pid \d+`, `[pid \d+]`
fn strip_pids(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    'outer: while i < chars.len() {
        let remaining: String = chars[i..].iter().collect();
        let lower = remaining.to_lowercase();
        // Match `pid=\d+` or `pid \d+`
        for sep in ["pid=", "pid "] {
            if lower.starts_with(sep) {
                let after_prefix = i + sep.len();
                // Optional `[` before digits
                let digit_start = if after_prefix < chars.len() && chars[after_prefix] == '[' {
                    after_prefix + 1
                } else {
                    after_prefix
                };
                if digit_start < chars.len() && chars[digit_start].is_ascii_digit() {
                    let mut j = digit_start;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                    // Consume optional closing `]`
                    if j < chars.len() && chars[j] == ']' {
                        j += 1;
                    }
                    out.push_str("pid=<N>");
                    i = j;
                    continue 'outer;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Replace numeric values in `expected N` / `got N` patterns with a placeholder.
/// e.g. `expected 5, got 3` → `expected N, got N`
fn strip_expected_got_numbers(s: &str) -> String {
    let keywords: &[&str] = &["expected ", "got ", "found ", "actual "];
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let remaining: String = chars[i..].iter().collect();
        let lower = remaining.to_lowercase();
        let mut matched = false;
        for kw in keywords {
            if lower.starts_with(kw) {
                let after_kw = i + kw.len();
                if after_kw < chars.len() && chars[after_kw].is_ascii_digit() {
                    // Emit the keyword
                    out.push_str(&chars[i..i + kw.len()].iter().collect::<String>());
                    // Skip the number
                    let mut j = after_kw;
                    while j < chars.len() && chars[j].is_ascii_digit() {
                        j += 1;
                    }
                    out.push('N');
                    i = j;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

pub fn escalation_for(count: u32, warn_threshold: u32, escalate_threshold: u32) -> EscalationLevel {
    if count >= escalate_threshold {
        EscalationLevel::Escalate
    } else if count >= warn_threshold {
        EscalationLevel::Notice
    } else {
        EscalationLevel::Normal
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_hex_addresses() {
        // Two errors differing only by heap address should get the same fingerprint
        let e1 = "TypeError: Cannot read property of null (0x7f3a4b0c8d00)";
        let e2 = "TypeError: Cannot read property of null (0x7f3a4b0c8001)";
        assert_eq!(normalize_error_message(e1), normalize_error_message(e2),
            "hex addresses must normalize identically");

        // Should NOT replace non-hex tokens that happen to start with 0x (edge: short)
        let e3 = "value 0x is fine";
        assert!(normalize_error_message(e3).contains("0x"),
            "bare '0x' with no hex digits after should be left alone");
    }

    #[test]
    fn test_normalize_absolute_paths() {
        let e1 = "/tmp/smoke_abc123.py: SyntaxError at line 1";
        let e2 = "/tmp/smoke_xyz999.py: SyntaxError at line 1";
        assert_eq!(normalize_error_message(e1), normalize_error_message(e2),
            "different temp paths in same error must normalize identically");

        let e3 = "error in /Users/alice/project/foo.py at line 5";
        let e4 = "error in /Users/bob/project/foo.py at line 5";
        assert_eq!(normalize_error_message(e3), normalize_error_message(e4),
            "/Users/<user>/... paths should normalize identically");
    }

    #[test]
    fn test_normalize_smoke_temp_names() {
        let e1 = "ReferenceError: foo is not defined\n  at smoke_verify.ts:3:5";
        let e2 = "ReferenceError: foo is not defined\n  at smoke_verify.ts:99:12";
        assert_eq!(normalize_error_message(e1), normalize_error_message(e2),
            "smoke temp filename with differing line/col must normalize identically");

        let e3 = "File smoke_a1b2c3d4.py, line 1, in <module>";
        let e4 = "File smoke_99887766.py, line 1, in <module>";
        assert_eq!(normalize_error_message(e3), normalize_error_message(e4),
            "smoke Python temp filenames must normalize identically");
    }

    #[test]
    fn test_normalize_epoch_timestamps() {
        let e1 = "Process exited at 1720000001";
        let e2 = "Process exited at 1720099999";
        assert_eq!(normalize_error_message(e1), normalize_error_message(e2),
            "10-digit epoch timestamps must normalize identically");

        // Short numbers should not be affected
        let e3 = "error code 42";
        assert!(normalize_error_message(e3).contains("42") || normalize_error_message(e3).contains("n"),
            "small numbers should not be stripped as timestamps");
    }

    #[test]
    fn test_normalize_pids() {
        let e1 = "child process pid=12345 exited with code 1";
        let e2 = "child process pid=99999 exited with code 1";
        assert_eq!(normalize_error_message(e1), normalize_error_message(e2),
            "different PIDs must normalize identically");
    }

    #[test]
    fn test_normalize_expected_got_numbers() {
        let e1 = "AssertionError: expected 5, got 3";
        let e2 = "AssertionError: expected 10, got 7";
        assert_eq!(normalize_error_message(e1), normalize_error_message(e2),
            "different expected/got values must normalize identically");

        // Ensure non-numeric got/expected are untouched
        let e3 = "expected boolean, got string";
        let n3 = normalize_error_message(e3);
        assert!(n3.contains("boolean") || n3.contains("string"),
            "expected/got with non-numeric values should keep them");
    }

    #[test]
    fn test_normalize_error_message() {
        let err1 = "Syntax error at line 42, column 7: expected ';' got 'ref'";
        let err2 = "Syntax error at line 105, column 12: expected ';' got 'ref'";
        assert_eq!(normalize_error_message(err1), normalize_error_message(err2));

        let err3 = "error[E0308]: mismatched types at src/main.rs:15:35";
        let err4 = "error[E0308]: mismatched types at src/main.rs:188:99";
        assert_eq!(normalize_error_message(err3), normalize_error_message(err4));

        let err5 = "cannot find name 'foo' in this scope";
        let err6 = "cannot find name 'bar' in this scope";
        assert_eq!(normalize_error_message(err5), normalize_error_message(err6));

        let err7 = "Uncaught error: \"database connections failed\" at index:4:5";
        let err8 = "Uncaught error: \"connection timeout\" at index:129:10";
        assert_eq!(normalize_error_message(err7), normalize_error_message(err8));
    }

    #[test]
    fn test_fingerprint() {
        let f1 = fingerprint("src/main.rs", "syntax_error", "Error at line 42, column 7: expected ';' got 'ref'");
        let f2 = fingerprint("src/main.rs", "syntax_error", "Error at line 105, column 12: expected ';' got 'ref'");
        assert_eq!(f1, f2);

        let f3 = fingerprint("src/main.rs", "runtime_error", "Error at line 42, column 7: expected ';' got 'ref'");
        assert_ne!(f1, f3); // Different category
    }

    #[test]
    fn test_escalation_for() {
        assert_eq!(escalation_for(1, 2, 3), EscalationLevel::Normal);
        assert_eq!(escalation_for(2, 2, 3), EscalationLevel::Notice);
        assert_eq!(escalation_for(3, 2, 3), EscalationLevel::Escalate);
        assert_eq!(escalation_for(4, 2, 3), EscalationLevel::Escalate);
    }

    #[test]
    fn test_session_state_lifecycle() {
        let _temp_dir = tempfile::TempDir::new().unwrap();
        let f1 = "fingerprint1";
        
        let mut state = SessionState::default();
        
        // Window 30 minutes
        let count = state.record_failure(f1, "src/main.rs", "Error 1", 30);
        assert_eq!(count, 1);
        
        let count2 = state.record_failure(f1, "src/main.rs", "Error 2", 30);
        assert_eq!(count2, 2);

        state.record_success("src/main.rs");
        assert!(state.attempts.get(f1).is_none());
    }
}
