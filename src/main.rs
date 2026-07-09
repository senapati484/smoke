use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod config;
mod hook;
mod mcp;
mod parser;
mod post_hook;
mod sandbox;

use config::Config;

// ── CLI definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "smoke",
    version,
    about = "Write. Run. Know. — A PreToolUse hook that executes AI-generated code before writes are allowed.",
    long_about = "SMOKE sits between the agent deciding to write code and the file actually \
                  being written. It runs that code in a sandbox and returns real stdout/stderr \
                  so the agent finds out about bugs the same second it introduces them."
)]
struct Cli {
    /// Path to a custom config file (overrides .smoke.toml and ~/.config/smoke/smoke.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// PreToolUse hook entry point — reads Claude Code's hook JSON from stdin.
    /// Primary integration path. Register in .claude/settings.json.
    Hook,

    /// PostToolUse hook entry point — auto-runs test files after a successful write.
    /// Complementary to `hook`. Register in .claude/settings.json PostToolUse section.
    PostHook,

    /// MCP server entry point — exposes smoke_verify as an MCP tool over stdio.
    /// Production integration path. Register in .mcp.json.
    Server,

    /// Standalone CLI for development and debugging.
    /// NOT agent-facing — use this to test sandbox behavior locally.
    Test {
        /// Code to execute
        #[arg(long)]
        code: String,

        /// Language: js, ts, javascript, typescript, python, py
        #[arg(long)]
        lang: String,
    },

    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Write a smoke.toml with default values and inline comments to the current directory
    Init,
    /// Print the currently active configuration (after all merge layers)
    Show,
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Load config once — all subcommands share the same loaded config.
    // Config load failure falls back to defaults (never crashes).
    let cfg = Config::load(cli.config.as_deref());

    let result = match cli.command {
        Commands::Hook => hook::run().await,
        Commands::PostHook => post_hook::run().await,
        Commands::Server => mcp::run().await,
        Commands::Test { code, lang } => run_test(&cfg, &code, &lang).await,
        Commands::Config { action } => run_config(action),
    };

    if let Err(e) = result {
        eprintln!("SMOKE error: {}", e);
        std::process::exit(1);
    }
}

// ── Subcommand implementations ────────────────────────────────────────────────

async fn run_test(cfg: &Config, code: &str, lang: &str) -> Result<()> {
    let result = match lang.to_lowercase().as_str() {
        "js" | "javascript" => {
            if !cfg.languages.js_enabled {
                anyhow::bail!("JavaScript sandbox is disabled in config");
            }
            let mut sandbox = sandbox::js::JsSandbox::new()?;
            sandbox.execute(code, false, cfg.limits.timeout_ms)
        }
        "ts" | "typescript" => {
            if !cfg.languages.ts_enabled {
                anyhow::bail!("TypeScript sandbox is disabled in config");
            }
            let mut sandbox = sandbox::js::JsSandbox::new()?;
            let mut res = sandbox.execute(code, true, cfg.limits.timeout_ms);
            res.language = "typescript".to_string();
            res
        }
        "py" | "python" => {
            if !cfg.languages.python_enabled {
                anyhow::bail!("Python sandbox is disabled in config");
            }
            let mut sandbox = sandbox::python::PythonSandbox::new();
            sandbox.execute(code, &cfg.python.interpreter, cfg.limits.timeout_ms).await
        }
        other => anyhow::bail!("Unknown language: '{}'. Use: js, ts, python", other),
    };

    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

fn run_config(action: ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Init => {
            let path = std::env::current_dir()?.join(".smoke.toml");
            config::write_default_config(&path)?;
        }
        ConfigAction::Show => {
            let cfg = Config::load(None);
            println!("{}", toml::to_string_pretty(&cfg)?);
        }
    }
    Ok(())
}
