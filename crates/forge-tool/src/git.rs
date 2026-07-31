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

pub struct GitLogTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl GitLogTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Show recent git commit history."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "Number of commits", "default": 10 }
            }
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let limit = arguments["limit"].as_u64().unwrap_or(10);
        let output = tokio::process::Command::new("git")
            .args([
                "log",
                &format!("-{limit}"),
                "--oneline",
                "--decorate",
            ])
            .current_dir(self.sandbox.workspace())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        self.audit.log("git_log", "log", json!({ "limit": limit }), true);

        Ok(ToolResult {
            output: stdout.to_string(),
            is_error: !output.status.success(),
        })
    }
}

pub struct GitCommitTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl GitCommitTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Stage all changes and create a git commit with the given message."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "Commit message" }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let message = arguments["message"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("message required".into()))?;

        self.sandbox.validate_command("git")?;

        let add = tokio::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(self.sandbox.workspace())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        if !add.status.success() {
            let stderr = String::from_utf8_lossy(&add.stderr);
            return Err(ToolError::Execution(format!("git add failed: {stderr}")));
        }

        let commit = tokio::process::Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(self.sandbox.workspace())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&commit.stdout);
        let stderr = String::from_utf8_lossy(&commit.stderr);
        let output = format!("{stdout}{stderr}");

        self.audit.log("git_commit", "commit", json!({ "message": message }), commit.status.success());

        Ok(ToolResult {
            output,
            is_error: !commit.status.success(),
        })
    }
}
