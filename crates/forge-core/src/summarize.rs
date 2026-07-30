use forge_provider::Message;

pub fn estimate_tokens(messages: &[Message]) -> usize {
    let text: String = messages
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n");
    // Rough estimate: ~4 chars per token
    text.len() / 4
}

pub fn summarize_messages(messages: &[Message], keep_recent: usize) -> Vec<Message> {
    if messages.len() <= keep_recent + 2 {
        return messages.to_vec();
    }

    let split = messages.len().saturating_sub(keep_recent);
    let older = &messages[..split];
    let recent = &messages[split..];

    let summary_text = older
        .iter()
        .map(message_to_text)
        .collect::<Vec<_>>()
        .join("\n");

    let truncated: String = summary_text.chars().take(4000).collect();

    let mut result = vec![Message::System {
        content: format!(
            "Previous conversation summary ({} messages condensed):\n{truncated}",
            older.len()
        ),
    }];
    result.extend_from_slice(recent);
    result
}

fn message_to_text(msg: &Message) -> String {
    match msg {
        Message::System { content } => format!("[system] {content}"),
        Message::User { content } => format!("[user] {content}"),
        Message::Assistant { content, tool_calls } => {
            let mut s = format!("[assistant] {}", content.as_deref().unwrap_or(""));
            if let Some(calls) = tool_calls {
                for call in calls {
                    s.push_str(&format!(
                        "\n  tool_call: {}({})",
                        call.function.name, call.function.arguments
                    ));
                }
            }
            s
        }
        Message::Tool {
            tool_call_id,
            content,
        } => format!("[tool:{tool_call_id}] {content}"),
    }
}
