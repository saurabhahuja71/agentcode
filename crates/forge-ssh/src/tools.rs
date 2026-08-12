use crate::SshManager;
use async_trait::async_trait;
use forge_safety::validate_allowed_command;
use forge_tool::{Tool, ToolError, ToolResult};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct RemoteExecTool {
    manager: Arc<SshManager>,
    allowed_commands: Vec<String>,
}

pub struct RemoteReadFileTool {
    manager: Arc<SshManager>,
}

pub struct RemoteListDirTool {
    manager: Arc<SshManager>,
}

pub struct SshExecuteTool {
    manager: Arc<SshManager>,
    allowed_commands: Vec<String>,
}

impl RemoteExecTool {
    pub fn new(manager: Arc<SshManager>, allowed_commands: Vec<String>) -> Self {
        Self {
            manager,
            allowed_commands,
        }
    }
}

impl RemoteReadFileTool {
    pub fn new(manager: Arc<SshManager>) -> Self {
        Self { manager }
    }
}

impl RemoteListDirTool {
    pub fn new(manager: Arc<SshManager>) -> Self {
        Self { manager }
    }
}

impl SshExecuteTool {
    pub fn new(manager: Arc<SshManager>, allowed_commands: Vec<String>) -> Self {
        Self {
            manager,
            allowed_commands,
        }
    }
}

#[async_trait]
impl Tool for SshExecuteTool {
    fn name(&self) -> &str {
        "ssh_execute"
    }

    fn description(&self) -> &str {
        "Execute a command through an SSH alias from ~/.ssh/config. Use this for podman8, podman9, and other configured hosts."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH config alias, for example podman8" },
                "command": { "type": "string", "description": "Command to run on the remote host" }
            },
            "required": ["alias", "command"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let alias = arguments["alias"]
            .as_str()
            .or_else(|| arguments["host"].as_str())
            .ok_or_else(|| ToolError::InvalidArgs("alias required".into()))?;
        let command = arguments["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("command required".into()))?;

        validate_allowed_command(command, &self.allowed_commands)?;
        match self.manager.exec_ssh_alias(alias, command).await {
            Ok(output) => Ok(ToolResult {
                output,
                is_error: false,
            }),
            Err(e) => Ok(ToolResult {
                output: e.to_string(),
                is_error: true,
            }),
        }
    }
}

#[async_trait]
impl Tool for RemoteExecTool {
    fn name(&self) -> &str {
        "remote_exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command on a remote SSH host configured in forge config."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH host alias" },
                "command": { "type": "string", "description": "Command to run" }
            },
            "required": ["alias", "command"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let alias = arguments["alias"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("alias required".into()))?;
        let command = arguments["command"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("command required".into()))?;

        validate_allowed_command(command, &self.allowed_commands)?;

        match self.manager.exec(alias, command).await {
            Ok(output) => Ok(ToolResult {
                output,
                is_error: false,
            }),
            Err(e) => Ok(ToolResult {
                output: e.to_string(),
                is_error: true,
            }),
        }
    }
}

#[async_trait]
impl Tool for RemoteReadFileTool {
    fn name(&self) -> &str {
        "remote_read_file"
    }

    fn description(&self) -> &str {
        "Read a file from a remote SSH host via cat."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH host alias" },
                "path": { "type": "string", "description": "Remote file path" }
            },
            "required": ["alias", "path"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let alias = arguments["alias"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("alias required".into()))?;
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("path required".into()))?;

        match crate::remote_read_file(&self.manager, alias, path).await {
            Ok(content) => Ok(ToolResult {
                output: content,
                is_error: false,
            }),
            Err(e) => Ok(ToolResult {
                output: e.to_string(),
                is_error: true,
            }),
        }
    }
}

#[async_trait]
impl Tool for RemoteListDirTool {
    fn name(&self) -> &str {
        "remote_list_dir"
    }

    fn description(&self) -> &str {
        "List directory contents on a remote SSH host."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "alias": { "type": "string", "description": "SSH host alias" },
                "path": { "type": "string", "description": "Remote directory path", "default": "." }
            },
            "required": ["alias"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let alias = arguments["alias"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("alias required".into()))?;
        let path = arguments["path"].as_str().unwrap_or(".");

        let command = format!("ls -la {path}");
        match self.manager.exec(alias, &command).await {
            Ok(output) => Ok(ToolResult {
                output,
                is_error: false,
            }),
            Err(e) => Ok(ToolResult {
                output: e.to_string(),
                is_error: true,
            }),
        }
    }
}

pub fn register_ssh_tools(
    registry: &mut forge_tool::ToolRegistry,
    manager: Arc<SshManager>,
    allowed_commands: Vec<String>,
) {
    registry.register(Arc::new(SshExecuteTool::new(
        manager.clone(),
        allowed_commands.clone(),
    )));
    registry.register(Arc::new(RemoteExecTool::new(
        manager.clone(),
        allowed_commands,
    )));
    registry.register(Arc::new(RemoteReadFileTool::new(manager.clone())));
    registry.register(Arc::new(RemoteListDirTool::new(manager)));
}
