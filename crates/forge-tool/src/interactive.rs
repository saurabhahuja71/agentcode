use crate::{Tool, ToolResult};
use async_trait::async_trait;
use forge_provider::ToolDefinition;
use serde_json::{json, Value};

/// Lets the model maintain the session's todo list. The agent loop intercepts
/// calls to this tool and applies them to the live session, so the execute
/// body here is only a fallback (headless / direct invocation).
pub struct TodoTool;

impl TodoTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TodoTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        "todo"
    }

    fn description(&self) -> &str {
        "Maintain the task todo list. Ops: add (text), complete|reopen|remove|update (id[, new_text]), clear, list. Returns the updated list."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["add", "complete", "reopen", "remove", "update", "clear", "list"],
                    "description": "Operation to perform"
                },
                "text": {
                    "type": "string",
                    "description": "Task text (required for add)"
                },
                "id": {
                    "type": "string",
                    "description": "Task id (required for complete/reopen/remove/update)"
                },
                "new_text": {
                    "type": "string",
                    "description": "Replacement text (required for update)"
                }
            },
            "required": ["op"]
        })
    }

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

    async fn execute(&self, arguments: Value) -> Result<ToolResult, crate::ToolError> {
        let op = arguments["op"].as_str().unwrap_or("add");
        let _ = op;
        Ok(ToolResult {
            output: "todo: handled interactively by the agent loop".into(),
            is_error: false,
        })
    }
}

/// Asks the user to pick from a list of options. The agent loop intercepts
/// calls and routes them through the interactive options UI.
pub struct AskOptionsTool;

impl AskOptionsTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AskOptionsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for AskOptionsTool {
    fn name(&self) -> &str {
        "ask_options"
    }

    fn description(&self) -> &str {
        "Present the user with a list of choices and wait for them to pick one or type their own answer. Use when a decision or direction is needed. Returns the chosen text."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The question or decision being asked"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "minItems": 1,
                    "description": "Choices to present (short labels)"
                }
            },
            "required": ["prompt", "options"]
        })
    }

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

    async fn execute(&self, _arguments: Value) -> Result<ToolResult, crate::ToolError> {
        Ok(ToolResult {
            output: "ask_options: handled interactively by the agent loop".into(),
            is_error: false,
        })
    }
}
