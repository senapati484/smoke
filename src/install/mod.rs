//! SMOKE install / uninstall / status
//!
//! Idempotent JSON-level registration and removal of SMOKE in every supported
//! AI tool config file.  All logic lives here so:
//!
//!   • `smoke install`   — writes hooks / MCP entry into each tool's config
//!   • `smoke uninstall` — removes only SMOKE's entries, leaves everything else intact
//!   • `smoke status`    — reads each config and prints ✓ / ✗ per tool
//!
//! No Python or Node.js is required — this is pure Rust using serde_json.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::PathBuf;

// ── Tool enum ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Tool {
    ClaudeCode,
    ClaudeDesktop,
    Windsurf,
    Cursor,
    Cline,
}

impl Tool {
    pub fn all() -> Vec<Tool> {
        vec![
            Tool::ClaudeCode,
            Tool::ClaudeDesktop,
            Tool::Windsurf,
            Tool::Cursor,
            Tool::Cline,
        ]
    }

    pub fn from_key(s: &str) -> Option<Tool> {
        match s.trim().to_lowercase().as_str() {
            "claude-code" | "claudecode" | "claude_code" => Some(Tool::ClaudeCode),
            "claude-desktop" | "claudedesktop" | "claude_desktop" => Some(Tool::ClaudeDesktop),
            "windsurf" => Some(Tool::Windsurf),
            "cursor" => Some(Tool::Cursor),
            "cline" | "roo" | "roo-code" => Some(Tool::Cline),
            _ => None,
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            Tool::ClaudeCode => "Claude Code",
            Tool::ClaudeDesktop => "Claude Desktop",
            Tool::Windsurf => "Windsurf",
            Tool::Cursor => "Cursor",
            Tool::Cline => "Cline / Roo Code",
        }
    }

    #[allow(dead_code)]
    pub fn key(&self) -> &'static str {
        match self {
            Tool::ClaudeCode => "claude-code",
            Tool::ClaudeDesktop => "claude-desktop",
            Tool::Windsurf => "windsurf",
            Tool::Cursor => "cursor",
            Tool::Cline => "cline",
        }
    }
}

// ── Parse the --tools flag ───────────────────────────────────────────────────

/// Parse a comma-separated tools string (or "all") into a `Vec<Tool>`.
/// Unknown entries are silently ignored and printed as warnings.
pub fn parse_tools(s: &str) -> Vec<Tool> {
    let s = s.trim().to_lowercase();
    if s == "all" {
        return Tool::all();
    }
    s.split(',')
        .filter_map(|part| {
            let p = part.trim();
            let t = Tool::from_key(p);
            if t.is_none() && !p.is_empty() {
                eprintln!("SMOKE: unknown tool '{}' — valid: claude-code, claude-desktop, windsurf, cursor, cline, all", p);
            }
            t
        })
        .collect()
}

// ── Binary path ──────────────────────────────────────────────────────────────

/// Returns the canonical path to the installed SMOKE binary.
pub fn installed_binary() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    if cfg!(target_os = "windows") {
        home.join(".smoke").join("bin").join("smoke.exe")
    } else {
        home.join(".smoke").join("bin").join("smoke")
    }
}

// ── Config file paths (per tool, can be multiple) ───────────────────────────

/// Returns the config file path(s) for a given tool.
/// Cline / Roo Code has two paths (one per VS Code extension).
pub fn config_paths(tool: &Tool) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));

    match tool {
        Tool::ClaudeCode => vec![home.join(".claude").join("settings.json")],

        Tool::ClaudeDesktop => {
            #[cfg(target_os = "macos")]
            return vec![home
                .join("Library")
                .join("Application Support")
                .join("Claude")
                .join("claude_desktop_config.json")];
            #[cfg(target_os = "windows")]
            {
                let appdata = std::env::var("APPDATA").unwrap_or_default();
                return vec![PathBuf::from(appdata)
                    .join("Claude")
                    .join("claude_desktop_config.json")];
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            return vec![home
                .join(".config")
                .join("Claude")
                .join("claude_desktop_config.json")];
        }

        Tool::Windsurf => vec![home
            .join(".codeium")
            .join("windsurf")
            .join("mcp_config.json")],

        Tool::Cursor => vec![home.join(".cursor").join("mcp.json")],

        Tool::Cline => {
            #[cfg(target_os = "macos")]
            {
                let base = home
                    .join("Library")
                    .join("Application Support")
                    .join("Code")
                    .join("User")
                    .join("globalStorage");
                return vec![
                    base.join("saoudrizwan.claude-dev")
                        .join("settings")
                        .join("cline_mcp_settings.json"),
                    base.join("rooveterinaryinc.roo-cline")
                        .join("settings")
                        .join("cline_mcp_settings.json"),
                ];
            }
            #[cfg(target_os = "windows")]
            {
                let appdata = std::env::var("APPDATA").unwrap_or_default();
                let base = PathBuf::from(appdata)
                    .join("Code")
                    .join("User")
                    .join("globalStorage");
                return vec![
                    base.join("saoudrizwan.claude-dev")
                        .join("settings")
                        .join("cline_mcp_settings.json"),
                    base.join("rooveterinaryinc.roo-cline")
                        .join("settings")
                        .join("cline_mcp_settings.json"),
                ];
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                let base = home
                    .join(".config")
                    .join("Code")
                    .join("User")
                    .join("globalStorage");
                return vec![
                    base.join("saoudrizwan.claude-dev")
                        .join("settings")
                        .join("cline_mcp_settings.json"),
                    base.join("rooveterinaryinc.roo-cline")
                        .join("settings")
                        .join("cline_mcp_settings.json"),
                ];
            }
        }
    }
}

