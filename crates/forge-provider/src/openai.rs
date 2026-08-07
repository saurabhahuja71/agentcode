use crate::{
    BoxStream, ChatRequest, ChatResponse, LlmProvider, Message, ProviderError, StreamEvent,
    TokenUsage, ToolCall,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone)]
pub struct OpenAiCompatibleProvider {
    name: String,
    base_url: String,
    api_key: Option<String>,
    models: Vec<String>,
    client: Client,
}

impl OpenAiCompatibleProvider {
    pub fn new(
        name: impl Into<String>,
        base_url: impl Into<String>,
        api_key: Option<String>,
        models: Vec<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("http client");
        Self {
            name: name.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            models,
            client,
        }
    }

    fn endpoint_url(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        }
    }
}

#[derive(Serialize)]
struct ApiChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    tools: &'a [crate::ToolDefinition],
    temperature: f32,
    max_tokens: u32,
    stream: bool,
}

#[derive(Deserialize)]
struct ApiChatResponse {
    choices: Vec<ApiChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct ApiChoice {
    message: ApiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ApiMessage {
    role: String,
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Deserialize)]
struct ApiUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    usage: Option<ApiUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<StreamToolCallDelta>>,
}

#[derive(Deserialize)]
struct StreamToolCallDelta {
    #[serde(default)]
    index: usize,
    id: Option<String>,
    function: Option<StreamFunctionDelta>,
}

#[derive(Deserialize)]
struct StreamFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_model(&self, model: &str) -> bool {
        self.models.is_empty() || self.models.iter().any(|m| m == model)
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let url = self.endpoint_url();
        let body = ApiChatRequest {
            model: &request.model,
            messages: &request.messages,
            tools: &request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: false,
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let parsed: ApiChatResponse = resp
            .json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))?;

        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::Api("no choices".into()))?;

        let message = match choice.message.role.as_str() {
            "assistant" => Message::Assistant {
                content: choice.message.content,
                tool_calls: choice.message.tool_calls,
            },
            "system" => Message::System {
                content: choice.message.content.unwrap_or_default(),
            },
            _ => Message::Assistant {
                content: choice.message.content,
                tool_calls: choice.message.tool_calls,
            },
        };

        Ok(ChatResponse {
            message,
            finish_reason: choice.finish_reason,
            usage: parsed.usage.map(|u| TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }).unwrap_or_default(),
        })
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<StreamEvent>, ProviderError> {
        let url = self.endpoint_url();
        let body = ApiChatRequest {
            model: &request.model,
            messages: &request.messages,
            tools: &request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            stream: true,
        };

        let mut req = self.client.post(&url).json(&body);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ProviderError::Api(format!("{status}: {text}")));
        }

        let stream = resp.bytes_stream().eventsource();
        // flat_map so a single SSE chunk can yield content *and* Done when the
        // provider packs finish_reason into the last content delta (common with
        // SGLang / some OpenAI-compatible servers). Returning only ContentDelta
        // previously left the HTTP body open until the 300s client timeout —
        // the TUI stayed "busy" after the first answer.
        let mapped = stream.flat_map(|event| {
            futures::stream::iter(parse_sse_event(event))
        });

        Ok(Box::pin(mapped))
    }
}

fn parse_sse_event(
    event: Result<eventsource_stream::Event, eventsource_stream::EventStreamError<reqwest::Error>>,
) -> Vec<StreamEvent> {
    match event {
        Ok(ev) => {
            if ev.data == "[DONE]" {
                return vec![StreamEvent::Done(ChatResponse {
                    message: Message::Assistant {
                        content: None,
                        tool_calls: None,
                    },
                    finish_reason: Some("stop".into()),
                    usage: TokenUsage::default(),
                })];
            }
            match serde_json::from_str::<StreamChunk>(&ev.data) {
                Ok(chunk) => {
                    let usage = chunk.usage.map(|u| TokenUsage {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                    });
                    let mut out = Vec::new();
                    if let Some(choice) = chunk.choices.into_iter().next() {
                        if let Some(content) = choice.delta.content {
                            if !content.is_empty() {
                                out.push(StreamEvent::ContentDelta(content));
                            }
                        }
                        if let Some(tool_calls) = choice.delta.tool_calls {
                            for tc in tool_calls {
                                let args = tc
                                    .function
                                    .as_ref()
                                    .and_then(|f| f.arguments.clone())
                                    .unwrap_or_default();
                                let name = tc.function.as_ref().and_then(|f| f.name.clone());
                                out.push(StreamEvent::ToolCallDelta {
                                    index: tc.index,
                                    id: tc.id,
                                    name,
                                    arguments_delta: args,
                                });
                            }
                        }
                        if choice.finish_reason.is_some() {
                            out.push(StreamEvent::Done(ChatResponse {
                                message: Message::Assistant {
                                    content: None,
                                    tool_calls: None,
                                },
                                finish_reason: choice.finish_reason,
                                usage: usage.unwrap_or_default(),
                            }));
                        }
                    } else if let Some(usage) = usage {
                        // Usage-only final chunk with empty choices — treat as Done
                        // so the consumer unblocks even without finish_reason/[DONE].
                        out.push(StreamEvent::Done(ChatResponse {
                            message: Message::Assistant {
                                content: None,
                                tool_calls: None,
                            },
                            finish_reason: Some("stop".into()),
                            usage,
                        }));
                    }
                    out
                }
                Err(e) => vec![StreamEvent::Error(e.to_string())],
            }
        }
        Err(e) => vec![StreamEvent::Error(e.to_string())],
    }
}
