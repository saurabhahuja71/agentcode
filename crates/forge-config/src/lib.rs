use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeConfig {
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    #[serde(default)]
    pub safety: SafetyConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub ssh: SshConfig,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default)]
    pub full_auto: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(default = "default_provider_kind")]
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(default)]
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[serde(alias = "openai_compatible", alias = "openai-compatible")]
    OpenAiCompatible,
    Ollama,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    #[serde(default = "default_true")]
    pub workspace_trust_required: bool,
    #[serde(default = "default_allowed_commands")]
    pub allowed_commands: Vec<String>,
    #[serde(default = "default_true")]
    pub restrict_to_workspace: bool,
    #[serde(default = "default_true")]
    pub confirm_destructive: bool,
    #[serde(default = "default_true")]
    pub audit_log: bool,
    #[serde(default)]
    pub audit_log_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default = "default_true")]
    pub file_tools: bool,
    #[serde(default = "default_true")]
    pub shell: bool,
    #[serde(default = "default_true")]
    pub git: bool,
    #[serde(default = "default_true")]
    pub search: bool,
    #[serde(default)]
    pub skills_dir: Option<PathBuf>,
    #[serde(default = "default_shell_timeout")]
    pub shell_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshConfig {
    #[serde(default)]
    pub hosts: Vec<SshHostConfig>,
    #[serde(default)]
    pub known_hosts_path: Option<PathBuf>,
    /// Commands permitted via remote_exec on SSH hosts.
    #[serde(default = "default_allowed_commands")]
    pub allowed_commands: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshHostConfig {
    pub alias: String,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub user: String,
    #[serde(default)]
    pub identity_file: Option<PathBuf>,
    #[serde(default)]
    pub password_env: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "default_session_dir")]
    pub storage_dir: PathBuf,
    #[serde(default = "default_true")]
    pub auto_save: bool,
    #[serde(default = "default_context_window")]
    pub context_window: usize,
    #[serde(default = "default_summarize_threshold")]
    pub summarize_threshold: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

fn default_model() -> String {
    "gpt-4o".into()
}
fn default_temperature() -> f32 {
    0.2
}
fn default_max_tokens() -> u32 {
    8192
}
fn default_max_turns() -> u32 {
    50
}
fn default_true() -> bool {
    true
}
fn default_provider_kind() -> ProviderKind {
    ProviderKind::OpenAiCompatible
}
fn default_ssh_port() -> u16 {
    22
}
fn default_shell_timeout() -> u64 {
    120
}
fn default_session_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".forge")
        .join("sessions")
}
fn default_context_window() -> usize {
    128_000
}
fn default_summarize_threshold() -> usize {
    100_000
}
fn default_system_prompt() -> String {
    "You are Forge, a precise and capable coding agent. \
     Use tools to read, search, edit, and execute code. \
     Be concise, safe, and production-minded."
        .into()
}

/// Default shell commands allowed locally (shell tool) and remotely (remote_exec).
pub fn default_allowed_commands() -> Vec<String> {
    vec![
        // file & text
        "ls".into(),
        "cat".into(),
        "grep".into(),
        "rg".into(),
        "find".into(),
        "head".into(),
        "tail".into(),
        "wc".into(),
        "diff".into(),
        "less".into(),
        "more".into(),
        "sed".into(),
        "awk".into(),
        "sort".into(),
        "cut".into(),
        "tr".into(),
        "tee".into(),
        "xargs".into(),
        // filesystem
        "mkdir".into(),
        "cp".into(),
        "mv".into(),
        "touch".into(),
        "ln".into(),
        "rm".into(),
        "chmod".into(),
        "chown".into(),
        "readlink".into(),
        "realpath".into(),
        "stat".into(),
        "du".into(),
        "df".into(),
        // archives
        "tar".into(),
        "gzip".into(),
        "gunzip".into(),
        "zip".into(),
        "unzip".into(),
        // dev & build
        "git".into(),
        "cargo".into(),
        "npm".into(),
        "node".into(),
        "python".into(),
        "python3".into(),
        "make".into(),
        "go".into(),
        "rustc".into(),
        // shell & meta
        "echo".into(),
        "pwd".into(),
        "which".into(),
        "env".into(),
        "bash".into(),
        "sh".into(),
        "true".into(),
        "false".into(),
        "test".into(),
        // network
        "curl".into(),
        "wget".into(),
        "ping".into(),
        "nc".into(),
        "netstat".into(),
        "ss".into(),
        "dig".into(),
        "nslookup".into(),
        // containers & orchestration
        "podman".into(),
        "docker".into(),
        "docker-compose".into(),
        "kubectl".into(),
        "helm".into(),
        // systemd & services
        "systemctl".into(),
        "journalctl".into(),
        "service".into(),
        // remote access
        "ssh".into(),
        "scp".into(),
        "rsync".into(),
        // processes
        "ps".into(),
        "top".into(),
        "htop".into(),
        "kill".into(),
        "killall".into(),
        "pgrep".into(),
        "pkill".into(),
        "nohup".into(),
        // system info
        "date".into(),
        "uname".into(),
        "hostname".into(),
        "id".into(),
        "whoami".into(),
        "sudo".into(),
    ]
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            agent: AgentConfig::default(),
            providers: vec![ProviderConfig {
                name: "ollama".into(),
                kind: ProviderKind::Ollama,
                base_url: "http://localhost:11434".into(),
                api_key_env: None,
                api_key: None,
                models: vec!["llama3.2".into()],
                enabled: true,
                priority: 10,
            }],
            safety: SafetyConfig::default(),
            tools: ToolsConfig::default(),
            ssh: SshConfig::default(),
            session: SessionConfig::default(),
            mcp: McpConfig::default(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            temperature: default_temperature(),
            max_tokens: default_max_tokens(),
            system_prompt: default_system_prompt(),
            max_turns: default_max_turns(),
            full_auto: false,
        }
    }
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            workspace_trust_required: true,
            allowed_commands: default_allowed_commands(),
            restrict_to_workspace: true,
            confirm_destructive: true,
            audit_log: true,
            audit_log_path: None,
        }
    }
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            file_tools: true,
            shell: true,
            git: true,
            search: true,
            skills_dir: None,
            shell_timeout_secs: default_shell_timeout(),
        }
    }
}

