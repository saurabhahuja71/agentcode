use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct GitStatusTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl GitStatusTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }

    async fn run_git(&self, args: &[&str]) -> Result<String, ToolError> {
        let output = tokio::process::Command::new("git")
            .args(args)
            .current_dir(self.sandbox.workspace())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(ToolError::Execution(format!("{stderr}{stdout}")))
        }
    }
}

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Show git working tree status."
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, ToolError> {
        let output = self.run_git(&["status", "--short", "--branch"]).await?;
        self.audit.log("git_status", "status", json!({}), true);
        Ok(ToolResult {
            output,
            is_error: false,
        })
    }
}

pub struct GitDiffTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl GitDiffTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Show git diff. Optionally pass a file path."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "staged": { "type": "boolean", "default": false }
            }
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let mut args = vec!["diff"];
        let staged = arguments["staged"].as_bool().unwrap_or(false);
        if staged {
            args.push("--staged");
        }
        let path = arguments["path"].as_str();
        if let Some(p) = path {
            args.push(p);
        }

        let output = tokio::process::Command::new("git")
            .args(&args)
            .current_dir(self.sandbox.workspace())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.audit.log("git_diff", "diff", json!({ "path": path }), true);

        Ok(ToolResult {
            output: stdout.to_string(),
            is_error: false,
        })
    }
}
