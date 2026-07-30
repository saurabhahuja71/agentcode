use anyhow::{Context, Result};
use forge_config::McpServerConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tracing::debug;

#[derive(Clone)]
pub struct McpClient {
    inner: Arc<McpClientInner>,
}

struct McpClientInner {
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<BufReader<ChildStdout>>,
    _child: Mutex<Child>,
    next_id: AtomicU64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Value,
}

#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Deserialize)]
struct JsonRpcResponse {
    id: Option<u64>,
    result: Option<Value>,
    error: Option<JsonRpcError>,
}

#[derive(Deserialize)]
struct JsonRpcError {
    message: String,
}

impl McpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning MCP server {}", config.name))?;

        let stdin = child.stdin.take().context("MCP stdin")?;
        let stdout = child.stdout.take().context("MCP stdout")?;

        let client = Self {
            inner: Arc::new(McpClientInner {
                stdin: Mutex::new(stdin),
                stdout: Mutex::new(BufReader::new(stdout)),
                _child: Mutex::new(child),
                next_id: AtomicU64::new(1),
            }),
        };

        client.initialize().await?;
        Ok(client)
    }

    async fn initialize(&self) -> Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "forge", "version": "0.1.0" }
        });
        let _ = self.request("initialize", Some(params)).await?;
        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpToolDef>> {
        let result = self.request("tools/list", None).await?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();

        let mut defs = Vec::new();
        for tool in tools {
            if let Ok(def) = serde_json::from_value::<McpToolDef>(tool) {
                defs.push(def);
            }
        }
        Ok(defs)
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });
        let result = self.request("tools/call", Some(params)).await?;

        if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
            let text: Vec<String> = content
                .iter()
                .filter_map(|item| {
                    if item.get("type")?.as_str()? == "text" {
                        item.get("text")?.as_str().map(|s| s.to_string())
                    } else {
                        None
                    }
                })
                .collect();
            return Ok(text.join("\n"));
        }

        Ok(result.to_string())
    }

    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let line = serde_json::to_string(&req)?;

        {
            let mut stdin = self.inner.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
        }

        loop {
            let mut response_line = String::new();
            {
                let mut stdout = self.inner.stdout.lock().await;
                stdout.read_line(&mut response_line).await?;
            }

            if response_line.trim().is_empty() {
                continue;
            }

            debug!(line = %response_line.trim(), "MCP response");
            let resp: JsonRpcResponse = serde_json::from_str(&response_line)
                .with_context(|| format!("parsing MCP response: {response_line}"))?;

            if resp.id != Some(id) {
                continue;
            }

            if let Some(err) = resp.error {
                anyhow::bail!("MCP error: {}", err.message);
            }

            return Ok(resp.result.unwrap_or(Value::Null));
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        let line = serde_json::to_string(&payload)?;
        let mut stdin = self.inner.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok(())
    }
}
