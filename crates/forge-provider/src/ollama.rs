use crate::{
    openai::OpenAiCompatibleProvider, BoxStream, ChatRequest, ChatResponse, LlmProvider,
    ProviderError, StreamEvent,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Ollama exposes an OpenAI-compatible API at /v1/chat/completions since recent versions.
/// This provider also supports native Ollama endpoints as fallback.
#[derive(Clone)]
pub struct OllamaProvider {
    inner: OpenAiCompatibleProvider,
    base_url: String,
    client: Client,
    models: Vec<String>,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>, models: Vec<String>) -> Self {
        let base = base_url.into().trim_end_matches('/').to_string();
        let inner = OpenAiCompatibleProvider::new("ollama", &base, None, models.clone());
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .expect("http client");
        Self {
            inner,
            base_url: base,
            client,
            models,
        }
    }

    async fn ensure_model(&self, model: &str) -> Result<(), ProviderError> {
        #[derive(Serialize)]
        struct PullRequest {
            name: String,
            stream: bool,
        }
        let url = format!("{}/api/pull", self.base_url);
        let _ = self
            .client
            .post(&url)
            .json(&PullRequest {
                name: model.to_string(),
                stream: false,
            })
            .send()
            .await;
        Ok(())
    }
}

#[derive(Deserialize)]
struct OllamaTags {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn supports_model(&self, model: &str) -> bool {
        if self.models.is_empty() {
            return true;
        }
        self.models.iter().any(|m| m == model || model.starts_with(m))
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let _ = self.ensure_model(&request.model).await;
        self.inner.chat(request).await
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<StreamEvent>, ProviderError> {
        let _ = self.ensure_model(&request.model).await;
        self.inner.chat_stream(request).await
    }
}
