use crate::session::TodoItem;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStart { turn: u32 },
    /// Model reasoning/thinking started.
    Thinking,
    /// A chunk of model reasoning/thinking text.
    ThinkingDelta { text: String },
    /// Model reasoning finished.
    ThinkingEnd,
    ContentDelta { text: String },
    ToolCallStart { name: String, arguments: String },
    ToolCallEnd { name: String, output: String, is_error: bool },
    /// A tool needs interactive approval before it may run.
    ApprovalRequest { tool_name: String, arguments: String },
    /// The agent is asking the user to pick from a set of options.
    OptionsRequest { prompt: String, options: Vec<String> },
    /// The session's todo list changed (full snapshot).
    TodoUpdate { items: Vec<TodoItem> },
    /// A provider call failed transiently and is being retried.
    Retrying { attempt: u32, error: String },
    TurnEnd { turn: u32 },
    Error { message: String },
    TokenUsage { prompt: u32, completion: u32, total: u32 },
    Done,
}
