use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct CodeOutlineTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl CodeOutlineTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for CodeOutlineTool {
    fn name(&self) -> &str {
        "code_outline"
    }

    fn description(&self) -> &str {
        "Extract structural outline (functions, structs, classes, methods) from a source file."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Source file path" }
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
        let ext = resolved
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let outline = match ext {
            "rs" => outline_rust(&content),
            "go" => outline_go(&content),
            "py" => outline_python(&content),
            "js" | "ts" | "tsx" | "jsx" => outline_javascript(&content),
            _ => outline_generic(&content),
        };

        self.audit.log("code_outline", "outline", json!({ "path": path }), true);

        Ok(ToolResult {
            output: outline,
            is_error: false,
        })
    }
}

fn outline_rust(content: &str) -> String {
    let mut items = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") {
            items.push(format!("{}:{} function {}", line_no + 1, line_no + 1, trimmed));
        } else if trimmed.starts_with("pub struct ") || trimmed.starts_with("struct ") {
            items.push(format!("{}:{} struct {}", line_no + 1, line_no + 1, trimmed));
        } else if trimmed.starts_with("pub enum ") || trimmed.starts_with("enum ") {
            items.push(format!("{}:{} enum {}", line_no + 1, line_no + 1, trimmed));
        } else if trimmed.starts_with("pub trait ") || trimmed.starts_with("trait ") {
            items.push(format!("{}:{} trait {}", line_no + 1, line_no + 1, trimmed));
        } else if trimmed.starts_with("impl ") {
            items.push(format!("{}:{} impl {}", line_no + 1, line_no + 1, trimmed));
        }
    }
    items.join("\n")
}

fn outline_go(content: &str) -> String {
    let mut items = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("func ") {
            items.push(format!("{}:{} {}", line_no + 1, line_no + 1, trimmed));
        } else if trimmed.starts_with("type ") {
            items.push(format!("{}:{} {}", line_no + 1, line_no + 1, trimmed));
        }
    }
    items.join("\n")
}

fn outline_python(content: &str) -> String {
    let mut items = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            items.push(format!("{}:{} {}", line_no + 1, line_no + 1, trimmed));
        } else if trimmed.starts_with("class ") {
            items.push(format!("{}:{} {}", line_no + 1, line_no + 1, trimmed));
        }
    }
    items.join("\n")
}

fn outline_javascript(content: &str) -> String {
    let mut items = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("function ")
            || trimmed.contains("function(")
            || trimmed.starts_with("export function ")
            || trimmed.starts_with("async function ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
        {
            items.push(format!("{}:{} {}", line_no + 1, line_no + 1, trimmed));
        }
    }
    items.join("\n")
}

fn outline_generic(content: &str) -> String {
    let mut items = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.contains('(') && !trimmed.starts_with("//") && !trimmed.starts_with('#') {
            items.push(format!("{}: {}", line_no + 1, trimmed));
        }
    }
    items.into_iter().take(100).collect::<Vec<_>>().join("\n")
}
