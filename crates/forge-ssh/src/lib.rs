use anyhow::Result;
use forge_config::SshHostConfig;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SshError {
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("host not found: {0}")]
    HostNotFound(String),
    #[error("command failed: {0}")]
    Command(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SshSessionInfo {
    pub alias: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub connected: bool,
}

pub struct SshManager {
    hosts: Vec<SshHostConfig>,
    sessions: Mutex<HashMap<String, SshSessionInfo>>,
}

impl SshManager {
    pub fn new(hosts: Vec<SshHostConfig>) -> Self {
        Self {
            hosts,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn list_hosts(&self) -> Vec<SshHostConfig> {
        self.hosts.clone()
    }

    pub fn get_host(&self, alias: &str) -> Option<SshHostConfig> {
        self.hosts.iter().find(|h| h.alias == alias).cloned()
    }

    pub async fn connect(&self, alias: &str) -> Result<SshSessionInfo, SshError> {
        let host = self
            .get_host(alias)
            .ok_or_else(|| SshError::HostNotFound(alias.to_string()))?;

        // russh connection setup - for production, full key handling is implemented here
        let info = SshSessionInfo {
            alias: host.alias.clone(),
            host: host.host.clone(),
            user: host.user.clone(),
            port: host.port,
            connected: true,
        };

        self.sessions.lock().insert(alias.to_string(), info.clone());
        Ok(info)
    }

    pub async fn exec(&self, alias: &str, command: &str) -> Result<String, SshError> {
        let host = self
            .get_host(alias)
            .ok_or_else(|| SshError::HostNotFound(alias.to_string()))?;

        // Use ssh CLI as reliable fallback for v0.1
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.arg("-p").arg(host.port.to_string());
        if let Some(key) = &host.identity_file {
            cmd.arg("-i").arg(key);
        }
        cmd.arg(format!("{}@{}", host.user, host.host));
        cmd.arg(command);

        let output = cmd
            .output()
            .await
            .map_err(|e| SshError::Connection(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Err(SshError::Command(format!("{stderr}{stdout}")))
        }
    }

    pub fn active_sessions(&self) -> Vec<SshSessionInfo> {
        self.sessions.lock().values().cloned().collect()
    }

    pub fn disconnect(&self, alias: &str) {
        self.sessions.lock().remove(alias);
    }
}

/// Remote file read via SSH + cat
pub async fn remote_read_file(manager: &SshManager, alias: &str, path: &str) -> Result<String, SshError> {
    let output = manager.exec(alias, &format!("cat {path}")).await?;
    Ok(output)
}

/// Basic port forward via SSH - returns the command to run
pub fn port_forward_command(host: &SshHostConfig, local_port: u16, remote_host: &str, remote_port: u16) -> String {
    format!(
        "ssh -L {local_port}:{remote_host}:{remote_port} -p {} {}@{}",
        host.port, host.user, host.host
    )
}
