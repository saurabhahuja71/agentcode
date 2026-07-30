use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

pub struct ShellTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
    timeout: Duration,
}

impl ShellTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>, timeout_secs: u64) -> Self {
        Self {
            sandbox,
            audit,
            timeout: Duration::from_secs(timeout_secs),
        }
    }
}

#[async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the workspace directory. Returns stdout and stderr."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command to execute" },
                "working_dir": { "type": "string", "description": "Optional working directory relative to workspace" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let command = arguments["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("command required".into()))?;

        self.sandbox.validate_command(command)?;

        let cwd = if let Some(dir) = arguments["working_dir"].as_str() {
            self.sandbox.resolve_path(dir)?
        } else {
            self.sandbox.workspace().to_path_buf()
        };

        let output = tokio::time::timeout(
            self.timeout,
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(command)
                .current_dir(&cwd)
                .output(),
        )
        .await
        .map_err(|_| ToolError::Execution("command timed out".into()))?
        .map_err(|e| ToolError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let exit_code = output.status.code().unwrap_or(-1);

        let result = format!(
            "exit_code: {exit_code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        );

        self.audit.log(
            "shell",
            "execute",
            json!({ "command": command, "exit_code": exit_code }),
            exit_code == 0,
        );

        Ok(ToolResult {
            output: result,
            is_error: exit_code != 0,
        })
    }
}
