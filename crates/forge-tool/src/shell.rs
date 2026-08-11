use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs};

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

        let executable_command = expand_ssh_alias(command);
        self.sandbox.validate_command(&executable_command)?;

        let cwd = if let Some(dir) = arguments["working_dir"].as_str() {
            self.sandbox.resolve_path(dir)?
        } else {
            self.sandbox.workspace().to_path_buf()
        };

        let output = tokio::time::timeout(
            self.timeout,
            tokio::process::Command::new("bash")
                .arg("-c")
                .arg(&executable_command)
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
            json!({ "command": command, "executed_command": executable_command, "exit_code": exit_code }),
            exit_code == 0,
        );

        Ok(ToolResult {
            output: result,
            is_error: exit_code != 0,
        })
    }
}

/// Expand a command's first token when it is an SSH alias declared in the
/// active user's bashrc. This keeps aliases dynamic and lets the normal shell
/// safety validator inspect the resulting executable command.
fn expand_ssh_alias(command: &str) -> String {
    let trimmed = command.trim_start();
    let prefix_len = command.len() - trimmed.len();
    let token_end = trimmed
        .find(char::is_whitespace)
        .unwrap_or(trimmed.len());
    let name = &trimmed[..token_end];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return command.to_string();
    }

    let home = env::var_os("HOME").map(std::path::PathBuf::from);
    let Some(home) = home else { return command.to_string() };
    let path = home.join(".bashrc");
    let Ok(contents) = fs::read_to_string(path) else { return command.to_string() };

    for line in contents.lines() {
        let Some(rest) = line.trim_start().strip_prefix("alias ") else { continue };
        let Some((alias_name, value)) = rest.split_once('=') else { continue };
        if alias_name.trim() != name {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')));
        let Some(value) = value else { continue };
        if !value.trim_start().starts_with("ssh ") {
            continue;
        }
        let suffix = &trimmed[token_end..];
        return format!("{}{}{}", &command[..prefix_len], value, suffix);
    }
    command.to_string()
}
