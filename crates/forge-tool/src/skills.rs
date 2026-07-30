use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub description: String,
    pub content: String,
}

pub struct SkillLoader {
    skills: Vec<Skill>,
}

impl SkillLoader {
    pub fn load(dir: Option<&Path>) -> Self {
        let Some(dir) = dir else {
            return Self { skills: vec![] };
        };
        if !dir.exists() {
            return Self { skills: vec![] };
        }

        let mut skills = Vec::new();
        for entry in WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if !file_name.eq_ignore_ascii_case("SKILL.md") && !file_name.ends_with(".skill.md") {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(skill) = parse_skill(path, &content) {
                    skills.push(skill);
                }
            }
        }

        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Self { skills }
    }

    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    pub fn names(&self) -> Vec<String> {
        self.skills.iter().map(|s| s.name.clone()).collect()
    }

    pub fn system_context(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }
        let mut ctx = String::from("Available skills (read the skill file for full instructions):\n");
        for skill in &self.skills {
            ctx.push_str(&format!(
                "- {}: {}\n",
                skill.name, skill.description
            ));
        }
        ctx
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|s| s.name == name)
    }
}

// --- ReadSkillTool ---

use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ReadSkillTool {
    loader: Arc<SkillLoader>,
}

impl ReadSkillTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for ReadSkillTool {
    fn name(&self) -> &str {
        "read_skill"
    }

    fn description(&self) -> &str {
        "Load the full content of a skill by name for workflow guidance."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Skill name" }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let name = arguments["name"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("name required".into()))?;

        let skill = self
            .loader
            .get(name)
            .ok_or_else(|| ToolError::NotFound(format!("skill: {name}")))?;

        Ok(ToolResult {
            output: skill.content.clone(),
            is_error: false,
        })
    }
}

fn parse_skill(path: &Path, content: &str) -> Option<Skill> {
    let name = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let (description, body) = if content.starts_with("---") {
        parse_frontmatter(content).unwrap_or_else(|| {
            let desc = content.lines().next().unwrap_or("").to_string();
            (desc, content.to_string())
        })
    } else {
        let desc = content.lines().next().unwrap_or("").to_string();
        (desc, content.to_string())
    };

    Some(Skill {
        name,
        path: path.to_path_buf(),
        description,
        content: body,
    })
}

fn parse_frontmatter(content: &str) -> Option<(String, String)> {
    let mut parts = content.splitn(3, "---");
    parts.next()?; // empty before first ---
    let front = parts.next()?;
    let body = parts.next()?;

    let description = front
        .lines()
        .find(|l| l.starts_with("description:"))
        .map(|l| l.trim_start_matches("description:").trim().to_string())
        .unwrap_or_else(|| body.lines().next().unwrap_or("").to_string());

    Some((description, body.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter() {
        let content = "---\ndescription: Test skill\n---\n# Body\nDo things.";
        let (desc, body) = parse_frontmatter(content).unwrap();
        assert_eq!(desc, "Test skill");
        assert!(body.contains("Do things"));
    }
}
