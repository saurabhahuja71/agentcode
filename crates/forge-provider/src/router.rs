use crate::{
    ollama::OllamaProvider, openai::OpenAiCompatibleProvider, BoxStream, ChatRequest,
    ChatResponse, LlmProvider, ProviderError, StreamEvent,
};
use async_trait::async_trait;
use forge_config::{ForgeConfig, ProviderConfig, ProviderKind};
use std::sync::Arc;
use tracing::{info, warn};

pub struct ProviderRouter {
    providers: Vec<Arc<dyn LlmProvider>>,
}

impl ProviderRouter {
    pub fn from_config(config: &ForgeConfig) -> Self {
        let mut providers: Vec<Arc<dyn LlmProvider>> = Vec::new();

        for p in config.enabled_providers() {
            if let Some(provider) = build_provider(p) {
                providers.push(provider);
            }
        }

        Self { providers }
    }

    fn find_provider(&self, model: &str) -> Option<&Arc<dyn LlmProvider>> {
        self.providers.iter().find(|p| p.supports_model(model))
    }

    pub fn list_providers(&self) -> Vec<String> {
        self.providers.iter().map(|p| p.name().to_string()).collect()
    }

    /// Name of the provider configured to serve `model`, or "unknown".
    pub fn provider_for_model(&self, model: &str) -> String {
        self.find_provider(model)
            .map(|p| p.name().to_string())
            .unwrap_or_else(|| "unknown".into())
    }
}

fn build_provider(config: &ProviderConfig) -> Option<Arc<dyn LlmProvider>> {
    let api_key = config
        .api_key
        .clone()
        .or_else(|| {
            config
                .api_key_env
                .as_ref()
                .and_then(|env| std::env::var(env).ok())
        });

    match config.kind {
        ProviderKind::Ollama => Some(Arc::new(OllamaProvider::new(
            &config.base_url,
            config.models.clone(),
        ))),
        ProviderKind::OpenAiCompatible => Some(Arc::new(OpenAiCompatibleProvider::new(
            &config.name,
            &config.base_url,
            api_key,
            config.models.clone(),
        ))),
    }
}

#[async_trait]
impl LlmProvider for ProviderRouter {
    fn name(&self) -> &str {
        "router"
    }

    fn supports_model(&self, model: &str) -> bool {
        self.find_provider(model).is_some()
    }

    fn provider_name_for_model(&self, model: &str) -> String {
        self.provider_for_model(model)
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        if self.providers.is_empty() {
            return Err(ProviderError::NoProviders);
        }

        let mut last_error = None;
        for provider in &self.providers {
            if !provider.supports_model(&request.model) {
                continue;
            }
            info!(provider = provider.name(), model = %request.model, "chat request");
            match provider.chat(request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    warn!(provider = provider.name(), error = %e, "provider failed, trying failover");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(ProviderError::NoProviders))
    }

    async fn chat_stream(&self, request: ChatRequest) -> Result<BoxStream<StreamEvent>, ProviderError> {
        if self.providers.is_empty() {
            return Err(ProviderError::NoProviders);
        }

        let mut last_error = None;
        for provider in &self.providers {
            if !provider.supports_model(&request.model) {
                continue;
            }
            info!(provider = provider.name(), model = %request.model, "stream request");
            match provider.chat_stream(request.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    warn!(provider = provider.name(), error = %e, "provider failed, trying failover");
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or(ProviderError::NoProviders))
    }
}
