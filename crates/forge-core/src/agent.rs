use crate::events::AgentEvent;
use crate::session::Session;
use crate::summarize::{estimate_tokens, summarize_messages};
use anyhow::Result;
use forge_config::ForgeConfig;
use forge_provider::{
    ChatRequest, FunctionCall, LlmProvider, Message, ProviderRouter, StreamEvent, ToolCall,
};
use forge_tool::ToolRegistry;
use futures::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct RuntimeAgentConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_turns: u32,
    pub system_prompt: String,
    pub stream: bool,
    pub summarize_threshold: usize,
}

impl From<&ForgeConfig> for RuntimeAgentConfig {
    fn from(config: &ForgeConfig) -> Self {
        Self {
            model: config.agent.model.clone(),
            temperature: config.agent.temperature,
            max_tokens: config.agent.max_tokens,
            max_turns: config.agent.max_turns,
            system_prompt: config.agent.system_prompt.clone(),
            stream: true,
            summarize_threshold: config.session.summarize_threshold,
        }
    }
}

pub struct Agent {
    provider: Arc<dyn LlmProvider>,
    tools: Arc<ToolRegistry>,
    config: RuntimeAgentConfig,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn LlmProvider>,
        tools: Arc<ToolRegistry>,
        config: RuntimeAgentConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            config,
        }
    }

    pub fn from_forge_config(config: &ForgeConfig, tools: Arc<ToolRegistry>) -> Self {
        let provider = Arc::new(ProviderRouter::from_config(config)) as Arc<dyn LlmProvider>;
        Self::new(provider, tools, RuntimeAgentConfig::from(config))
    }

    pub fn set_model(&mut self, model: String) {
        self.config.model = model;
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.names()
    }

    pub async fn run_turn(
        &self,
        session: &mut Session,
        user_message: String,
        event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<()> {
        let emit = |event: AgentEvent| {
            if let Some(tx) = &event_tx {
                let _ = tx.send(event);
            }
        };

        if session.messages.is_empty() {
            session.messages.push(Message::System {
                content: self.config.system_prompt.clone(),
            });
        }

        session.messages.push(Message::User {
            content: user_message.clone(),
        });
        session.set_title_from_message(&user_message);

        if estimate_tokens(&session.messages) > self.config.summarize_threshold {
            session.messages = summarize_messages(&session.messages, 20);
        }

        for turn in 0..self.config.max_turns {
            emit(AgentEvent::TurnStart { turn });

            let request = ChatRequest {
                model: self.config.model.clone(),
                messages: session.messages.clone(),
                tools: self.tools.definitions(),
                temperature: self.config.temperature,
                max_tokens: self.config.max_tokens,
                stream: self.config.stream,
            };

            let response = if self.config.stream {
                self.run_streaming(&request, &emit).await?
            } else {
                self.provider.chat(request).await.map_err(|e| anyhow::anyhow!(e))?
            };

            session.total_tokens += response.usage.total_tokens as u64;
            emit(AgentEvent::TokenUsage {
                prompt: response.usage.prompt_tokens,
                completion: response.usage.completion_tokens,
                total: response.usage.total_tokens,
            });

            let Message::Assistant { content, tool_calls } = response.message else {
                emit(AgentEvent::Error {
                    message: "unexpected non-assistant response".into(),
                });
                break;
            };

            session.messages.push(Message::Assistant {
                content: content.clone(),
                tool_calls: tool_calls.clone(),
            });

            let calls = match tool_calls {
                Some(c) if !c.is_empty() => c,
                _ => {
                    emit(AgentEvent::TurnEnd { turn });
                    break;
                }
            };

            for call in calls {
                let name = call.function.name.clone();
                let args_str = call.function.arguments.clone();
                emit(AgentEvent::ToolCallStart {
                    name: name.clone(),
                    arguments: args_str.clone(),
                });

                let args: Value = serde_json::from_str(&args_str).unwrap_or(Value::Null);
                let result = self.tools.execute(&name, args).await;
                let (output, is_error) = match result {
                    Ok(r) => (r.output, r.is_error),
                    Err(e) => (e.to_string(), true),
                };

                emit(AgentEvent::ToolCallEnd {
                    name: name.clone(),
                    output: output.clone(),
                    is_error,
                });

                session.messages.push(Message::Tool {
                    tool_call_id: call.id,
                    content: output,
                });
            }

            emit(AgentEvent::TurnEnd { turn });
        }

        session.touch();
        emit(AgentEvent::Done);
        Ok(())
    }

    async fn run_streaming(
        &self,
        request: &ChatRequest,
        emit: &impl Fn(AgentEvent),
    ) -> Result<forge_provider::ChatResponse> {
        let mut stream = self
            .provider
            .chat_stream(request.clone())
            .await
            .map_err(|e| anyhow::anyhow!(e))?;

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::ContentDelta(delta) => {
                    if !delta.is_empty() {
                        content.push_str(&delta);
                        emit(AgentEvent::ContentDelta { text: delta });
                    }
                }
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    while tool_calls.len() <= index {
                        tool_calls.push(ToolCall {
                            id: String::new(),
                            call_type: "function".into(),
                            function: FunctionCall {
                                name: String::new(),
                                arguments: String::new(),
                            },
                        });
                    }
                    if let Some(id) = id {
                        tool_calls[index].id = id;
                    }
                    if let Some(name) = name {
                        tool_calls[index].function.name = name;
                    }
                    tool_calls[index]
                        .function
                        .arguments
                        .push_str(&arguments_delta);
                }
                StreamEvent::Done(resp) => {
                    if content.is_empty() {
                        if let Message::Assistant {
                            content: c,
                            tool_calls: tc,
                        } = resp.message
                        {
                            return Ok(forge_provider::ChatResponse {
                                message: Message::Assistant {
                                    content: c,
                                    tool_calls: tc,
                                },
                                finish_reason: resp.finish_reason,
                                usage: resp.usage,
                            });
                        }
                    }
                    return Ok(forge_provider::ChatResponse {
                        message: Message::Assistant {
                            content: if content.is_empty() {
                                None
                            } else {
                                Some(content)
                            },
                            tool_calls: if tool_calls.is_empty() {
                                None
                            } else {
                                Some(tool_calls)
                            },
                        },
                        finish_reason: Some("stop".into()),
                        usage: resp.usage,
                    });
                }
                StreamEvent::Error(e) => {
                    emit(AgentEvent::Error { message: e.clone() });
                    return Err(anyhow::anyhow!(e));
                }
            }
        }

        Ok(forge_provider::ChatResponse {
            message: Message::Assistant {
                content: if content.is_empty() {
                    None
                } else {
                    Some(content)
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
            },
            finish_reason: Some("stop".into()),
            usage: forge_provider::TokenUsage::default(),
        })
    }
}
