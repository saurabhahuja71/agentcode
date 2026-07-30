use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    TurnStart { turn: u32 },
    Thinking,
    ContentDelta { text: String },
    ToolCallStart { name: String, arguments: String },
    ToolCallEnd { name: String, output: String, is_error: bool },
    TurnEnd { turn: u32 },
    Error { message: String },
    TokenUsage { prompt: u32, completion: u32, total: u32 },
    Done,
}
