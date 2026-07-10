//! Model provider abstraction.
//!
//! Kestrel talks to two API shapes, which between them cover the providers we
//! target: the **Anthropic Messages API** and the **OpenAI-compatible Chat
//! Completions API** (OpenAI, DeepSeek, and Kimi/Moonshot all speak the latter,
//! differing only in base URL, key, and model names). This module builds the
//! right request for each and parses the reply, so the rest of Kestrel is
//! provider-agnostic.
//!
//! Requests go over the system `curl` (piped via stdin, no temp files), so no
//! bundled HTTP/TLS stack is required.

use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use std::process::{Command, Stdio};

/// The API shape a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Anthropic Messages API (`/v1/messages`, `x-api-key`).
    Anthropic,
    /// OpenAI-compatible Chat Completions (`/chat/completions`, bearer token).
    Openai,
}

/// One message in a chat request.
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

/// A provider-agnostic chat request.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub max_tokens: u64,
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
}

/// The credentials and endpoint for one configured provider.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: String,
}

/// The endpoint URL for a request of this provider kind.
pub fn endpoint(kind: ProviderKind, base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    match kind {
        ProviderKind::Anthropic => format!("{base}/v1/messages"),
        ProviderKind::Openai => format!("{base}/chat/completions"),
    }
}

/// The HTTP headers for a request of this provider kind.
pub fn headers(kind: ProviderKind, api_key: &str) -> Vec<(String, String)> {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    match kind {
        ProviderKind::Anthropic => {
            headers.push(("x-api-key".to_string(), api_key.to_string()));
            headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
        }
        ProviderKind::Openai => {
            headers.push(("authorization".to_string(), format!("Bearer {api_key}")));
        }
    }
    headers
}

/// Build the JSON request body for a provider kind. For OpenAI-compatible
/// providers the system prompt becomes a leading `system` message.
pub fn build_body(kind: ProviderKind, request: &ChatRequest) -> serde_json::Value {
    match kind {
        ProviderKind::Anthropic => {
            let messages: Vec<serde_json::Value> = request
                .messages
                .iter()
                .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
                .collect();
            let mut body = serde_json::json!({
                "model": request.model,
                "max_tokens": request.max_tokens,
                "messages": messages,
            });
            if let Some(system) = &request.system {
                body["system"] = serde_json::json!(system);
            }
            body
        }
        ProviderKind::Openai => {
            let mut messages = Vec::new();
            if let Some(system) = &request.system {
                messages.push(serde_json::json!({ "role": "system", "content": system }));
            }
            for m in &request.messages {
                messages.push(serde_json::json!({ "role": m.role, "content": m.content }));
            }
            serde_json::json!({
                "model": request.model,
                "max_tokens": request.max_tokens,
                "messages": messages,
            })
        }
    }
}

/// Extract the assistant text from a provider response, or an error message.
pub fn parse_response(kind: ProviderKind, response: &serde_json::Value) -> Result<String, String> {
    if let Some(message) = response.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(message.to_string());
    }
    match kind {
        ProviderKind::Anthropic => {
            if response.get("stop_reason").and_then(|s| s.as_str()) == Some("refusal") {
                return Err("the model declined this request (refusal)".to_string());
            }
            let mut text = String::new();
            if let Some(blocks) = response.get("content").and_then(|c| c.as_array()) {
                for block in blocks {
                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                            text.push_str(t);
                        }
                    }
                }
            }
            Ok(text)
        }
        ProviderKind::Openai => response
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(str::to_string)
            .ok_or_else(|| "response had no choices[0].message.content".to_string()),
    }
}

/// Send a chat request to the configured provider and return the reply text.
pub fn chat(config: &ProviderConfig, request: &ChatRequest) -> io::Result<Result<String, String>> {
    if config.api_key.trim().is_empty() {
        return Ok(Err("no API key configured for this provider".to_string()));
    }
    let url = endpoint(config.kind, &config.base_url);
    let header_args = headers(config.kind, &config.api_key);
    let body = build_body(config.kind, request);
    let raw = post_json(&url, &header_args, &body)?;
    let response: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(err) => {
            return Ok(Err(format!(
                "unexpected response (not JSON): {err}\n{}",
                String::from_utf8_lossy(&raw)
            )))
        }
    };
    Ok(parse_response(config.kind, &response))
}

/// POST a JSON body to `url` with the given headers via the system `curl`,
/// piping the body through stdin (no temp files). Returns raw response bytes.
fn post_json(
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
) -> io::Result<Vec<u8>> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let mut command = Command::new("curl");
    command.arg("-sS").arg(url);
    for (key, value) in headers {
        command.arg("-H").arg(format!("{key}: {value}"));
    }
    command.args(["--data-binary", "@-"]);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn()?;
    child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("failed to open curl stdin"))?
        .write_all(body_str.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "curl request failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> ChatRequest {
        ChatRequest {
            model: "m".to_string(),
            max_tokens: 100,
            system: Some("be terse".to_string()),
            messages: vec![ChatMessage::user("hi")],
        }
    }

    #[test]
    fn endpoints_are_provider_specific() {
        assert_eq!(
            endpoint(ProviderKind::Anthropic, "https://api.anthropic.com/"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            endpoint(ProviderKind::Openai, "https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn anthropic_headers_use_x_api_key() {
        let h = headers(ProviderKind::Anthropic, "sk-x");
        assert!(h.iter().any(|(k, v)| k == "x-api-key" && v == "sk-x"));
        assert!(h.iter().any(|(k, _)| k == "anthropic-version"));
    }

    #[test]
    fn openai_headers_use_bearer() {
        let h = headers(ProviderKind::Openai, "sk-x");
        assert!(h
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer sk-x"));
    }

    #[test]
    fn anthropic_body_keeps_system_top_level() {
        let body = build_body(ProviderKind::Anthropic, &request());
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body.get("max_tokens").is_some());
    }

    #[test]
    fn openai_body_prepends_system_message() {
        let body = build_body(ProviderKind::Openai, &request());
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "be terse");
        assert_eq!(body["messages"][1]["role"], "user");
    }

    #[test]
    fn parse_anthropic_text() {
        let resp = serde_json::json!({
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn"
        });
        assert_eq!(
            parse_response(ProviderKind::Anthropic, &resp).unwrap(),
            "hello"
        );
    }

    #[test]
    fn parse_openai_text() {
        let resp = serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": "hi there"}}]
        });
        assert_eq!(
            parse_response(ProviderKind::Openai, &resp).unwrap(),
            "hi there"
        );
    }

    #[test]
    fn parse_surfaces_api_errors() {
        let resp = serde_json::json!({"error": {"message": "bad key"}});
        assert_eq!(
            parse_response(ProviderKind::Openai, &resp).unwrap_err(),
            "bad key"
        );
    }
}
