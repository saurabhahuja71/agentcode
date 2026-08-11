use crate::events::AgentEvent;
use crate::hooks::{HookContext, HookPhase, HookRegistry};
use crate::session::Session;
use crate::summarize::{estimate_tokens, summarize_messages};
use anyhow::Result;
use forge_config::ForgeConfig;
use forge_provider::{
    ChatRequest, FunctionCall, LlmProvider, Message, ProviderError, ProviderRouter, StreamEvent,
    ToolCall,
};
use forge_tool::ToolRegistry;
use futures::StreamExt;
use parking_lot::RwLock;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

const MAX_RETRIES: u32 = 3;

/// Transient provider failures worth retrying: network errors, 429 rate
/// limits, 5xx server errors, and common "try again" messages.
fn is_retriable(err: &ProviderError) -> bool {
    match err {
        ProviderError::Http(_) => true,
        ProviderError::Api(msg) => {
            let lower = msg.to_lowercase();
            ["429", "500", "502", "503", "504", "529"]
                .iter()
                .any(|code| lower.contains(code))
                || lower.contains("timeout")
                || lower.contains("temporarily")
                || lower.contains("overloaded")
                || lower.contains("too many requests")
                || lower.contains("unavailable")
        }
        _ => false,
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(500 * (1u64 << attempt.min(4)))
}

/// Interactive checkpoint for approving destructive tool calls. When no
/// receiver is wired up (headless mode), every request is allowed through;
/// the sandbox allow-list and destructive-command checks still apply.
#[derive(Clone, Debug, Default)]
pub struct ApprovalGate {
    inner: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<bool>>>>,
}

impl ApprovalGate {
    /// Build a live gate backed by the given response channel.
    pub fn new(rx: mpsc::UnboundedReceiver<bool>) -> Self {
        Self {
            inner: Some(Arc::new(tokio::sync::Mutex::new(rx))),
        }
    }

    /// Headless gate: never blocks, always approves.
    pub fn none() -> Self {
        Self { inner: None }
    }

    /// Whether this gate is interactive (has a live response channel).
    pub fn is_interactive(&self) -> bool {
        self.inner.is_some()
    }

    /// Emit an `ApprovalRequest` event, then block until the operator answers.
    /// Returns `true` when the call may proceed.
    pub async fn ask(&self, tool_name: &str, arguments: &str, emit: &impl Fn(AgentEvent)) -> bool {
        let Some(rx) = &self.inner else {
            return true;
        };
        emit(AgentEvent::ApprovalRequest {
            tool_name: tool_name.to_string(),
            arguments: arguments.to_string(),
        });
        rx.lock().await.recv().await.unwrap_or(false)
    }
}

/// Channel-backed interactive multiple-choice prompt. When no receiver is
/// wired up (headless mode), `ask` returns `None` so callers can degrade
/// gracefully instead of blocking forever.
#[derive(Clone, Debug, Default)]
pub struct OptionsGate {
    inner: Option<Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>>>,
}

impl OptionsGate {
    /// Build a live gate backed by the given response channel.
    pub fn new(rx: mpsc::UnboundedReceiver<String>) -> Self {
        Self {
            inner: Some(Arc::new(tokio::sync::Mutex::new(rx))),
        }
    }

    /// Headless gate: never blocks, always yields `None`.
    pub fn none() -> Self {
        Self { inner: None }
    }

    /// Emit an `OptionsRequest` event, then block until the user answers.
    /// Returns the chosen text (selected option or typed answer), or `None`
    /// when the prompt was dismissed.
    pub async fn ask(
        &self,
        prompt: &str,
        options: &[String],
        emit: &impl Fn(AgentEvent),
    ) -> Option<String> {
        let Some(rx) = &self.inner else {
            return None;
        };
        emit(AgentEvent::OptionsRequest {
            prompt: prompt.to_string(),
            options: options.to_vec(),
        });
        rx.lock().await.recv().await
    }
}

/// Bundle of interactive UI channels handed to `run_turn`.
#[derive(Clone, Debug)]
pub struct Interactivity {
    pub approval: ApprovalGate,
    pub options: OptionsGate,
}

impl Interactivity {
    /// Headless: no interactive approval, no option prompts.
    pub fn none() -> Self {
        Self {
            approval: ApprovalGate::none(),
            options: OptionsGate::none(),
        }
    }

    /// Fully interactive: approval + options wired to the UI.
    pub fn new(approval: ApprovalGate, options: OptionsGate) -> Self {
        Self { approval, options }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeAgentConfig {
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_turns: u32,
    pub system_prompt: String,
    pub stream: bool,
    pub summarize_threshold: usize,
    pub context_window: usize,
    /// When true, destructive shell commands never require interactive approval.
    pub full_auto: bool,
    /// "ask" | "allow" | "plan".
    pub permission_mode: String,
    /// Accumulated cost in USD for the current session.
    pub total_cost: f64,
}

impl From<&ForgeConfig> for RuntimeAgentConfig {
    fn from(config: &ForgeConfig) -> Self {
        let permission_mode = match &config.agent.permission_mode {
            Some(m) if m == "plan" => "plan".to_string(),
            Some(m) => m.clone(),
            None if config.agent.full_auto => "allow".to_string(),
            None => "allow".to_string(),
        };
        Self {
            model: config.agent.model.clone(),
            temperature: config.agent.temperature,
            max_tokens: config.agent.max_tokens,
            max_turns: config.agent.max_turns,
            system_prompt: config.agent.system_prompt.clone(),
            stream: true,
            summarize_threshold: config.session.summarize_threshold,
            context_window: config.session.context_window,
            full_auto: config.agent.full_auto,
            permission_mode,
            total_cost: 0.0,
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

    /// Select the provider request mode for the next turn.
    pub fn set_stream(&self, stream: bool) {
        self.config.write().stream = stream;
    }

    pub fn model(&self) -> String {
        self.config.read().model.clone()
    }

    /// Name of the provider that would serve the current model (for the status bar).
    pub fn provider_name(&self) -> String {
        let model = self.model();
        self.provider.provider_name_for_model(&model)
    }

    pub fn full_auto(&self) -> bool {
        self.config.read().full_auto
    }

    pub fn permission_mode(&self) -> String {
        self.config.read().permission_mode.clone()
    }

    pub fn set_permission_mode(&self, mode: &str) {
        self.config.write().permission_mode = mode.to_string();
    }

    pub fn pricing(&self) -> Option<(f64, f64)> {
        crate::pricing::price_per_1m(&self.config.read().model)
    }

    pub fn total_cost(&self) -> f64 {
        self.config.read().total_cost
    }

    fn add_cost(&self, input_tokens: u64, output_tokens: u64) {
        if let Some(cost) =
            crate::pricing::cost_for(&self.config.read().model, input_tokens, output_tokens)
        {
            self.config.write().total_cost += cost;
        }
    }

    pub fn context_window(&self) -> usize {
        self.config.read().context_window
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.names()
    }

    pub fn restrict_to_workspace(&self) -> bool {
        self.tools.restrict_to_workspace()
    }

    pub fn toggle_restrict_to_workspace(&self) -> bool {
        self.tools.toggle_restrict_to_workspace()
    }

    pub async fn run_turn(
        &self,
        session: &mut Session,
        user_message: String,
        event_tx: Option<mpsc::UnboundedSender<AgentEvent>>,
        interactivity: Interactivity,
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
            compact_for_context(&mut session.messages, cfg.context_window);
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
                self.chat_with_retry(&request, &emit).await?
            };

            session.total_tokens += response.usage.total_tokens as u64;
            self.add_cost(
                response.usage.prompt_tokens as u64,
                response.usage.completion_tokens as u64,
            );
            emit(AgentEvent::TokenUsage {
                prompt: response.usage.prompt_tokens,
                completion: response.usage.completion_tokens,
                total: response.usage.total_tokens,
            });

            let Message::Assistant {
                content,
                tool_calls,
            } = response.message
            else {
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

            let tool_results = self
                .execute_tools_concurrent(&calls, &emit, &interactivity, session)
                .await;
            for (call_id, output, is_error) in tool_results {
                let content = if is_error {
                    format!("[tool_error]\n{output}")
                } else {
                    output
                };
                session.messages.push(Message::Tool {
                    tool_call_id: call_id,
                    content,
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

    /// Replace older messages with an LLM-generated summary, keeping recent context.
    /// Returns the summary text, or `None` when the conversation is too short to compact.
    pub async fn compact(&self, session: &mut Session) -> Result<Option<String>> {
        const KEEP_RECENT: usize = 10;
        if session.messages.len() <= KEEP_RECENT + 1 {
            return Ok(None);
        }

        let cfg = self.config.read().clone();

        // Walk the split back until `older` ends on a non-tool message so the
        // summarization request is always well-formed (no orphan tool results).
        let mut split = session.messages.len().saturating_sub(KEEP_RECENT);
        while split > 0 && matches!(session.messages.get(split - 1), Some(Message::Tool { .. })) {
            split -= 1;
        }

        let older = session.messages[..split].to_vec();
        let recent = session.messages[split..].to_vec();

        // Preserve the leading system prompt; summarize everything after it.
        let (prefix, to_summarize) = match older.split_first() {
            Some((Message::System { .. }, rest)) => (older[..1].to_vec(), rest.to_vec()),
            _ => (Vec::new(), older.clone()),
        };
        if to_summarize.is_empty() {
            return Ok(None);
        }

        let summary = match self.summarize_with_llm(&to_summarize, &cfg).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "LLM compact failed; falling back to truncation");
                summarize_messages(&to_summarize, 0)
                    .into_iter()
                    .find_map(|m| match m {
                        Message::System { content } => Some(content),
                        _ => None,
                    })
                    .unwrap_or_else(|| "[summary unavailable]".into())
            }
        };

        session.messages = prefix;
        session.messages.push(Message::System {
            content: format!("Summary of earlier conversation:\n{summary}"),
        });
        session.messages.extend(recent);
        session.touch();
        Ok(Some(summary))
    }

    async fn summarize_with_llm(
        &self,
        messages: &[Message],
        cfg: &RuntimeAgentConfig,
    ) -> Result<String> {
        let mut req_messages = messages.to_vec();
        req_messages.push(Message::User {
            content: "Summarize the conversation above for a follow-up task. \
                      Keep: goals, decisions, file paths, error states, and unfinished work. \
                      Output only the summary."
                .into(),
        });
        let request = ChatRequest {
            model: cfg.model.clone(),
            messages: req_messages,
            tools: vec![],
            temperature: 0.2,
            max_tokens: 2000,
            stream: false,
        };
        let resp = self
            .provider
            .chat(request)
            .await
            .map_err(|e| anyhow::anyhow!(e))?;
        match resp.message {
            Message::Assistant {
                content: Some(c), ..
            } if !c.is_empty() => Ok(c),
            _ => Err(anyhow::anyhow!("summarization returned empty response")),
        }
    }

    /// Shell commands that deserve a human checkpoint before running.
    fn requires_approval(&self, call: &ToolCall) -> bool {
        if call.function.name != "shell" {
            return false;
        }
        let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
        let command = args["command"].as_str().unwrap_or("");
        is_destructive_command(command)
    }

    /// Tools that only read the codebase / session; always allowed in "plan" mode.
    fn is_read_only_tool(name: &str) -> bool {
        matches!(
            name,
            "read_file"
                | "list_dir"
                | "grep"
                | "glob"
                | "project_index"
                | "search_codebase"
                | "code_outline"
                | "read_skill"
                | "git_status"
                | "git_diff"
                | "git_log"
        )
    }

    async fn execute_tools_concurrent(
        &self,
        calls: &[ToolCall],
        emit: &impl Fn(AgentEvent),
        interactivity: &Interactivity,
        session: &mut Session,
    ) -> Vec<(String, String, bool)> {
        let mut handles = Vec::with_capacity(calls.len());
        let mut results = Vec::with_capacity(calls.len());

        for call in calls {
            let name = call.function.name.clone();
            let args_str = call.function.arguments.clone();
            let call_id = call.id.clone();

            // Interactive tools are handled inline (they may block on the UI).
            if name == "ask_options" {
                let outcome = self
                    .execute_options_call(call, &args_str, emit, &interactivity.options)
                    .await;
                let output = outcome.0;
                let is_error = outcome.1;
                emit(AgentEvent::ToolCallEnd {
                    name: name.clone(),
                    output: output.clone(),
                    is_error,
                });
                results.push((call_id, output, is_error));
                continue;
            }
            if name == "todo" {
                emit(AgentEvent::ToolCallStart {
                    name: name.clone(),
                    arguments: args_str.clone(),
                });
                let output = self.execute_todo_call(call, emit, session).await;
                emit(AgentEvent::ToolCallEnd {
                    name: name.clone(),
                    output: output.clone(),
                    is_error: false,
                });
                results.push((call_id, output, false));
                continue;
            }

            let plan_mode = self.config.read().permission_mode == "plan";
            let permission_mode = self.config.read().permission_mode.clone();
            let needs_check = if plan_mode {
                !Self::is_read_only_tool(&name)
            } else if permission_mode == "allow" {
                false
            } else {
                self.requires_approval(call)
            };

            if needs_check {
                let allowed = interactivity.approval.ask(&name, &args_str, emit).await;

                if !allowed {
                    let output = if plan_mode {
                        "[plan] proposal not approved; execution skipped.".to_string()
                    } else {
                        "Command was not approved by the user; execution skipped.".to_string()
                    };
                    emit(AgentEvent::ToolCallEnd {
                        name,
                        output: output.clone(),
                        is_error: true,
                    });
                    results.push((call_id, output, true));
                    continue;
                }
            }

            emit(AgentEvent::ToolCallStart {
                name: name.clone(),
                arguments: args_str.clone(),
            });

            let args: Value = serde_json::from_str(&args_str).unwrap_or(Value::Null);
            self.hooks.emit(&HookContext::pre_tool(&name, args.clone()));

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

        for handle in handles {
            match handle.await {
                Ok((call_id, name, output, is_error)) => {
                    emit(AgentEvent::ToolCallEnd {
                        name,
                        output: output.clone(),
                        is_error,
                    });
                    results.push((call_id, output, is_error));
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

    /// Interactive `ask_options` tool call: emit an `OptionsRequest`, wait for
    /// the user's choice, and return it as the tool result.
    async fn execute_options_call(
        &self,
        call: &ToolCall,
        args_str: &str,
        emit: &impl Fn(AgentEvent),
        options: &OptionsGate,
    ) -> (String, bool) {
        emit(AgentEvent::ToolCallStart {
            name: call.function.name.clone(),
            arguments: args_str.to_string(),
        });

        let args: Value = serde_json::from_str(args_str).unwrap_or(Value::Null);
        let prompt = args["prompt"]
            .as_str()
            .unwrap_or("Choose an option")
            .to_string();
        let options_list: Vec<String> = args["options"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|o| o.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        if options_list.is_empty() {
            return ("[ask_options] no options provided".to_string(), true);
        }

        match options.ask(&prompt, &options_list, emit).await {
            Some(choice) => (choice, false),
            None => (
                "[ask_options] prompt dismissed; pick one yourself".to_string(),
                false,
            ),
        }
    }

    /// Interactive `todo` tool call: apply the operation to `session.todos`,
    /// emit a `TodoUpdate` snapshot, and return a short confirmation.
    async fn execute_todo_call(
        &self,
        call: &ToolCall,
        emit: &impl Fn(AgentEvent),
        session: &mut Session,
    ) -> String {
        let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
        let op = args["op"].as_str().unwrap_or("add").to_string();
        let text = args["text"].as_str().unwrap_or("").trim().to_string();
        let id = args["id"].as_str().unwrap_or("").to_string();

        let summary = match op.as_str() {
            "add" if !text.is_empty() => {
                let item = crate::session::TodoItem::new(text.clone());
                session.todos.push(item);
                format!("todo added: {text}")
            }
            "complete" | "done" => {
                if let Some(item) = session.todos.iter_mut().find(|t| t.id == id) {
                    item.done = true;
                    format!("todo completed: {}", item.text)
                } else {
                    "todo not found".to_string()
                }
            }
            "reopen" => {
                if let Some(item) = session.todos.iter_mut().find(|t| t.id == id) {
                    item.done = false;
                    format!("todo reopened: {}", item.text)
                } else {
                    "todo not found".to_string()
                }
            }
            "remove" | "delete" => {
                let before = session.todos.len();
                session.todos.retain(|t| t.id != id);
                if session.todos.len() < before {
                    "todo removed".to_string()
                } else {
                    "todo not found".to_string()
                }
            }
            "update" => {
                let new_text = args["new_text"].as_str().unwrap_or("").trim();
                if let Some(item) = session.todos.iter_mut().find(|t| t.id == id) {
                    if !new_text.is_empty() {
                        item.text = new_text.to_string();
                        format!("todo updated: {new_text}")
                    } else {
                        "todo update needs new_text".to_string()
                    }
                } else {
                    "todo not found".to_string()
                }
            }
            "clear" => {
                let count = session.todos.len();
                session.todos.clear();
                format!("cleared {count} todos")
            }
            "list" => {
                if session.todos.is_empty() {
                    "no todos".to_string()
                } else {
                    let lines: Vec<String> = session
                        .todos
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            format!(
                                "{}. {} {}",
                                i + 1,
                                if t.done { "[x]" } else { "[ ]" },
                                t.text
                            )
                        })
                        .collect();
                    format!("todos:\n{}", lines.join("\n"))
                }
            }
            other => {
                format!("todo: unknown op '{other}' (add|complete|reopen|remove|update|clear|list)")
            }
        };

        emit(AgentEvent::TodoUpdate {
            items: session.todos.clone(),
        });
        summary
    }

    async fn chat_with_retry(
        &self,
        request: &ChatRequest,
        emit: &impl Fn(AgentEvent),
    ) -> Result<forge_provider::ChatResponse> {
        for attempt in 0..MAX_RETRIES {
            match self.provider.chat(request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) if is_retriable(&e) && attempt + 1 < MAX_RETRIES => {
                    emit(AgentEvent::Retrying {
                        attempt: attempt + 1,
                        error: e.to_string(),
                    });
                    tokio::time::sleep(backoff(attempt)).await;
                }
                Err(e) => return Err(anyhow::anyhow!(e)),
            }
        }
        Err(anyhow::anyhow!("provider call failed after retries"))
    }

    async fn run_streaming(
        &self,
        request: &ChatRequest,
        emit: &impl Fn(AgentEvent),
    ) -> Result<forge_provider::ChatResponse> {
        let mut stream = None;
        for attempt in 0..MAX_RETRIES {
            match self.provider.chat_stream(request.clone()).await {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) if is_retriable(&e) && attempt + 1 < MAX_RETRIES => {
                    emit(AgentEvent::Retrying {
                        attempt: attempt + 1,
                        error: e.to_string(),
                    });
                    tokio::time::sleep(backoff(attempt)).await;
                }
                Err(e) => return Err(anyhow::anyhow!(e)),
            }
        }
        let mut stream = stream.ok_or_else(|| anyhow::anyhow!("provider stream unavailable"))?;

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut thinking = false;

        while let Some(event) = stream.next().await {
            match event {
                StreamEvent::ReasoningDelta(delta) => {
                    if !delta.is_empty() {
                        if !thinking {
                            thinking = true;
                            emit(AgentEvent::Thinking);
                        }
                        emit(AgentEvent::ThinkingDelta { text: delta });
                    }
                }
                StreamEvent::ContentDelta(delta) => {
                    if thinking {
                        thinking = false;
                        emit(AgentEvent::ThinkingEnd);
                    }
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
                    if thinking {
                        emit(AgentEvent::ThinkingEnd);
                    }
                    let mut final_tool_calls = if tool_calls.is_empty() {
                        if let Message::Assistant {
                            tool_calls: Some(ref tc),
                            ..
                        } = resp.message
                        {
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
                            content: Some(c), ..
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
                            content: if content.is_empty() {
                                None
                            } else {
                                Some(content)
                            },
                            tool_calls: if final_tool_calls.is_empty() {
                                None
                            } else {
                                Some(final_tool_calls)
                            },
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

/// Keep tool results from exhausting a local model context window. Tool output
/// has already been emitted to the caller, so retaining a bounded prefix is
/// sufficient for the model's next decision and keeps headless runs usable.
fn compact_for_context(messages: &mut Vec<Message>, context_window: usize) {
    const MAX_TOOL_CHARS: usize = 12_000;
    for message in messages.iter_mut() {
        if let Message::Tool { content, .. } = message {
            if content.len() > MAX_TOOL_CHARS {
                content.truncate(MAX_TOOL_CHARS);
                content.push_str("\n[tool output truncated in conversation context]");
            }
        }
    }

    let budget = context_window.saturating_mul(3) / 4;
    if estimate_tokens(messages) > budget {
        *messages = summarize_messages(messages, 8);
    }
}

/// Whether a shell command is destructive enough to warrant a confirmation.
fn is_destructive_command(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or("");
    let base = Path::new(first)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(first);
    match base {
        "rm" | "dd" | "mkfs" | "mkfs.ext4" | "mkfs.xfs" | "shutdown" | "reboot" | "halt"
        | "poweroff" | "sudo" | "kill" | "killall" | "pkill" => return true,
        _ => {}
    }
    let lower = command.to_lowercase();
    lower.contains("git reset --hard")
        || lower.contains("git clean -f")
        || lower.contains("git push --force")
        || lower.contains("git checkout --")
        || lower.contains("drop table")
        || lower.contains("truncate ")
        || lower.contains("chmod 777")
        || lower.contains("> /dev/sd")
}
