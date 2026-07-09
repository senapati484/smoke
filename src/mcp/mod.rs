// Phase 7: MCP server (production integration path)
// Exposes smoke_verify as an MCP tool over stdio using rmcp 0.16.
// Reuses JsSandbox and PythonSandbox — no new sandbox logic here.

use crate::config::Config;
use crate::sandbox::js::JsSandbox;
use crate::sandbox::python::PythonSandbox;
use crate::sandbox::SandboxResult;
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::{Json, Parameters}},
    model::{ServerCapabilities, ServerInfo, Implementation},
    tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use rmcp::schemars;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SmokeVerifyRequest {
    /// Code content to execute
    pub code: String,
    /// Target language: "javascript", "typescript", or "python"
    pub language: String,
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SmokeServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "smoke".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

pub struct SmokeServer {
    tool_router: ToolRouter<Self>,
}

impl SmokeServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl SmokeServer {
    #[tool(
        name = "smoke_verify",
        description = "Executes JS/TS or Python code in a local sandbox and returns stdout/stderr. JS/TS is fully sandboxed by V8 (no filesystem or network access). Python is process-isolated with resource limits and a partial seccomp filter — not a full sandbox. Do not use for untrusted third-party code."
    )]
    pub async fn smoke_verify(
        &self,
        params: Parameters<SmokeVerifyRequest>,
    ) -> Result<Json<SandboxResult>, String> {
        let req = params.0;
        let cfg = Config::load(None);

        let result = match req.language.to_lowercase().as_str() {
            "js" | "javascript" => {
                if !cfg.languages.js_enabled {
                    return Err("JavaScript sandbox is disabled in config".to_string());
                }
                match JsSandbox::new() {
                    Ok(mut sb) => sb.execute(&req.code, false, cfg.limits.timeout_ms),
                    Err(e) => SandboxResult::error("javascript", format!("Failed to create JS sandbox: {}", e), 0),
                }
            }
            "ts" | "typescript" => {
                if !cfg.languages.ts_enabled {
                    return Err("TypeScript sandbox is disabled in config".to_string());
                }
                match JsSandbox::new() {
                    Ok(mut sb) => {
                        let mut res = sb.execute(&req.code, true, cfg.limits.timeout_ms);
                        res.language = "typescript".to_string();
                        res
                    }
                    Err(e) => SandboxResult::error("typescript", format!("Failed to create TS sandbox: {}", e), 0),
                }
            }
            "py" | "python" => {
                if !cfg.languages.python_enabled {
                    return Err("Python sandbox is disabled in config".to_string());
                }
                let mut sb = PythonSandbox::new();
                sb.execute(&req.code, &cfg.python.interpreter, cfg.limits.timeout_ms).await
            }
            other => return Err(format!("Unknown language: '{}'. Use: js, ts, python", other)),
        };

        Ok(Json(result))
    }
}

pub async fn run() -> anyhow::Result<()> {
    let server = SmokeServer::new();
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
