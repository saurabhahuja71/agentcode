use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use forge_provider::Message;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub workspace: PathBuf,
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub todos: Vec<TodoItem>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub total_tokens: u64,
}

/// A tracked task item. Managed by the agent through the `todo` tool and by
/// the user through the TUI side panel; persisted with the session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub done: bool,
    pub created_at: DateTime<Utc>,
}

impl TodoItem {
    pub fn new(text: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            text,
            done: false,
            created_at: Utc::now(),
        }
    }
}

impl Session {
    pub fn new(workspace: PathBuf, model: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            title: "New session".into(),
            workspace,
            model,
            messages: Vec::new(),
            todos: Vec::new(),
            created_at: now,
            updated_at: now,
            total_tokens: 0,
        }
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }

    pub fn set_title_from_message(&mut self, msg: &str) {
        let title: String = msg.chars().take(60).collect();
        if !title.is_empty() {
            self.title = title;
        }
    }
}

pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    pub fn save(&self, session: &Session) -> Result<()> {
        let path = self.session_path(&session.id);
        let content = serde_json::to_string_pretty(session)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Session> {
        let path = self.session_path(id);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("loading session {id}"))?;
        Ok(serde_json::from_str(&content)?)
    }

    pub fn list(&self) -> Result<Vec<SessionMeta>> {
        let mut sessions = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(session) = serde_json::from_str::<Session>(&content) {
                    sessions.push(SessionMeta {
                        id: session.id,
                        title: session.title,
                        model: session.model,
                        updated_at: session.updated_at,
                        message_count: session.messages.len(),
                    });
                }
            }
        }
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(sessions)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.session_path(id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    pub fn latest(&self) -> Result<Option<Session>> {
        let metas = self.list()?;
        if let Some(meta) = metas.first() {
            return Ok(Some(self.load(&meta.id)?));
        }
        Ok(None)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub title: String,
    pub model: String,
    pub updated_at: DateTime<Utc>,
    pub message_count: usize,
}
