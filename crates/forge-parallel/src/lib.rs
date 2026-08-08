use anyhow::Result;
use forge_core::{Agent, AgentEvent, Interactivity, Session};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::info;
use uuid::Uuid;

pub mod worktree;
pub use worktree::{is_git_repo, WorktreeGuard};

#[derive(Debug, Clone)]
pub struct ParallelConfig {
    pub use_worktrees: bool,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self { use_worktrees: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelTask {
    pub id: String,
    pub description: String,
    pub status: TaskStatus,
    pub result: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

pub struct ParallelExecutor {
    agent: Arc<Agent>,
    workspace: PathBuf,
    model: String,
    config: ParallelConfig,
}

impl ParallelExecutor {
    pub fn new(agent: Arc<Agent>, workspace: PathBuf, model: String) -> Self {
        Self {
            agent,
            workspace,
            model,
            config: ParallelConfig::default(),
        }
    }

    pub fn with_config(mut self, config: ParallelConfig) -> Self {
        self.config = config;
        self
    }

    pub async fn run_parallel(&self, tasks: Vec<String>) -> Result<Vec<ParallelTask>> {
        let use_worktrees = self.config.use_worktrees && is_git_repo(&self.workspace);
        let mut handles = Vec::new();
        let results: Arc<Mutex<Vec<ParallelTask>>> = Arc::new(Mutex::new(Vec::new()));

        for description in tasks {
            let task_id = Uuid::new_v4().to_string();
            let agent = self.agent.clone();
            let workspace = self.workspace.clone();
            let model = self.model.clone();
            let results = results.clone();

            let handle = tokio::spawn(async move {
                info!(task_id = %task_id, "starting parallel task");

                let mut task = ParallelTask {
                    id: task_id.clone(),
                    description: description.clone(),
                    status: TaskStatus::Running,
                    result: None,
                };

                let (_worktree_guard, task_workspace) = if use_worktrees {
                    match WorktreeGuard::create(&workspace, None).await {
                        Ok(guard) => {
                            let path = guard.path().to_path_buf();
                            (Some(guard), path)
                        }
                        Err(e) => {
                            task.status = TaskStatus::Failed;
                            task.result = Some(format!("worktree creation failed: {e}"));
                            results.lock().await.push(task);
                            return;
                        }
                    }
                } else {
                    (None, workspace.clone())
                };

                let mut session = Session::new(task_workspace, model);
                let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
                let agent_clone = agent.clone();
                let desc = description.clone();

                let run_handle = tokio::spawn(async move {
                    agent_clone.run_turn(&mut session, desc, Some(tx), Interactivity::none()).await
                });

                let mut output = String::new();
                while let Some(event) = rx.recv().await {
                    match event {
                        AgentEvent::ContentDelta { text } => output.push_str(&text),
                        AgentEvent::ToolCallEnd { output: o, .. } => {
                            output.push_str(&format!("\n[tool] {o}\n"));
                        }
                        AgentEvent::Error { message } => {
                            output.push_str(&format!("\n[error] {message}\n"));
                        }
                        AgentEvent::Done => break,
                        _ => {}
                    }
                }

                let run_result = run_handle.await;
                match run_result {
                    Ok(Ok(())) => {
                        task.status = TaskStatus::Completed;
                        task.result = Some(output);
                    }
                    Ok(Err(e)) => {
                        task.status = TaskStatus::Failed;
                        task.result = Some(e.to_string());
                    }
                    Err(e) => {
                        task.status = TaskStatus::Failed;
                        task.result = Some(e.to_string());
                    }
                }

                results.lock().await.push(task);
            });

            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        let final_results = results.lock().await.clone();
        Ok(final_results)
    }

    pub async fn decompose_and_run(&self, goal: String, num_tasks: usize) -> Result<Vec<ParallelTask>> {
        let prompt = format!(
            "Break down this goal into exactly {num_tasks} independent sub-tasks that can run in parallel. \
             Return ONLY a JSON array of strings, one task per element.\n\nGoal: {goal}"
        );

        let mut session = Session::new(self.workspace.clone(), self.model.clone());
        self.agent
            .run_turn(&mut session, prompt, None, Interactivity::none())
            .await?;

        let last_assistant = session
            .messages
            .iter()
            .rev()
            .find_map(|m| match m {
                forge_provider::Message::Assistant { content, .. } => content.clone(),
                _ => None,
            })
            .unwrap_or_else(|| goal.clone());

        let tasks: Vec<String> = serde_json::from_str(&last_assistant)
            .unwrap_or_else(|_| vec![goal]);

        self.run_parallel(tasks).await
    }
}
