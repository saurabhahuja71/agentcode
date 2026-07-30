use crate::SafetyError;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub struct WorkspaceTrust {
    trust_file: PathBuf,
    trusted: Mutex<HashSet<String>>,
}

impl WorkspaceTrust {
    pub fn new() -> Self {
        let trust_file = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".forge")
            .join("trusted_workspaces");
        let trusted = if trust_file.exists() {
            std::fs::read_to_string(&trust_file)
                .unwrap_or_default()
                .lines()
                .map(|s| s.to_string())
                .collect()
        } else {
            HashSet::new()
        };
        Self {
            trust_file,
            trusted: Mutex::new(trusted),
        }
    }

    fn hash_workspace(path: &Path) -> String {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut hasher = Sha256::new();
        hasher.update(canonical.to_string_lossy().as_bytes());
        hex::encode(hasher.finalize())
    }

    pub fn is_trusted(&self, workspace: &Path) -> bool {
        let hash = Self::hash_workspace(workspace);
        self.trusted.lock().contains(&hash)
    }

    pub fn trust(&self, workspace: &Path) -> Result<(), SafetyError> {
        let hash = Self::hash_workspace(workspace);
        {
            let mut trusted = self.trusted.lock();
            trusted.insert(hash.clone());
            let content = trusted.iter().cloned().collect::<Vec<_>>().join("\n");
            if let Some(parent) = self.trust_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&self.trust_file, content)?;
        }
        Ok(())
    }

    pub fn require_trust(&self, workspace: &Path, required: bool) -> Result<(), SafetyError> {
        if required && !self.is_trusted(workspace) {
            return Err(SafetyError::UntrustedWorkspace(
                workspace.display().to_string(),
            ));
        }
        Ok(())
    }
}

impl Default for WorkspaceTrust {
    fn default() -> Self {
        Self::new()
    }
}
