pub mod client;
pub mod tool;

pub use client::McpClient;
pub use tool::McpTool;

use anyhow::Result;
use forge_config::McpConfig;
use forge_tool::ToolRegistry;
use std::sync::Arc;
use tracing::{info, warn};

pub async fn register_mcp_tools(registry: &mut ToolRegistry, config: &McpConfig) -> Result<Vec<String>> {
    let mut registered = Vec::new();

    for server in &config.servers {
        info!(server = %server.name, "connecting to MCP server");
        match McpClient::connect(server).await {
            Ok(client) => {
                let tools = client.list_tools().await?;
                for tool_def in tools {
                    let name = format!("mcp_{}_{}", server.name, tool_def.name);
                    registry.register(Arc::new(McpTool::new(
                        client.clone(),
                        name.clone(),
                        tool_def.name,
                        tool_def.description,
                        tool_def.input_schema,
                    )));
                    registered.push(name);
                }
            }
            Err(e) => {
                warn!(server = %server.name, error = %e, "failed to connect MCP server");
            }
        }
    }

    Ok(registered)
}
