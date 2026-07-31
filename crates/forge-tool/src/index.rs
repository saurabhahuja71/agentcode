use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use forge_safety::{AuditLogger, Sandbox};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};
use walkdir::WalkDir;

const INDEXABLE_EXTENSIONS: &[&str] = &[
    "rs", "go", "py", "js", "ts", "tsx", "jsx", "java", "c", "cpp", "h", "hpp", "md", "toml",
    "yaml", "yml", "json", "sh", "sql",
];

const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".forge",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "vendor",
];

#[derive(Debug, Clone)]
struct IndexEntry {
    path: String,
    symbols: Vec<String>,
    preview: String,
}

#[derive(Debug)]
pub struct ProjectIndex {
    entries: Vec<IndexEntry>,
}

impl ProjectIndex {
    pub fn build(workspace: &Path) -> Self {
        let mut entries = Vec::new();

        for entry in WalkDir::new(workspace)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                if e.file_type().is_dir() {
                    let name = e.file_name().to_string_lossy();
                    return !SKIP_DIRS.contains(&name.as_ref());
                }
                true
            })
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !INDEXABLE_EXTENSIONS.contains(&ext) {
                continue;
            }

            let rel = path
                .strip_prefix(workspace)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            if let Ok(content) = std::fs::read_to_string(path) {
                let symbols = extract_symbols(&content, ext);
                let preview: String = content.chars().take(200).collect();
                entries.push(IndexEntry {
                    path: rel,
                    symbols,
                    preview,
                });
            }
        }

        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Self { entries }
    }

    pub fn summary(&self) -> String {
        let mut by_ext: HashMap<String, usize> = HashMap::new();
        for entry in &self.entries {
            let ext = Path::new(&entry.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("other")
                .to_string();
            *by_ext.entry(ext).or_insert(0) += 1;
        }

        let mut report = format!(
            "Project index: {} files indexed\n\nBy extension:\n",
            self.entries.len()
        );
        let mut exts: Vec<_> = by_ext.iter().collect();
        exts.sort_by_key(|(k, _)| *k);
        for (ext, count) in exts {
            report.push_str(&format!("  .{ext}: {count}\n"));
        }

        report.push_str("\nTop-level structure:\n");
        let mut top_dirs: HashMap<String, usize> = HashMap::new();
        for entry in &self.entries {
            let top = entry.path.split('/').next().unwrap_or(&entry.path);
            *top_dirs.entry(top.to_string()).or_insert(0) += 1;
        }
        let mut tops: Vec<_> = top_dirs.iter().collect();
        tops.sort_by(|a, b| b.1.cmp(a.1));
        for (dir, count) in tops.iter().take(15) {
            report.push_str(&format!("  {dir}/ ({count} files)\n"));
        }
        report
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, f32, String)> {
        let terms: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .filter(|t| t.len() > 1)
            .map(|t| t.to_string())
            .collect();

        if terms.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(String, f32, String)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let path_lower = entry.path.to_lowercase();
                let symbols_lower: String = entry.symbols.join(" ").to_lowercase();
                let preview_lower = entry.preview.to_lowercase();

                let mut score = 0.0f32;
                for term in &terms {
                    if path_lower.contains(term) {
                        score += 3.0;
                    }
                    if symbols_lower.contains(term) {
                        score += 2.0;
                    }
                    if preview_lower.contains(term) {
                        score += 1.0;
                    }
                }
                if score > 0.0 {
                    let match_detail = if !entry.symbols.is_empty() {
                        format!("symbols: {}", entry.symbols.join(", "))
                    } else {
                        entry.preview.chars().take(80).collect()
                    };
                    Some((entry.path.clone(), score, match_detail))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit);
        scored
    }

    pub fn file_count(&self) -> usize {
        self.entries.len()
    }
}

