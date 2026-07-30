use crate::SafetyError;
use forge_config::SafetyConfig;
use std::path::{Component, Path, PathBuf};

pub struct Sandbox {
    workspace: PathBuf,
    restrict_to_workspace: bool,
    allowed_commands: Vec<String>,
    confirm_destructive: bool,
    full_auto: bool,
}

impl Sandbox {
    pub fn new(workspace: PathBuf, safety: &SafetyConfig, full_auto: bool) -> Self {
        Self {
            workspace: workspace.canonicalize().unwrap_or(workspace),
            restrict_to_workspace: safety.restrict_to_workspace,
            allowed_commands: safety.allowed_commands.clone(),
            confirm_destructive: safety.confirm_destructive,
            full_auto,
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn resolve_path(&self, path: &str) -> Result<PathBuf, SafetyError> {
        let p = Path::new(path);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace.join(p)
        };
        let canonical = resolved.canonicalize().unwrap_or(resolved);

        if self.restrict_to_workspace && !canonical.starts_with(&self.workspace) {
            return Err(SafetyError::PathEscape(canonical.display().to_string()));
        }
        Ok(canonical)
    }

    pub fn validate_command(&self, command: &str) -> Result<(), SafetyError> {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return Err(SafetyError::CommandDenied("empty command".into()));
        }

        let first_token = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_start_matches("./");

        let base_cmd = Path::new(first_token)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(first_token);

        if !self.allowed_commands.iter().any(|c| c == base_cmd) {
            return Err(SafetyError::CommandDenied(base_cmd.to_string()));
        }

        if self.confirm_destructive && !self.full_auto && is_destructive(trimmed) {
            return Err(SafetyError::ConfirmationRequired(trimmed.to_string()));
        }

        Ok(())
    }

    pub fn is_within_workspace(&self, path: &Path) -> bool {
        path.starts_with(&self.workspace)
    }
}

fn is_destructive(command: &str) -> bool {
    let lower = command.to_lowercase();
    lower.contains("rm -rf")
        || lower.contains("rm -r")
        || lower.starts_with("rm ")
        || lower.contains("git reset --hard")
        || lower.contains("git clean -f")
        || lower.contains("drop table")
        || lower.contains("truncate ")
}

pub fn normalize_relative(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            other => result.push(other),
        }
    }
    result
}
