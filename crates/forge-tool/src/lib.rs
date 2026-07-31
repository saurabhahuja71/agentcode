use async_trait::async_trait;
use forge_provider::ToolDefinition;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool not found: {0}")]
    NotFound(String),
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("execution failed: {0}")]
    Execution(String),
    #[error("safety: {0}")]
    Safety(#[from] forge_safety::SafetyError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            tool_type: "function".into(),
            function: forge_provider::FunctionDefinition {
                name: self.name().to_string(),
                description: self.description().to_string(),
                parameters: self.parameters_schema(),
            },
        }
    }
    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError>;
}

pub mod file;
pub mod git;
pub mod index;
pub mod outline;
pub mod registry;
pub mod search;
pub mod shell;
pub mod skills;

pub use index::{new_index_tools, ProjectIndexTool, SearchCodebaseTool};

pub use registry::ToolRegistry;
pub use skills::{Skill, SkillLoader};