impl Default for SshConfig {
    fn default() -> Self {
        Self {
            hosts: vec![],
            known_hosts_path: None,
            allowed_commands: default_allowed_commands(),
        }
    }
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            storage_dir: default_session_dir(),
            auto_save: true,
            context_window: default_context_window(),
            summarize_threshold: default_summarize_threshold(),
        }
    }
}

impl Default for McpConfig {
    fn default() -> Self {
        Self { servers: vec![] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_models_deduplicates_and_includes_agent_default() {
        let mut config = ForgeConfig::default();
        config.agent.model = "custom-model".into();
        config.providers = vec![
            ProviderConfig {
                name: "a".into(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: "http://localhost".into(),
                api_key_env: None,
                api_key: None,
                models: vec!["gpt-4o".into(), "gpt-4o-mini".into()],
                enabled: true,
                priority: 1,
            },
            ProviderConfig {
                name: "b".into(),
                kind: ProviderKind::OpenAiCompatible,
                base_url: "http://localhost".into(),
                api_key_env: None,
                api_key: None,
                models: vec!["gpt-4o".into(), "llama3.2".into()],
                enabled: true,
                priority: 2,
            },
        ];

        let models = config.available_models();
        assert_eq!(
            models,
            vec![
                "custom-model".to_string(),
                "gpt-4o".to_string(),
                "gpt-4o-mini".to_string(),
                "llama3.2".to_string(),
            ]
        );
    }

    #[test]
    fn available_models_falls_back_to_agent_model() {
        let mut config = ForgeConfig::default();
        config.agent.model = "qwen2.5".into();
        config.providers = vec![ProviderConfig {
            name: "sglang".into(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: "http://127.0.0.1:30000".into(),
            api_key_env: None,
            api_key: None,
            models: vec![],
            enabled: true,
            priority: 1,
        }];

        assert_eq!(config.available_models(), vec!["qwen2.5".to_string()]);
    }
}

impl ForgeConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Self = toml::from_str(&content)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(config)
    }

    pub fn load_or_default(path: Option<&Path>) -> Result<Self> {
        if let Some(p) = path {
            if p.exists() {
                return Self::load(p);
            }
        }
        let default_path = Self::default_config_path();
        if default_path.exists() {
            return Self::load(&default_path);
        }
        Ok(Self::default())
    }

    pub fn default_config_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".forge")
            .join("config.toml")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn enabled_providers(&self) -> Vec<&ProviderConfig> {
        let mut providers: Vec<_> = self.providers.iter().filter(|p| p.enabled).collect();
        providers.sort_by_key(|p| p.priority);
        providers
    }

    /// All configured models from enabled providers, deduplicated in priority order.
    /// Falls back to the agent default when no provider lists models.
    pub fn available_models(&self) -> Vec<String> {
        let mut models = Vec::new();
        for provider in self.enabled_providers() {
            for model in &provider.models {
                if !models.iter().any(|m| m == model) {
                    models.push(model.clone());
                }
            }
        }
        if models.is_empty() {
            models.push(self.agent.model.clone());
        } else if !models.iter().any(|m| m == &self.agent.model) {
            models.insert(0, self.agent.model.clone());
        }
        models
    }
}
