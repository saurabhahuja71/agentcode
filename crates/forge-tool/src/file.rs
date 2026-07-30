use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ReadFileTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl ReadFileTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Optionally specify offset and limit for large files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File path relative to workspace or absolute" },
                "offset": { "type": "integer", "description": "Line offset (1-based)" },
                "limit": { "type": "integer", "description": "Max lines to read" }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("path required".into()))?;
        let resolved = self.sandbox.resolve_path(path)?;
        let content = tokio::fs::read_to_string(&resolved).await?;

        let offset = arguments["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let limit = arguments["limit"].as_u64().map(|l| l as usize);

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1);
        let end = limit.map(|l| (start + l).min(lines.len())).unwrap_or(lines.len());

        let output: String = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{}|{}", start + i + 1, line))
            .collect::<Vec<_>>()
            .join("\n");

        self.audit.log(
            "read_file",
            "read",
            json!({ "path": path, "lines": end - start }),
            true,
        );

        Ok(ToolResult {
            output,
            is_error: false,
        })
    }
}

pub struct WriteFileTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl WriteFileTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories if needed."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("path required".into()))?;
        let content = arguments["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("content required".into()))?;

        let resolved = self.sandbox.resolve_path(path)?;
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&resolved, content).await?;

        self.audit.log("write_file", "write", json!({ "path": path }), true);

        Ok(ToolResult {
            output: format!("Wrote {} bytes to {}", content.len(), path),
            is_error: false,
        })
    }
}

pub struct EditFileTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl EditFileTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Surgically replace old_string with new_string in a file. old_string must be unique."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("path required".into()))?;
        let old_string = arguments["old_string"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("old_string required".into()))?;
        let new_string = arguments["new_string"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("new_string required".into()))?;
        let replace_all = arguments["replace_all"].as_bool().unwrap_or(false);

        let resolved = self.sandbox.resolve_path(path)?;
        let content = tokio::fs::read_to_string(&resolved).await?;

        let count = content.matches(old_string).count();
        if count == 0 {
            return Err(ToolError::Execution(format!(
                "old_string not found in {path}"
            )));
        }
        if !replace_all && count > 1 {
            return Err(ToolError::Execution(format!(
                "old_string found {count} times; set replace_all or make it unique"
            )));
        }

        let new_content = if replace_all {
            content.replace(old_string, new_string)
        } else {
            content.replacen(old_string, new_string, 1)
        };

        tokio::fs::write(&resolved, &new_content).await?;
        self.audit.log("edit_file", "edit", json!({ "path": path }), true);

        Ok(ToolResult {
            output: format!("Edited {path}"),
            is_error: false,
        })
    }
}

pub struct ListDirTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl ListDirTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn description(&self) -> &str {
        "List files and directories in a path."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": "." },
                "recursive": { "type": "boolean", "default": false }
            }
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let path = arguments["path"].as_str().unwrap_or(".");
        let recursive = arguments["recursive"].as_bool().unwrap_or(false);
        let resolved = self.sandbox.resolve_path(path)?;

        let mut entries = Vec::new();
        if recursive {
            for entry in walkdir::WalkDir::new(&resolved)
                .into_iter()
                .filter_map(|e| e.ok())
                .take(500)
            {
                let rel = entry
                    .path()
                    .strip_prefix(self.sandbox.workspace())
                    .unwrap_or(entry.path());
                let kind = if entry.file_type().is_dir() { "dir" } else { "file" };
                entries.push(format!("{kind}\t{}", rel.display()));
            }
        } else {
            let mut read_dir = tokio::fs::read_dir(&resolved).await?;
            while let Some(entry) = read_dir.next_entry().await? {
                let ft = entry.file_type().await?;
                let kind = if ft.is_dir() { "dir" } else { "file" };
                entries.push(format!("{kind}\t{}", entry.file_name().to_string_lossy()));
            }
        }

        self.audit.log("list_dir", "list", json!({ "path": path }), true);

        Ok(ToolResult {
            output: entries.join("\n"),
            is_error: false,
        })
    }
}
