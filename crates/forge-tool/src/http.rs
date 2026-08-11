use crate::{Tool, ToolError, ToolResult};
use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{json, Value};

const MAX_RESPONSE_BYTES: usize = 16 * 1024;

/// Bounded HTTP client for API checks and local service integration.
pub struct HttpRequestTool {
    client: reqwest::Client,
}

impl HttpRequestTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("HTTP client configuration is valid"),
        }
    }

    fn validate_url(url: &str) -> Result<(), ToolError> {
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| ToolError::InvalidArgs(format!("invalid url: {e}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ToolError::InvalidArgs("url must use http or https".into()));
        }
        Ok(())
    }

    fn headers(value: Option<&Value>) -> Result<HeaderMap, ToolError> {
        let mut headers = HeaderMap::new();
        let Some(object) = value.and_then(Value::as_object) else {
            return Ok(headers);
        };
        for (name, value) in object {
            let header_name = HeaderName::try_from(name.as_str())
                .map_err(|e| ToolError::InvalidArgs(format!("invalid header name: {e}")))?;
            let header_value = value
                .as_str()
                .ok_or_else(|| ToolError::InvalidArgs(format!("header {name} must be a string")))?;
            headers.insert(
                header_name,
                HeaderValue::try_from(header_value).map_err(|e| {
                    ToolError::InvalidArgs(format!("invalid value for header {name}: {e}"))
                })?,
            );
        }
        Ok(headers)
    }
}

impl Default for HttpRequestTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for HttpRequestTool {
    fn name(&self) -> &str {
        "http_request"
    }

    fn description(&self) -> &str {
        "Make a bounded HTTP GET, POST, PUT, PATCH, or DELETE request and return status, headers, and response text."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE"] },
                "url": { "type": "string", "description": "Absolute http:// or https:// URL" },
                "headers": { "type": "object", "additionalProperties": { "type": "string" } },
                "body": { "type": "string", "description": "Optional request body" }
            },
            "required": ["url"]
        })
    }

    async fn execute(&self, arguments: Value) -> Result<ToolResult, ToolError> {
        let url = arguments["url"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArgs("url required".into()))?;
        Self::validate_url(url)?;

        let method = arguments["method"].as_str().unwrap_or("GET").to_uppercase();
        let method = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| ToolError::InvalidArgs(format!("invalid method: {e}")))?;
        let request = self
            .client
            .request(method, url)
            .headers(Self::headers(arguments.get("headers"))?);
        let request = if let Some(body) = arguments["body"].as_str() {
            request.body(body.to_owned())
        } else {
            request
        };

        let response = request
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("HTTP request failed: {e}")))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .text()
            .await
            .map_err(|e| ToolError::Execution(format!("reading HTTP response failed: {e}")))?;
        let body = if body.len() > MAX_RESPONSE_BYTES {
            format!(
                "{}\n[response truncated at {MAX_RESPONSE_BYTES} bytes]",
                &body[..MAX_RESPONSE_BYTES]
            )
        } else {
            body
        };
        let content_type = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown");

        Ok(ToolResult {
            output: format!(
                "status: {}\ncontent_type: {content_type}\n--- body ---\n{body}",
                status.as_u16()
            ),
            is_error: !status.is_success(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HttpRequestTool;

    #[test]
    fn only_allows_http_schemes() {
        assert!(HttpRequestTool::validate_url("http://localhost:11434/api/tags").is_ok());
        assert!(HttpRequestTool::validate_url("https://example.com").is_ok());
        assert!(HttpRequestTool::validate_url("file:///etc/passwd").is_err());
    }
}