fn extract_symbols(content: &str, ext: &str) -> Vec<String> {
    let mut symbols = Vec::new();
    for line in content.lines().take(500) {
        let trimmed = line.trim();
        let symbol = match ext {
            "rs" if trimmed.starts_with("pub fn ") || trimmed.starts_with("fn ") => {
                Some(trimmed)
            }
            "rs" if trimmed.starts_with("pub struct ")
                || trimmed.starts_with("struct ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("enum ")
                || trimmed.starts_with("pub trait ")
                || trimmed.starts_with("trait ") =>
            {
                Some(trimmed)
            }
            "go" if trimmed.starts_with("func ") => Some(trimmed),
            "py" if trimmed.starts_with("def ") || trimmed.starts_with("class ") => Some(trimmed),
            "js" | "ts" | "tsx" | "jsx" if trimmed.starts_with("function ")
                || trimmed.starts_with("export function ")
                || trimmed.starts_with("class ")
                || trimmed.starts_with("export class ") =>
            {
                Some(trimmed)
            }
            _ => None,
        };
        if let Some(s) = symbol {
            symbols.push(s.chars().take(80).collect());
            if symbols.len() >= 20 {
                break;
            }
        }
    }
    symbols
}

type SharedIndex = Arc<RwLock<Option<ProjectIndex>>>;

pub struct ProjectIndexTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
    index: SharedIndex,
}

pub struct SearchCodebaseTool {
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
    index: SharedIndex,
}

pub fn new_index_tools(
    sandbox: Arc<Sandbox>,
    audit: Arc<AuditLogger>,
) -> (ProjectIndexTool, SearchCodebaseTool) {
    let index: SharedIndex = Arc::new(RwLock::new(None));
    (
        ProjectIndexTool {
            sandbox: sandbox.clone(),
            audit: audit.clone(),
            index: index.clone(),
        },
        SearchCodebaseTool {
            sandbox,
            audit,
            index,
        },
    )
}

impl ProjectIndexTool {
    fn ensure_index(&self) -> Result<(), ToolError> {
        let needs_build = self.index.read().map_err(|_| {
            ToolError::Execution("index lock poisoned".into())
        })?.is_none();

        if needs_build {
            let workspace = self.sandbox.workspace().to_path_buf();
            let built = ProjectIndex::build(&workspace);
            let mut guard = self.index.write().map_err(|_| {
                ToolError::Execution("index lock poisoned".into())
            })?;
            *guard = Some(built);
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for ProjectIndexTool {
    fn name(&self) -> &str {
        "project_index"
    }

    fn description(&self) -> &str {
        "Build or refresh a project index (file tree, symbols, structure summary)."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "refresh": {
                    "type": "boolean",
                    "description": "Force rebuild of the index",
                    "default": false
                }
            }
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let refresh = arguments["refresh"].as_bool().unwrap_or(false);
        if refresh {
            let mut guard = self.index.write().map_err(|_| {
                ToolError::Execution("index lock poisoned".into())
            })?;
            *guard = None;
        }
        self.ensure_index()?;

        let summary = self
            .index
            .read()
            .map_err(|_| ToolError::Execution("index lock poisoned".into()))?
            .as_ref()
            .map(|idx| idx.summary())
            .unwrap_or_else(|| "Index empty".into());

        self.audit
            .log("project_index", "build", json!({ "refresh": refresh }), true);

        Ok(ToolResult {
            output: summary,
            is_error: false,
        })
    }
}

#[async_trait]
impl Tool for SearchCodebaseTool {
    fn name(&self) -> &str {
        "search_codebase"
    }

    fn description(&self) -> &str {
        "Semantic-style search across the project index. Finds files by path, symbols, and content relevance."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "limit": { "type": "integer", "description": "Max results", "default": 10 }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let query = arguments["query"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("query required".into()))?;
        let limit = arguments["limit"].as_u64().unwrap_or(10) as usize;

        if let Ok(mut guard) = self.index.write() {
            if guard.is_none() {
                let workspace = self.sandbox.workspace().to_path_buf();
                *guard = Some(ProjectIndex::build(&workspace));
            }
        }

        let results = self
            .index
            .read()
            .map_err(|_| ToolError::Execution("index lock poisoned".into()))?
            .as_ref()
            .map(|idx| idx.search(query, limit))
            .unwrap_or_default();

        self.audit.log(
            "search_codebase",
            "search",
            json!({ "query": query, "results": results.len() }),
            true,
        );

        if results.is_empty() {
            return Ok(ToolResult {
                output: format!("No matches for '{query}'. Try project_index first or broaden query."),
                is_error: false,
            });
        }

        let output: Vec<String> = results
            .iter()
            .map(|(path, score, detail)| format!("{path} (score: {score:.1}) — {detail}"))
            .collect();

        Ok(ToolResult {
            output: output.join("\n"),
            is_error: false,
        })
    }
}