// ── JSON helpers ─────────────────────────────────────────────────────────────

fn read_json(path: &std::path::Path) -> Value {
    if !path.exists() {
        return json!({});
    }
    let raw = std::fs::read_to_string(path).unwrap_or_default();
    serde_json::from_str(&raw).unwrap_or(json!({}))
}

fn write_json(path: &std::path::Path, value: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating directory {}", parent.display()))?;
    }
    let pretty = serde_json::to_string_pretty(value)
        .context("serialising JSON")?;
    std::fs::write(path, pretty + "\n")
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ── Register ─────────────────────────────────────────────────────────────────

/// Register SMOKE for a list of tools. Idempotent — safe to run multiple times.
pub fn register(tools: &[Tool]) -> Result<()> {
    let binary = installed_binary();
    let binary_str = binary.to_string_lossy().to_string();

    for tool in tools {
        let result = match tool {
            Tool::ClaudeCode => register_claude_code(&binary_str),
            _ => {
                let mut any_err = Ok(());
                for path in config_paths(tool) {
                    if let Err(e) = register_mcp(&path, &binary_str, tool) {
                        any_err = Err(e);
                    }
                }
                any_err
            }
        };

        match result {
            Ok(()) => println!(
                "  \x1b[32m✓\x1b[0m {} registered",
                tool.display_name()
            ),
            Err(e) => eprintln!(
                "  \x1b[31m✗\x1b[0m {} failed: {}",
                tool.display_name(),
                e
            ),
        }
    }
    Ok(())
}

fn register_claude_code(binary: &str) -> Result<()> {
    let path = config_paths(&Tool::ClaudeCode).remove(0);
    let mut data = read_json(&path);

    // Ensure hooks object exists
    if data.get("hooks").is_none() {
        data["hooks"] = json!({});
    }

    // Clean up any stale/duplicate smoke hook entries from old installs.
    // Previous versions registered under "Write|Edit" before MultiEdit was added,
    // which caused duplicate hook firings. Remove them before upserting the canonical form.
    remove_stale_smoke_hooks(&mut data, "PreToolUse", "Write|Edit", "smoke hook");
    remove_stale_smoke_hooks(&mut data, "PostToolUse", "Write|Edit", "smoke post-hook");

    // PreToolUse — find or create the Write|Edit|MultiEdit matcher entry
    let pre_hook = json!({
        "type": "command",
        "command": format!("{} hook", binary),
        "timeout": 30,
        "statusMessage": "SMOKE: verifying code..."
    });
    upsert_hook(&mut data, "PreToolUse", "Write|Edit|MultiEdit", pre_hook, "smoke hook");

    // PostToolUse — find or create the Write|Edit|MultiEdit matcher entry
    let post_hook = json!({
        "type": "command",
        "command": format!("{} post-hook", binary),
        "timeout": 30
    });
    upsert_hook(&mut data, "PostToolUse", "Write|Edit|MultiEdit", post_hook, "smoke post-hook");

    write_json(&path, &data)
        .with_context(|| format!("writing Claude Code settings to {}", path.display()))?;
    Ok(())
}

/// Remove any existing smoke hook entries registered under a stale/legacy matcher string.
/// This prevents duplicate hook firings when the matcher changed between versions.
/// Only removes the inner hook command; if the matcher entry's hooks array becomes
/// empty after removal, the entire matcher entry is dropped.
fn remove_stale_smoke_hooks(data: &mut Value, event: &str, stale_matcher: &str, identity: &str) {
    if let Some(arr) = data["hooks"][event].as_array_mut() {
        arr.retain_mut(|entry| {
            if entry.get("matcher").and_then(|m| m.as_str()) == Some(stale_matcher) {
                if let Some(inner) = entry["hooks"].as_array_mut() {
                    inner.retain(|h| {
                        !h.get("command")
                            .and_then(|c| c.as_str())
                            .unwrap_or("")
                            .contains(identity)
                    });
                    // Drop the whole matcher entry if no hooks remain
                    return !inner.is_empty();
                }
            }
            true
        });
    }
}

