use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::sync::Arc;

pub struct GrepTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl GrepTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents using regex patterns (ripgrep-style)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string" },
                "path": { "type": "string", "default": "." },
                "glob": { "type": "string", "description": "File glob filter" },
                "max_results": { "type": "integer", "default": 100 }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let pattern = arguments["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("pattern required".into()))?;
        let path = arguments["path"].as_str().unwrap_or(".");
        let glob_filter = arguments["glob"].as_str();
        let max_results = arguments["max_results"].as_u64().unwrap_or(100) as usize;

        let resolved = self.sandbox.resolve_path(path)?;
        let re = regex::Regex::new(pattern)
            .map_err(|e| ToolError::InvalidArgs(e.to_string()))?;

        let mut matches = Vec::new();
        for entry in walkdir::WalkDir::new(&resolved)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            if let Some(glob) = glob_filter {
                let file_name = entry.path().file_name().and_then(|s| s.to_str()).unwrap_or("");
                if !glob::Pattern::new(glob)
                    .map(|p| p.matches(file_name))
                    .unwrap_or(true)
                {
                    continue;
                }
            }

            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                for (line_no, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        let rel = entry
                            .path()
                            .strip_prefix(self.sandbox.workspace())
                            .unwrap_or(entry.path());
                        matches.push(format!("{}:{}:{}", rel.display(), line_no + 1, line));
                        if matches.len() >= max_results {
                            break;
                        }
                    }
                }
            }
            if matches.len() >= max_results {
                break;
            }
        }

        self.audit.log("grep", "search", json!({ "pattern": pattern }), true);

        Ok(ToolResult {
            output: matches.join("\n"),
            is_error: false,
        })
    }
}

pub struct GlobSearchTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
}

impl GlobSearchTool {
    pub fn new(sandbox: Arc<Sandbox>, audit: Arc<AuditLogger>) -> Self {
        Self { sandbox, audit }
    }
}

#[async_trait]
impl Tool for GlobSearchTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files matching a glob pattern."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob pattern e.g. **/*.rs" },
                "path": { "type": "string", "default": "." }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let pattern = arguments["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("pattern required".into()))?;
        let path = arguments["path"].as_str().unwrap_or(".");
        let resolved = self.sandbox.resolve_path(path)?;

        let full_pattern = format!("{}/{}", resolved.display(), pattern);
        let mut results = Vec::new();
        for entry in glob::glob(&full_pattern)
            .map_err(|e| ToolError::InvalidArgs(e.to_string()))?
            .filter_map(|e| e.ok())
            .take(200)
        {
            let rel = entry
                .strip_prefix(self.sandbox.workspace())
                .unwrap_or(&entry);
            results.push(rel.display().to_string());
        }

        self.audit.log("glob", "search", json!({ "pattern": pattern }), true);

        Ok(ToolResult {
            output: results.join("\n"),
            is_error: false,
        })
    }
}
