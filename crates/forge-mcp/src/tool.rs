use crate::client::McpClient;
use async_trait::async_trait;
use forge_tool::{Tool, ToolError, ToolResult};
use serde_json::Value;

pub struct McpTool {
    client: McpClient,
    name: String,
    mcp_name: String,
    description: String,
    schema: Value,
}

impl McpTool {
    pub fn new(
        client: McpClient,
        name: String,
        mcp_name: String,
        description: String,
        schema: Value,
    ) -> Self {
        Self {
            client,
            name,
            mcp_name,
            description,
            schema,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        if self.schema.is_null() || self.schema.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            serde_json::json!({ "type": "object", "properties": {} })
        } else {
            self.schema.clone()
        }
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let output = self
            .client
            .call_tool(&self.mcp_name, arguments)
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        Ok(ToolResult {
            output,
            is_error: false,
        })
    }
}
