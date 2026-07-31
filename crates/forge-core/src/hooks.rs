use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookPhase {
    PreTool,
    PostTool,
    TurnStart,
    TurnEnd,
    SessionStart,
    SessionEnd,
}

#[derive(Debug, Clone)]
pub struct HookContext {
    pub phase: HookPhase,
    pub tool_name: Option<String>,
    pub arguments: Option<Value>,
    pub output: Option<String>,
    pub is_error: Option<bool>,
    pub turn: Option<u32>,
}

impl HookContext {
    pub fn pre_tool(name: &str, arguments: Value) -> Self {
        Self {
            phase: HookPhase::PreTool,
            tool_name: Some(name.to_string()),
            arguments: Some(arguments),
            output: None,
            is_error: None,
            turn: None,
        }
    }

    pub fn post_tool(name: &str, output: String, is_error: bool) -> Self {
        Self {
            phase: HookPhase::PostTool,
            tool_name: Some(name.to_string()),
            arguments: None,
            output: Some(output),
            is_error: Some(is_error),
            turn: None,
        }
    }
}

pub type HookCallback = Arc<dyn Fn(&HookContext) + Send + Sync>;

pub struct HookRegistry {
    callbacks: Mutex<Vec<HookCallback>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            callbacks: Mutex::new(Vec::new()),
        }
    }

    pub fn register<F>(&self, callback: F)
    where
        F: Fn(&HookContext) + Send + Sync + 'static,
    {
        self.callbacks
            .lock()
            .expect("hook registry lock")
            .push(Arc::new(callback));
    }

    pub fn emit(&self, ctx: &HookContext) {
        let callbacks = self.callbacks.lock().expect("hook registry lock");
        for cb in callbacks.iter() {
            cb(ctx);
        }
    }

    pub fn len(&self) -> usize {
        self.callbacks.lock().map(|c| c.len()).unwrap_or(0)
    }
}
