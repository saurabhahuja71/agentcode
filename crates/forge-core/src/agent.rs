use crate::events::AgentEvent;
use crate::hooks::{HookContext, HookPhase, HookRegistry};
use crate::session::Session;
use crate::summarize::{estimate_tokens, summarize_messages};
use anyhow::Result;
use forge_config::ForgeConfig;
use forge_provider::{
    ChatRequest, FunctionCall, LlmProvider, Message, ProviderRouter, StreamEvent, ToolCall,
};
use forge_tool::ToolRegistry;
use futures::StreamExt;
use parking_lot::RwLock;
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
    config: RwLock<RuntimeAgentConfig>,
    hooks: Arc<HookRegistry>,
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
            config: RwLock::new(config),
            hooks: Arc::new(HookRegistry::new()),
        }
    }

    pub fn from_forge_config(config: &ForgeConfig, tools: Arc<ToolRegistry>) -> Self {
        let provider = Arc::new(ProviderRouter::from_config(config)) as Arc<dyn LlmProvider>;
        Self::new(provider, tools, RuntimeAgentConfig::from(config))
    }

    pub fn hooks(&self) -> Arc<HookRegistry> {
        self.hooks.clone()
    }

    pub fn set_model(&self, model: String) {
        self.config.write().model = model;
    }

    pub fn model(&self) -> String {
        self.config.read().model.clone()
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

        let cfg = self.config.read().clone();

        if session.messages.is_empty() {
            session.messages.push(Message::System {
                content: cfg.system_prompt.clone(),
            });
        }

        session.messages.push(Message::User {
            content: user_message.clone(),
        });
        session.set_title_from_message(&user_message);

        if estimate_tokens(&session.messages) > cfg.summarize_threshold {
            session.messages = summarize_messages(&session.messages, 20);
        }

        for turn in 0..cfg.max_turns {
            emit(AgentEvent::TurnStart { turn });
            self.hooks.emit(&HookContext {
                phase: HookPhase::TurnStart,
                tool_name: None,
                arguments: None,
                output: None,
                is_error: None,
                turn: Some(turn),
            });

            let request = ChatRequest {
                model: cfg.model.clone(),
                messages: session.messages.clone(),
                tools: self.tools.definitions(),
                temperature: cfg.temperature,
                max_tokens: cfg.max_tokens,
                stream: cfg.stream,
            };

            let response = if cfg.stream {
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

            let tool_results = self.execute_tools_concurrent(&calls, &emit).await;
            for (call_id, output) in tool_results {
                session.messages.push(Message::Tool {
                    tool_call_id: call_id,
                    content: output,
                });
            }

            emit(AgentEvent::TurnEnd { turn });
            self.hooks.emit(&HookContext {
                phase: HookPhase::TurnEnd,
                tool_name: None,
                arguments: None,
                output: None,
                is_error: None,
                turn: Some(turn),
            });
        }

        session.touch();
        emit(AgentEvent::Done);
        Ok(())
    }

    async fn execute_tools_concurrent(
        &self,
        calls: &[ToolCall],
        emit: &impl Fn(AgentEvent),
    ) -> Vec<(String, String)> {
        let mut handles = Vec::with_capacity(calls.len());

        for call in calls {
            let name = call.function.name.clone();
            let args_str = call.function.arguments.clone();
            let call_id = call.id.clone();
            emit(AgentEvent::ToolCallStart {
                name: name.clone(),
                arguments: args_str.clone(),
            });

            let args: Value = serde_json::from_str(&args_str).unwrap_or(Value::Null);
            self.hooks
                .emit(&HookContext::pre_tool(&name, args.clone()));

            let tools = self.tools.clone();
            let hooks = self.hooks.clone();
            let name_for_task = name.clone();

            handles.push(tokio::spawn(async move {
                let result = tools.execute(&name_for_task, args).await;
                let (output, is_error) = match result {
                    Ok(r) => (r.output, r.is_error),
                    Err(e) => (e.to_string(), true),
                };
                hooks.emit(&HookContext::post_tool(
                    &name_for_task,
                    output.clone(),
                    is_error,
                ));
                (call_id, name_for_task, output, is_error)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok((call_id, name, output, is_error)) => {
                    emit(AgentEvent::ToolCallEnd {
                        name,
                        output: output.clone(),
                        is_error,
                    });
                    results.push((call_id, output));
                }
                Err(e) => {
                    emit(AgentEvent::Error {
                        message: format!("tool task panicked: {e}"),
                    });
                }
            }
        }
        results
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
                    let mut final_tool_calls = if tool_calls.is_empty() {
                        if let Message::Assistant { tool_calls: Some(ref tc), .. } = resp.message {
                            tc.clone()
                        } else {
                            Vec::new()
                        }
                    } else {
                        tool_calls
                    };

                    for (i, tc) in final_tool_calls.iter_mut().enumerate() {
                        if tc.id.is_empty() {
                            tc.id = format!("call_{}", i);
                        }
                    }

                    if content.is_empty() {
                        if let Message::Assistant {
                            content: Some(c),
                            ..
                        } = resp.message.clone()
                        {
                            if !c.is_empty() {
                                emit(AgentEvent::ContentDelta { text: c.clone() });
                                content = c;
                            }
                        }
                    }

                    return Ok(forge_provider::ChatResponse {
                        message: Message::Assistant {
                            content: if content.is_empty() { None } else { Some(content) },
                            tool_calls: if final_tool_calls.is_empty() { None } else { Some(final_tool_calls) },
                        },
                        finish_reason: resp.finish_reason,
                        usage: resp.usage,
                    });
                }
                StreamEvent::Error(e) => {
                    emit(AgentEvent::Error { message: e.clone() });
                    return Err(anyhow::anyhow!(e));
                }
            }
        }

        for (i, tc) in tool_calls.iter_mut().enumerate() {
            if tc.id.is_empty() {
                tc.id = format!("call_{}", i);
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