/// Upsert a hook command into a PreToolUse/PostToolUse matcher array,
/// replacing any existing smoke entry (idempotent).
fn upsert_hook(data: &mut Value, event: &str, matcher: &str, hook: Value, identity: &str) {
    let hooks_arr = data["hooks"][event]
        .as_array_mut()
        .cloned()
        .unwrap_or_default();

    let mut found = false;
    let mut updated: Vec<Value> = hooks_arr
        .into_iter()
        .map(|mut entry| {
            if entry.get("matcher").and_then(|m| m.as_str()) == Some(matcher) {
                found = true;
                // Remove any existing smoke hook from this entry's hooks array
                if let Some(inner) = entry["hooks"].as_array_mut() {
                    inner.retain(|h| {
                        h.get("command")
                            .and_then(|c| c.as_str())
                            .map(|c| !c.contains(identity))
                            .unwrap_or(true)
                    });
                    inner.push(hook.clone());
                }
            }
            entry
        })
        .collect();

    if !found {
        updated.push(json!({
            "matcher": matcher,
            "hooks": [hook]
        }));
    }

    data["hooks"][event] = Value::Array(updated);
}

fn register_mcp(path: &std::path::Path, binary: &str, tool: &Tool) -> Result<()> {
    let mut data = read_json(path);

    if data.get("mcpServers").is_none() {
        data["mcpServers"] = json!({});
    }

    let mut server = json!({
        "command": binary,
        "args": ["server"]
    });

    // Cline needs extra keys
    if matches!(tool, Tool::Cline) {
        server["disabled"] = json!(false);
        server["alwaysAllow"] = json!([]);
    }

    data["mcpServers"]["smoke"] = server;

    write_json(path, &data)
        .with_context(|| format!("writing MCP config to {}", path.display()))?;
    Ok(())
}

// ── Unregister ───────────────────────────────────────────────────────────────

/// Remove SMOKE from a list of tools. Idempotent — safe to run if not installed.
pub fn unregister(tools: &[Tool]) -> Result<()> {
    for tool in tools {
        let result = match tool {
            Tool::ClaudeCode => unregister_claude_code(),
            _ => {
                let mut any_err = Ok(());
                for path in config_paths(tool) {
                    if let Err(e) = unregister_mcp(&path) {
                        any_err = Err(e);
                    }
                }
                any_err
            }
        };

        match result {
            Ok(()) => println!(
                "  \x1b[32m✓\x1b[0m {} removed",
                tool.display_name()
            ),
            Err(e) => eprintln!(
                "  \x1b[31m✗\x1b[0m {} failed: {}",
                tool.display_name(),
                e
            ),
        }
    }
    Ok(())
}

fn unregister_claude_code() -> Result<()> {
    let path = config_paths(&Tool::ClaudeCode).remove(0);
    if !path.exists() {
        return Ok(()); // Nothing to remove
    }

    let mut data = read_json(&path);

    remove_hook(&mut data, "PreToolUse", "smoke hook");
    remove_hook(&mut data, "PostToolUse", "smoke post-hook");

    write_json(&path, &data)
        .with_context(|| format!("writing Claude Code settings to {}", path.display()))?;
    Ok(())
}

fn remove_hook(data: &mut Value, event: &str, identity: &str) {
    let arr = match data["hooks"][event].as_array_mut() {
        Some(a) => a,
        None => return,
    };

    for entry in arr.iter_mut() {
        if let Some(inner) = entry["hooks"].as_array_mut() {
            inner.retain(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .map(|c| !c.contains(identity))
                    .unwrap_or(true)
            });
        }
    }

    // Drop matcher entries that are now empty
    arr.retain(|entry| {
        entry["hooks"]
            .as_array()
            .map(|h| !h.is_empty())
            .unwrap_or(true)
    });
}

fn unregister_mcp(path: &std::path::Path) -> Result<()> {
    if !path.exists() {
        return Ok(()); // Nothing to remove
    }

    let mut data = read_json(path);

    if let Some(servers) = data["mcpServers"].as_object_mut() {
        servers.remove("smoke");
    }

    write_json(path, &data)
        .with_context(|| format!("writing MCP config to {}", path.display()))?;
    Ok(())
}

// ── Status ───────────────────────────────────────────────────────────────────

