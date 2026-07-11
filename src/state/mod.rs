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

    // Lowercase, collapse whitespace, truncate to 200 chars.
    let mut collapsed = String::new();
    let mut last_was_space = false;
    for c in s2.to_lowercase().chars() {
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
