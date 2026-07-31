pub mod audit;
pub mod sandbox;
pub mod trust;

pub use audit::AuditLogger;
pub use sandbox::{validate_allowed_command, Sandbox};
pub use trust::WorkspaceTrust;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SafetyError {
    #[error("path escapes workspace: {0}")]
    PathEscape(String),
    #[error("workspace not trusted: {0}")]
    UntrustedWorkspace(String),
    #[error("command not allowed: {0}")]
    CommandDenied(String),
    #[error("destructive action requires confirmation: {0}")]
    ConfirmationRequired(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub tool: String,
    pub action: String,
    pub details: serde_json::Value,
    pub success: bool,
}
