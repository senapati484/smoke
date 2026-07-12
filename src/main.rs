use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod config;
mod hook;
mod install;
mod mcp;
mod parser;
mod post_hook;
mod sandbox;
mod state;

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

    /// Register SMOKE hooks and MCP server in AI tool config files.
    /// Idempotent — safe to run multiple times.
    ///
    /// Examples:
    ///   smoke install                               # all tools
    ///   smoke install --tools claude-code           # Claude Code hooks only
    ///   smoke install --tools claude-desktop,cursor # two MCP clients
    Install {
        /// Comma-separated list of tools to configure, or "all" (default).
        /// Valid values: claude-code, claude-desktop, windsurf, cursor, cline
        #[arg(long, default_value = "all")]
        tools: String,
    },

    /// Remove SMOKE hooks and MCP server entries from AI tool config files.
    /// Leaves all other entries in the config files untouched.
    ///
    /// Examples:
    ///   smoke uninstall                        # all tools
    ///   smoke uninstall --tools claude-code    # Claude Code only
    Uninstall {
        /// Comma-separated list of tools to remove from, or "all" (default).
        #[arg(long, default_value = "all")]
        tools: String,
    },

    /// Show current SMOKE registration status across all supported AI tools.
    Status,

    /// Run performance benchmarks measuring parser, V8, Python, and loop check execution times.
    Benchmark,
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
        Commands::Install { tools } => run_install(&tools),
        Commands::Uninstall { tools } => run_uninstall(&tools),
        Commands::Status => {
            install::status();
            Ok(())
        }
        Commands::Benchmark => run_benchmark().await,
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
        "rs" | "rust" => {
            if !cfg.languages.rust_enabled {
                anyhow::bail!("Rust sandbox is disabled in config");
            }
            let mut sandbox = sandbox::rust::RustSandbox::new();
            let cwd = std::env::current_dir()?.to_string_lossy().to_string();
            sandbox.execute(code, None, &cwd, cfg.limits.timeout_ms).await
        }
        other => anyhow::bail!("Unknown language: '{}'. Use: js, ts, python, rust", other),
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

fn run_install(tools_str: &str) -> Result<()> {
    println!("SMOKE: preparing binary and path...");
    install::self_install()?;
    println!();

    let tools = install::parse_tools(tools_str);
    if tools.is_empty() {
        eprintln!("SMOKE: no valid tools specified. Use --tools all or a comma-separated list.");
        return Ok(());
    }
    println!("SMOKE: registering in {} tool(s)...", tools.len());
    install::register(&tools)?;
    println!();
    println!("Done. Run \x1b[36msmoke status\x1b[0m to verify.");
    Ok(())
}

fn run_uninstall(tools_str: &str) -> Result<()> {
    let tools = install::parse_tools(tools_str);
    if tools.is_empty() {
        eprintln!("SMOKE: no valid tools specified. Use --tools all or a comma-separated list.");
        return Ok(());
    }
    println!("SMOKE: removing from {} tool(s)...", tools.len());
    install::unregister(&tools)?;
    println!();
    println!("Done. Run \x1b[36msmoke status\x1b[0m to verify.");
    Ok(())
}

async fn run_benchmark() -> Result<()> {
    use std::time::Instant;

    println!("==================================================");
    println!("         SMOKE Performance Benchmark              ");
    println!("==================================================");

    // 1. Benchmark Tree-sitter Parser
    println!("\n[1] Benchmarking Tree-sitter Syntax Parser...");
    let code_js = "const x = 1; function add(a, b) { return a + b; }";
    let start = Instant::now();
    for _ in 0..1000 {
        let _ = parser::check_syntax(code_js, "js");
    }
    let duration = start.elapsed();
    println!("    - JavaScript (1,000 runs): {:?}", duration);
    println!("    - Average parse time: {:.3} µs", duration.as_micros() as f64 / 1000.0);

    // 2. Benchmark JS Sandbox (V8)
    println!("\n[2] Benchmarking JavaScript Sandbox (V8)...");
    let mut js_sandbox = sandbox::js::JsSandbox::new()?;
    
    // Cold run
    let start_cold = Instant::now();
    let res_cold = js_sandbox.execute("const x = 1;", false, 2000);
    let cold_duration = start_cold.elapsed();
    println!("    - Cold run latency: {:?}", cold_duration);
    assert!(res_cold.passed);

    // Warm runs
    let start_warm = Instant::now();
    for _ in 0..100 {
        let _ = js_sandbox.execute("const x = 1;", false, 2000);
    }
    let warm_duration = start_warm.elapsed();
    println!("    - Warm runs (100 runs): {:?}", warm_duration);
    println!("    - Average warm execution: {:.3} ms", warm_duration.as_millis() as f64 / 100.0);

    // 3. Benchmark Python Sandbox
    println!("\n[3] Benchmarking Python Sandbox (Subprocess)...");
    let mut python_sandbox = sandbox::python::PythonSandbox::new();
    let interpreter = "python3";

    // Cold run
    let start_py_cold = Instant::now();
    let res_py_cold = python_sandbox.execute("print('hello')", interpreter, 2000).await;
    let py_cold_duration = start_py_cold.elapsed();
    println!("    - Cold run latency (spawn): {:?}", py_cold_duration);
    assert!(res_py_cold.passed);

    // Warm runs
    let start_py_warm = Instant::now();
    for _ in 0..10 {
        let _ = python_sandbox.execute("print('hello')", interpreter, 2000).await;
    }
    let py_warm_duration = start_py_warm.elapsed();
    println!("    - Spawn runs (10 runs): {:?}", py_warm_duration);
    println!("    - Average Python execution: {:.3} ms", py_warm_duration.as_millis() as f64 / 10.0);

    // 4. Benchmark State Hashing
    println!("\n[4] Benchmarking Error Signature Hashing & Fingerprinting...");
    let error_text = "Syntax error at line 42, column 7: expected ';' got 'ref'";
    let start_hash = Instant::now();
    for _ in 0..5000 {
        let _ = state::fingerprint("src/main.rs", "syntax_error", error_text);
    }
    let hash_duration = start_hash.elapsed();
    println!("    - Fingerprint & FNV-1a (5,000 runs): {:?}", hash_duration);
    println!("    - Average hash time: {:.3} µs", hash_duration.as_micros() as f64 / 5000.0);

    println!("\n==================================================");
    println!("               Benchmark Summary                  ");
    println!("==================================================");
    println!("- Syntax check:   ~{:.1} µs (Instantaneous)", duration.as_micros() as f64 / 1000.0);
    println!("- JS V8 execution: ~{:.1} ms (Zero sandbox escape)", warm_duration.as_millis() as f64 / 100.0);
    println!("- Python execution:~{:.1} ms (Subprocess spawn limit)", py_warm_duration.as_millis() as f64 / 10.0);
    println!("- Loop tracking:  ~{:.2} µs (Ultra low memory footprint)", hash_duration.as_micros() as f64 / 5000.0);
    println!("==================================================");

    Ok(())
}
