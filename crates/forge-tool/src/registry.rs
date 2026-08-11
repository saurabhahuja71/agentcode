use crate::{Tool, ToolError, ToolResult};
use forge_config::ToolsConfig;
use forge_provider::ToolDefinition;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    sandbox: Arc<Sandbox>,
}

impl ToolRegistry {
    pub fn new(
        sandbox: Arc<Sandbox>,
        audit: Arc<AuditLogger>,
        config: &ToolsConfig,
        skill_loader: Option<Arc<super::skills::SkillLoader>>,
    ) -> Self {
        let mut registry = Self {
            tools: HashMap::new(),
            sandbox: sandbox.clone(),
        };

        if config.file_tools {
            registry.register(Arc::new(super::file::ReadFileTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::file::WriteFileTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::file::EditFileTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::file::ListDirTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::outline::CodeOutlineTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
        }

        if let Some(loader) = skill_loader {
            if !loader.skills().is_empty() {
                registry.register(Arc::new(super::skills::ReadSkillTool::new(loader)));
            }
        }

        if config.search {
            registry.register(Arc::new(super::search::GrepTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::search::GlobSearchTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            let (index_tool, search_tool) =
                super::index::new_index_tools(sandbox.clone(), audit.clone());
            registry.register(Arc::new(index_tool));
            registry.register(Arc::new(search_tool));
        }

        if config.http {
            registry.register(Arc::new(super::http::HttpRequestTool::new()));
        }

        if config.shell {
            registry.register(Arc::new(super::shell::ShellTool::new(
                sandbox.clone(),
                audit.clone(),
                config.shell_timeout_secs,
            )));
        }

        if config.git {
            registry.register(Arc::new(super::git::GitStatusTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::git::GitDiffTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::git::GitLogTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
            registry.register(Arc::new(super::git::GitCommitTool::new(
                sandbox.clone(),
                audit.clone(),
            )));
        }

        registry.register(Arc::new(super::interactive::TodoTool::new()));
        registry.register(Arc::new(super::interactive::AskOptionsTool::new()));

        registry
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn restrict_to_workspace(&self) -> bool {
        self.sandbox.restrict_to_workspace()
    }

    pub fn toggle_restrict_to_workspace(&self) -> bool {
        self.sandbox.toggle_restrict_to_workspace()
    }

    pub async fn execute(&self, name: &str, arguments: Value) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_string()))?;
        tool.execute(arguments).await
    }
}
