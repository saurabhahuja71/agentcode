use crate::AuditEntry;
use forge_config::SafetyConfig;
use parking_lot::Mutex;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;

pub struct AuditLogger {
    enabled: bool,
    path: PathBuf,
    buffer: Mutex<Vec<AuditEntry>>,
}

impl AuditLogger {
    pub fn new(config: &SafetyConfig) -> Self {
        let path = config.audit_log_path.clone().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".forge")
                .join("audit.log")
        });
        if config.audit_log {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
        Self {
            enabled: config.audit_log,
            path,
            buffer: Mutex::new(Vec::new()),
        }
    }

    pub fn log(&self, tool: &str, action: &str, details: serde_json::Value, success: bool) {
        if !self.enabled {
            return;
        }
        let entry = AuditEntry {
            id: Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            tool: tool.to_string(),
            action: action.to_string(),
            details,
            success,
        };
        self.buffer.lock().push(entry.clone());
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&self.path) {
            if let Ok(line) = serde_json::to_string(&entry) {
                let _ = writeln!(file, "{line}");
            }
        }
    }

    pub fn recent(&self, limit: usize) -> Vec<AuditEntry> {
        let buf = self.buffer.lock();
        let start = buf.len().saturating_sub(limit);
        buf[start..].to_vec()
    }
}