/// Print the registration status of every tool.
pub fn status() {
    let binary = installed_binary();

    println!("SMOKE installation status");
    println!("  Binary: {}", if binary.exists() {
        format!("\x1b[32m{}\x1b[0m (installed)", binary.display())
    } else {
        format!("\x1b[31m{}\x1b[0m (not found)", binary.display())
    });
    println!();

    for tool in Tool::all() {
        let registered = is_registered(&tool);
        let paths = config_paths(&tool);
        let path_str = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(", ");

        if registered {
            println!(
                "  \x1b[32m✓\x1b[0m  {:<20}  {}",
                tool.display_name(),
                path_str
            );
        } else {
            println!(
                "  \x1b[31m✗\x1b[0m  {:<20}  {} \x1b[2m(not registered)\x1b[0m",
                tool.display_name(),
                path_str
            );
        }
    }
}

fn is_registered(tool: &Tool) -> bool {
    match tool {
        Tool::ClaudeCode => {
            let path = config_paths(tool).remove(0);
            if !path.exists() { return false; }
            let data = read_json(&path);
            // Check if "smoke hook" appears in any PreToolUse hook command
            data["hooks"]["PreToolUse"]
                .as_array()
                .map(|entries| {
                    entries.iter().any(|entry| {
                        entry["hooks"]
                            .as_array()
                            .map(|hooks| {
                                hooks.iter().any(|h| {
                                    h.get("command")
                                        .and_then(|c| c.as_str())
                                        .map(|c| c.contains("smoke hook"))
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        }
        _ => {
            // For MCP tools: check the first path
            let paths = config_paths(tool);
            paths.iter().any(|path| {
                if !path.exists() { return false; }
                let data = read_json(path);
                data["mcpServers"]["smoke"].is_object()
            })
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn parse_tools_all() {
        let tools = parse_tools("all");
        assert_eq!(tools.len(), Tool::all().len());
    }

    #[test]
    fn parse_tools_subset() {
        let tools = parse_tools("claude-code,windsurf");
        assert_eq!(tools.len(), 2);
        assert!(tools.contains(&Tool::ClaudeCode));
        assert!(tools.contains(&Tool::Windsurf));
    }

    #[test]
    fn parse_tools_unknown_skipped() {
        let tools = parse_tools("claude-code,bogus-tool");
        assert_eq!(tools.len(), 1);
        assert!(tools.contains(&Tool::ClaudeCode));
    }

    #[test]
    fn register_unregister_mcp_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        let binary = "/usr/local/bin/smoke";

        register_mcp(&path, binary, &Tool::Cursor).unwrap();

        let data = read_json(&path);
        assert_eq!(data["mcpServers"]["smoke"]["command"].as_str(), Some(binary));

        unregister_mcp(&path).unwrap();
        let data2 = read_json(&path);
        assert!(data2["mcpServers"]["smoke"].is_null());
    }

    #[test]
    fn register_mcp_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("mcp.json");
        let binary = "/usr/local/bin/smoke";

        register_mcp(&path, binary, &Tool::Cursor).unwrap();
        register_mcp(&path, binary, &Tool::Cursor).unwrap(); // second call

        let data = read_json(&path);
        // mcpServers should still have exactly one "smoke" entry
        assert!(data["mcpServers"]["smoke"].is_object());
        let servers = data["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
    }

    #[test]
    fn register_unregister_claude_code_roundtrip() {
        // We can't easily override the real home dir, so test the hook
        // manipulation helpers directly on in-memory Values.
        let mut data = serde_json::json!({});
        data["hooks"] = serde_json::json!({});

        let hook = serde_json::json!({
            "type": "command",
            "command": "/test/smoke hook",
            "timeout": 10
        });
        upsert_hook(&mut data, "PreToolUse", "Write|Edit|MultiEdit", hook, "smoke hook");

        // Verify it was inserted
        let entries = data["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        let inner = entries[0]["hooks"].as_array().unwrap();
        assert_eq!(inner.len(), 1);

        // Idempotent second insert — should not duplicate
        let hook2 = serde_json::json!({
            "type": "command",
            "command": "/test/smoke hook",
            "timeout": 10
        });
        upsert_hook(&mut data, "PreToolUse", "Write|Edit|MultiEdit", hook2, "smoke hook");
        let entries2 = data["hooks"]["PreToolUse"].as_array().unwrap();
        let inner2 = entries2[0]["hooks"].as_array().unwrap();
        assert_eq!(inner2.len(), 1, "idempotent: no duplicate hooks");

        // Remove
        remove_hook(&mut data, "PreToolUse", "smoke hook");
        let entries3 = data["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(entries3.is_empty(), "hook removed, empty matcher pruned");
    }

    #[test]
    fn cline_registration_has_extra_keys() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("cline_mcp_settings.json");
        register_mcp(&path, "/test/smoke", &Tool::Cline).unwrap();
        let data = read_json(&path);
        assert_eq!(data["mcpServers"]["smoke"]["disabled"].as_bool(), Some(false));
        assert!(data["mcpServers"]["smoke"]["alwaysAllow"].is_array());
    }
}
