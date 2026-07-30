use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;
use uuid::Uuid;

pub struct WorktreeGuard {
    path: PathBuf,
    remove_on_drop: bool,
}

impl WorktreeGuard {
    pub async fn create(repo: &Path, base_branch: Option<&str>) -> Result<Self> {
        let id = Uuid::new_v4().to_string();
        let short_id = &id[..8];
        let worktree_path = repo
            .parent()
            .unwrap_or(repo)
            .join(format!(".forge-worktrees/{short_id}"));

        std::fs::create_dir_all(worktree_path.parent().unwrap())?;

        let branch = format!("forge/parallel-{short_id}");
        let base = base_branch.unwrap_or("HEAD");

        let output = tokio::process::Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                &branch,
                worktree_path.to_str().unwrap(),
                base,
            ])
            .current_dir(repo)
            .output()
            .await
            .context("creating git worktree")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git worktree add failed: {stderr}");
        }

        info!(path = %worktree_path.display(), branch = %branch, "created worktree");

        Ok(Self {
            path: worktree_path,
            remove_on_drop: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        if !self.remove_on_drop {
            return;
        }
        let path = self.path.clone();
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", path.to_str().unwrap_or("")])
            .status();
        let _ = std::fs::remove_dir_all(path.parent().unwrap_or(Path::new(".")));
    }
}

pub fn is_git_repo(path: &Path) -> bool {
    std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
