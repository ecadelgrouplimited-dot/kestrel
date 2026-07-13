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
use std::io::{self, BufRead, BufReader, Read, Write};
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
                body["system"] = serde_json::json!([{
                    "type": "text",
                    "text": system,
                    "cache_control": { "type": "ephemeral" },
                }]);
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

/// Stream a chat completion, calling `on_token` with each text delta as it
/// arrives (Server-Sent Events over `curl -N`), and returning the full text.
/// Text-only: tool calls are not streamed (the agent loop uses `run_turn`).
pub fn chat_stream(
    config: &ProviderConfig,
    request: &ChatRequest,
    mut on_token: impl FnMut(&str),
) -> io::Result<Result<(String, Usage), String>> {
    if config.api_key.trim().is_empty() {
        return Ok(Err("no API key configured for this provider".to_string()));
    }
    let url = endpoint(config.kind, &config.base_url);
    let header_args = headers(config.kind, &config.api_key);
    let mut body = build_body(config.kind, request);
    body["stream"] = serde_json::json!(true);
    if config.kind == ProviderKind::Openai {
        // Ask OpenAI-compatible providers to emit a final usage chunk.
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    let kind = config.kind;
    let mut full = String::new();
    let mut usage = Usage::default();
    let mut error: Option<String> = None;

    post_json_stream(&url, &header_args, &body, |line| {
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            return;
        };
        if let Some(message) = value.pointer("/error/message").and_then(|m| m.as_str()) {
            error = Some(message.to_string());
            return;
        }
        accumulate_stream_usage(kind, &value, &mut usage);
        let delta = match kind {
            ProviderKind::Anthropic => {
                if value.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
                    value.pointer("/delta/text").and_then(|t| t.as_str())
                } else {
                    None
                }
            }
            ProviderKind::Openai => value
                .pointer("/choices/0/delta/content")
                .and_then(|c| c.as_str()),
        };
        if let Some(text) = delta {
            if !text.is_empty() {
                full.push_str(text);
                on_token(text);
            }
        }
    })?;

    if let Some(message) = error {
        return Ok(Err(message));
    }
    Ok(Ok((full, usage)))
}

/// Update running `usage` from a single streamed SSE event.
fn accumulate_stream_usage(kind: ProviderKind, value: &serde_json::Value, usage: &mut Usage) {
    let u = |ptr: &str| value.pointer(ptr).and_then(|v| v.as_u64());
    match kind {
        ProviderKind::Anthropic => match value.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => {
                usage.input_tokens = u("/message/usage/input_tokens").unwrap_or(usage.input_tokens);
                usage.cache_read =
                    u("/message/usage/cache_read_input_tokens").unwrap_or(usage.cache_read);
                usage.cache_write =
                    u("/message/usage/cache_creation_input_tokens").unwrap_or(usage.cache_write);
            }
            Some("message_delta") => {
                if let Some(o) = u("/usage/output_tokens") {
                    usage.output_tokens = o;
                }
            }
            _ => {}
        },
        ProviderKind::Openai => {
            if value.get("usage").is_some() {
                let cached = u("/usage/prompt_tokens_details/cached_tokens").unwrap_or(0);
                let prompt = u("/usage/prompt_tokens").unwrap_or(0);
                usage.input_tokens = prompt.saturating_sub(cached);
                usage.cache_read = cached;
                usage.output_tokens = u("/usage/completion_tokens").unwrap_or(usage.output_tokens);
            }
        }
    }
}

/// How many times to attempt a request before giving up.
const CURL_MAX_ATTEMPTS: usize = 3;

/// Whether a curl exit code is a transient network failure worth retrying —
/// connection resets/timeouts/empty replies, not auth or malformed-request
/// errors. Notably 35 (SSL/TLS handshake) and 56 (recv failure: reset), which
/// are the flaky-connection cases users hit mid-run.
fn curl_is_transient(code: Option<i32>) -> bool {
    matches!(code, Some(6 | 7 | 16 | 18 | 28 | 35 | 52 | 55 | 56 | 92))
}

/// Build a curl command for a POST to `url` with `headers`, reading the body
/// from stdin. `stream` toggles `-N` (unbuffered) for SSE.
fn curl_command(url: &str, headers: &[(String, String)], stream: bool) -> Command {
    let mut command = Command::new("curl");
    command.arg("-sS");
    if stream {
        command.arg("-N");
    }
    // Bound the handshake/idle waits so a dead connection fails fast enough to
    // retry rather than hanging the turn.
    command.args(["--connect-timeout", "30"]);
    command.arg(url);
    for (key, value) in headers {
        command.arg("-H").arg(format!("{key}: {value}"));
    }
    command.args(["--data-binary", "@-"]);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    command
}

/// Sleep a short, growing backoff between retry attempts.
fn retry_backoff(attempt: usize) {
    std::thread::sleep(std::time::Duration::from_millis(400 * (attempt as u64 + 1)));
}

/// POST a JSON body and stream the response body line by line to `on_line`
/// (used for SSE). Retries transient connection failures, but only while nothing
/// has been streamed yet — once lines have been delivered a retry would double
/// them, so a mid-stream drop surfaces as an error.
fn post_json_stream(
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
    mut on_line: impl FnMut(&str),
) -> io::Result<()> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let mut last_err = String::new();

    for attempt in 0..CURL_MAX_ATTEMPTS {
        let mut child = curl_command(url, headers, true).spawn()?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| io::Error::other("failed to open curl stdin"))?;
            // Tolerate a broken pipe (server hung up early) — the curl exit code
            // below carries the real error.
            let _ = stdin.write_all(body_str.as_bytes());
        }
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("failed to open curl stdout"))?;
        let mut delivered = false;
        for line in BufReader::new(stdout).lines() {
            delivered = true;
            on_line(&line?);
        }
        let status = child.wait()?;
        if status.success() {
            return Ok(());
        }
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        last_err = stderr.trim().to_string();
        // Only safe to retry if we streamed nothing yet.
        if !delivered && attempt + 1 < CURL_MAX_ATTEMPTS && curl_is_transient(status.code()) {
            retry_backoff(attempt);
            continue;
        }
        break;
    }
    Err(io::Error::other(format!("curl request failed: {last_err}")))
}

/// POST a JSON body to `url` with the given headers via the system `curl`,
/// piping the body through stdin (no temp files). Retries transient connection
/// failures (resets/timeouts) with a short backoff. Returns raw response bytes.
fn post_json(
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
) -> io::Result<Vec<u8>> {
    let body_str = serde_json::to_string(body).unwrap_or_default();
    let mut last_err = String::new();

    for attempt in 0..CURL_MAX_ATTEMPTS {
        let mut child = curl_command(url, headers, false).spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            // Tolerate a broken pipe; the exit code carries the real error.
            let _ = stdin.write_all(body_str.as_bytes());
        }
        let output = child.wait_with_output()?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        last_err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if attempt + 1 < CURL_MAX_ATTEMPTS && curl_is_transient(output.status.code()) {
            retry_backoff(attempt);
            continue;
        }
        break;
    }
    Err(io::Error::other(format!("curl request failed: {last_err}")))
}

// ---------------------------------------------------------------------------
// Tool-using agent turns.
//
// The plain `chat` path above sends text and gets text back. An agent also
// needs to advertise *tools*, receive the model's requests to call them, run
// them, and feed the results back over multiple turns. Anthropic and OpenAI
// represent tool calls differently on the wire, so we keep a provider-agnostic
// representation here and translate at the edges.
// ---------------------------------------------------------------------------

/// A tool the model may call, described by a JSON Schema for its input.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// The result of running a tool, to feed back to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub content: String,
}

/// One message in an agent conversation (richer than [`ChatMessage`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMessage {
    User(String),
    Assistant { text: String, calls: Vec<ToolCall> },
    ToolResults(Vec<ToolResult>),
}

/// Token usage for a request, including prompt-cache accounting. `input_tokens`
/// is fresh (uncached) input; `cache_read` was served from cache (~10% cost);
/// `cache_write` was written to cache (~25% surcharge).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// Total input tokens (fresh + cache read + cache write).
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_read + self.cache_write
    }

    /// Accumulate another usage into this one.
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

/// The outcome of a single model turn in an agent loop.
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub stop_reason: String,
    pub usage: Usage,
}

/// Build the request body for one agent turn (tools + tool-aware messages).
pub fn build_agent_body(
    kind: ProviderKind,
    model: &str,
    max_tokens: u64,
    system: Option<&str>,
    messages: &[AgentMessage],
    tools: &[ToolSpec],
) -> serde_json::Value {
    match kind {
        ProviderKind::Anthropic => {
            let mut msgs: Vec<serde_json::Value> = messages.iter().map(anthropic_message).collect();
            // Prompt caching: a rolling breakpoint on the last message caches the
            // whole conversation prefix, so repeated agent turns re-read it at
            // ~10% cost instead of re-billing the full history each turn.
            if let Some(last) = msgs.last_mut() {
                mark_cache_control(last);
            }
            let tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect();
            let mut body = serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": msgs,
                "tools": tool_defs,
            });
            if let Some(system) = system {
                // Cache the (large, constant) system + tools prefix.
                body["system"] = serde_json::json!([{
                    "type": "text",
                    "text": system,
                    "cache_control": { "type": "ephemeral" },
                }]);
            }
            body
        }
        ProviderKind::Openai => {
            let mut msgs = Vec::new();
            if let Some(system) = system {
                msgs.push(serde_json::json!({ "role": "system", "content": system }));
            }
            for message in messages {
                openai_messages(message, &mut msgs);
            }
            let tool_defs: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        },
                    })
                })
                .collect();
            serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "messages": msgs,
                "tools": tool_defs,
            })
        }
    }
}

/// Attach an ephemeral cache-control breakpoint to a message's last content
/// block (Anthropic prompt caching). No-op if the content isn't a block array.
fn mark_cache_control(message: &mut serde_json::Value) {
    if let Some(content) = message.get_mut("content").and_then(|c| c.as_array_mut()) {
        if let Some(last) = content.last_mut().and_then(|b| b.as_object_mut()) {
            last.insert(
                "cache_control".to_string(),
                serde_json::json!({ "type": "ephemeral" }),
            );
        }
    }
}

fn anthropic_message(message: &AgentMessage) -> serde_json::Value {
    match message {
        AgentMessage::User(text) => serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": text }],
        }),
        AgentMessage::Assistant { text, calls } => {
            let mut content = Vec::new();
            if !text.is_empty() {
                content.push(serde_json::json!({ "type": "text", "text": text }));
            }
            for call in calls {
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.name,
                    "input": call.input,
                }));
            }
            serde_json::json!({ "role": "assistant", "content": content })
        }
        AgentMessage::ToolResults(results) => {
            let content: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "type": "tool_result",
                        "tool_use_id": r.id,
                        "content": r.content,
                    })
                })
                .collect();
            serde_json::json!({ "role": "user", "content": content })
        }
    }
}

fn openai_messages(message: &AgentMessage, out: &mut Vec<serde_json::Value>) {
    match message {
        AgentMessage::User(text) => {
            out.push(serde_json::json!({ "role": "user", "content": text }))
        }
        AgentMessage::Assistant { text, calls } => {
            let tool_calls: Vec<serde_json::Value> = calls
                .iter()
                .map(|call| {
                    serde_json::json!({
                        "id": call.id,
                        "type": "function",
                        "function": {
                            "name": call.name,
                            "arguments": serde_json::to_string(&call.input)
                                .unwrap_or_else(|_| "{}".to_string()),
                        },
                    })
                })
                .collect();
            let mut msg = serde_json::json!({
                "role": "assistant",
                "content": if text.is_empty() { serde_json::Value::Null } else { serde_json::json!(text) },
            });
            if !tool_calls.is_empty() {
                msg["tool_calls"] = serde_json::json!(tool_calls);
            }
            out.push(msg);
        }
        AgentMessage::ToolResults(results) => {
            for r in results {
                out.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": r.id,
                    "content": r.content,
                }));
            }
        }
    }
}

/// Parse a provider response into a [`TurnResult`] (assistant text + calls).
pub fn parse_turn(kind: ProviderKind, response: &serde_json::Value) -> Result<TurnResult, String> {
    if let Some(message) = response.pointer("/error/message").and_then(|m| m.as_str()) {
        return Err(message.to_string());
    }
    match kind {
        ProviderKind::Anthropic => {
            let stop_reason = response
                .get("stop_reason")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let mut text = String::new();
            let mut calls = Vec::new();
            if let Some(blocks) = response.get("content").and_then(|c| c.as_array()) {
                for block in blocks {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            text.push_str(block.get("text").and_then(|t| t.as_str()).unwrap_or(""))
                        }
                        Some("tool_use") => calls.push(ToolCall {
                            id: block
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or("")
                                .to_string(),
                            name: block
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input: block.get("input").cloned().unwrap_or(serde_json::json!({})),
                        }),
                        _ => {}
                    }
                }
            }
            Ok(TurnResult {
                text,
                calls,
                stop_reason,
                usage: parse_usage(kind, response),
            })
        }
        ProviderKind::Openai => {
            let message = response.pointer("/choices/0/message");
            let text = message
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string();
            let stop_reason = response
                .pointer("/choices/0/finish_reason")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let mut calls = Vec::new();
            if let Some(tool_calls) = message
                .and_then(|m| m.get("tool_calls"))
                .and_then(|t| t.as_array())
            {
                for call in tool_calls {
                    let args = call
                        .pointer("/function/arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}");
                    calls.push(ToolCall {
                        id: call
                            .get("id")
                            .and_then(|i| i.as_str())
                            .unwrap_or("")
                            .to_string(),
                        name: call
                            .pointer("/function/name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("")
                            .to_string(),
                        input: serde_json::from_str(args).unwrap_or(serde_json::json!({})),
                    });
                }
            }
            Ok(TurnResult {
                text,
                calls,
                stop_reason,
                usage: parse_usage(kind, response),
            })
        }
    }
}

/// Extract token usage (including cache accounting) from a full response.
pub fn parse_usage(kind: ProviderKind, response: &serde_json::Value) -> Usage {
    let u = |ptr: &str| response.pointer(ptr).and_then(|v| v.as_u64()).unwrap_or(0);
    match kind {
        ProviderKind::Anthropic => Usage {
            input_tokens: u("/usage/input_tokens"),
            output_tokens: u("/usage/output_tokens"),
            cache_read: u("/usage/cache_read_input_tokens"),
            cache_write: u("/usage/cache_creation_input_tokens"),
        },
        ProviderKind::Openai => {
            let cached = u("/usage/prompt_tokens_details/cached_tokens");
            let prompt = u("/usage/prompt_tokens");
            Usage {
                input_tokens: prompt.saturating_sub(cached),
                output_tokens: u("/usage/completion_tokens"),
                cache_read: cached,
                cache_write: 0,
            }
        }
    }
}

/// Run one agent turn: send the tool-aware conversation and parse the reply.
pub fn run_turn(
    config: &ProviderConfig,
    model: &str,
    max_tokens: u64,
    system: Option<&str>,
    messages: &[AgentMessage],
    tools: &[ToolSpec],
) -> io::Result<Result<TurnResult, String>> {
    if config.api_key.trim().is_empty() {
        return Ok(Err("no API key configured for this provider".to_string()));
    }
    let url = endpoint(config.kind, &config.base_url);
    let header_args = headers(config.kind, &config.api_key);
    let body = build_agent_body(config.kind, model, max_tokens, system, messages, tools);
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
    Ok(parse_turn(config.kind, &response))
}

/// A live event from a streaming agent turn, for real-time display.
pub enum TurnEvent<'a> {
    /// A chunk of the model's narration text.
    Text(&'a str),
    /// A tool call's arguments *so far* (the accumulated JSON string). Emitted on
    /// every delta, so a consumer can show a file being written as it streams.
    ToolProgress { name: &'a str, args: &'a str },
}

/// A tool call being assembled from a stream.
#[derive(Default)]
struct StreamTool {
    id: String,
    name: String,
    args: String,
}

/// Accumulates a streamed turn (text + tool calls + usage) into a [`TurnResult`].
#[derive(Default)]
struct StreamState {
    text: String,
    stop_reason: String,
    usage: Usage,
    tools: std::collections::BTreeMap<usize, StreamTool>,
}

impl StreamState {
    /// Fold one parsed SSE event into the state, emitting live events.
    fn handle(
        &mut self,
        kind: ProviderKind,
        value: &serde_json::Value,
        on_event: &mut dyn FnMut(TurnEvent),
    ) {
        match kind {
            ProviderKind::Anthropic => self.handle_anthropic(value, on_event),
            ProviderKind::Openai => self.handle_openai(value, on_event),
        }
    }

    fn handle_anthropic(&mut self, value: &serde_json::Value, on_event: &mut dyn FnMut(TurnEvent)) {
        match value.get("type").and_then(|t| t.as_str()) {
            Some("content_block_start") => {
                let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let block = value.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(|t| t.as_str()) == Some("tool_use") {
                    let id = block
                        .and_then(|b| b.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    let name = block
                        .and_then(|b| b.get("name"))
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.tools.insert(
                        index,
                        StreamTool {
                            id,
                            name,
                            args: String::new(),
                        },
                    );
                }
            }
            Some("content_block_delta") => {
                let index = value.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let delta = value.get("delta");
                match delta.and_then(|d| d.get("type")).and_then(|t| t.as_str()) {
                    Some("text_delta") => {
                        if let Some(t) = delta.and_then(|d| d.get("text")).and_then(|t| t.as_str())
                        {
                            self.text.push_str(t);
                            on_event(TurnEvent::Text(t));
                        }
                    }
                    Some("input_json_delta") => {
                        if let Some(pj) = delta
                            .and_then(|d| d.get("partial_json"))
                            .and_then(|p| p.as_str())
                        {
                            if let Some(tool) = self.tools.get_mut(&index) {
                                tool.args.push_str(pj);
                            }
                            if let Some(tool) = self.tools.get(&index) {
                                on_event(TurnEvent::ToolProgress {
                                    name: &tool.name,
                                    args: &tool.args,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(reason) = value.pointer("/delta/stop_reason").and_then(|s| s.as_str()) {
                    self.stop_reason = reason.to_string();
                }
            }
            _ => {}
        }
    }

    fn handle_openai(&mut self, value: &serde_json::Value, on_event: &mut dyn FnMut(TurnEvent)) {
        if let Some(t) = value
            .pointer("/choices/0/delta/content")
            .and_then(|c| c.as_str())
        {
            if !t.is_empty() {
                self.text.push_str(t);
                on_event(TurnEvent::Text(t));
            }
        }
        if let Some(reason) = value
            .pointer("/choices/0/finish_reason")
            .and_then(|s| s.as_str())
        {
            self.stop_reason = reason.to_string();
        }
        if let Some(calls) = value
            .pointer("/choices/0/delta/tool_calls")
            .and_then(|t| t.as_array())
        {
            for call in calls {
                let index = call.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                let tool = self.tools.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(|i| i.as_str()) {
                    if !id.is_empty() {
                        tool.id = id.to_string();
                    }
                }
                if let Some(name) = call.pointer("/function/name").and_then(|n| n.as_str()) {
                    if !name.is_empty() {
                        tool.name = name.to_string();
                    }
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(|a| a.as_str()) {
                    tool.args.push_str(args);
                }
                let tool = &self.tools[&index];
                on_event(TurnEvent::ToolProgress {
                    name: &tool.name,
                    args: &tool.args,
                });
            }
        }
    }

    /// Assemble the final turn once the stream is complete.
    fn into_turn(self) -> TurnResult {
        let calls = self
            .tools
            .into_values()
            .filter(|t| !t.name.is_empty())
            .map(|t| ToolCall {
                id: t.id,
                name: t.name,
                input: serde_json::from_str(&t.args).unwrap_or_else(|_| serde_json::json!({})),
            })
            .collect();
        TurnResult {
            text: self.text,
            calls,
            stop_reason: self.stop_reason,
            usage: self.usage,
        }
    }
}

/// Run one agent turn with **streaming**, emitting [`TurnEvent`]s as text and
/// tool arguments arrive, and returning the assembled [`TurnResult`]. Mirrors
/// [`run_turn`] but lets the UI show a file being written in real time.
#[allow(clippy::too_many_arguments)]
pub fn run_turn_streaming(
    config: &ProviderConfig,
    model: &str,
    max_tokens: u64,
    system: Option<&str>,
    messages: &[AgentMessage],
    tools: &[ToolSpec],
    mut on_event: impl FnMut(TurnEvent),
) -> io::Result<Result<TurnResult, String>> {
    if config.api_key.trim().is_empty() {
        return Ok(Err("no API key configured for this provider".to_string()));
    }
    let url = endpoint(config.kind, &config.base_url);
    let header_args = headers(config.kind, &config.api_key);
    let mut body = build_agent_body(config.kind, model, max_tokens, system, messages, tools);
    body["stream"] = serde_json::json!(true);
    if config.kind == ProviderKind::Openai {
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }

    let kind = config.kind;
    let mut state = StreamState::default();
    let mut api_error: Option<String> = None;

    post_json_stream(&url, &header_args, &body, |line| {
        let Some(data) = line.trim().strip_prefix("data:") else {
            return;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
            return;
        };
        if let Some(message) = value.pointer("/error/message").and_then(|m| m.as_str()) {
            api_error = Some(message.to_string());
            return;
        }
        accumulate_stream_usage(kind, &value, &mut state.usage);
        state.handle(kind, &value, &mut on_event);
    })?;

    if let Some(message) = api_error {
        return Ok(Err(message));
    }
    Ok(Ok(state.into_turn()))
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
    fn anthropic_body_caches_system_prompt() {
        let body = build_body(ProviderKind::Anthropic, &request());
        // System is a cache-controlled text block for prompt caching.
        assert_eq!(body["system"][0]["text"], "be terse");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
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

    #[test]
    fn agent_body_advertises_tools_per_kind() {
        let tools = vec![ToolSpec {
            name: "read_file".to_string(),
            description: "read".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        }];
        let messages = vec![AgentMessage::User("hi".to_string())];
        let anthropic = build_agent_body(
            ProviderKind::Anthropic,
            "m",
            100,
            Some("sys"),
            &messages,
            &tools,
        );
        assert_eq!(anthropic["tools"][0]["name"], "read_file");
        assert_eq!(anthropic["system"][0]["text"], "sys");
        assert_eq!(anthropic["system"][0]["cache_control"]["type"], "ephemeral");
        // The last message carries a rolling cache breakpoint.
        assert_eq!(
            anthropic["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );

        let openai = build_agent_body(
            ProviderKind::Openai,
            "m",
            100,
            Some("sys"),
            &messages,
            &tools,
        );
        assert_eq!(openai["tools"][0]["type"], "function");
        assert_eq!(openai["tools"][0]["function"]["name"], "read_file");
        assert_eq!(openai["messages"][0]["role"], "system");
    }

    #[test]
    fn parse_usage_reads_cache_tokens_both_shapes() {
        let anthropic = serde_json::json!({
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_read_input_tokens": 900,
                "cache_creation_input_tokens": 100
            }
        });
        let u = parse_usage(ProviderKind::Anthropic, &anthropic);
        assert_eq!(u.input_tokens, 10);
        assert_eq!(u.output_tokens, 20);
        assert_eq!(u.cache_read, 900);
        assert_eq!(u.cache_write, 100);
        assert_eq!(u.total_input(), 1010);

        let openai = serde_json::json!({
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 50,
                "prompt_tokens_details": { "cached_tokens": 800 }
            }
        });
        let u = parse_usage(ProviderKind::Openai, &openai);
        assert_eq!(u.input_tokens, 200);
        assert_eq!(u.cache_read, 800);
        assert_eq!(u.output_tokens, 50);
    }

    #[test]
    fn parse_turn_reads_tool_calls_both_shapes() {
        let anthropic = serde_json::json!({
            "stop_reason": "tool_use",
            "content": [
                {"type": "text", "text": "let me look"},
                {"type": "tool_use", "id": "t1", "name": "read_file", "input": {"path": "a.rs"}}
            ]
        });
        let turn = parse_turn(ProviderKind::Anthropic, &anthropic).unwrap();
        assert_eq!(turn.text, "let me look");
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.calls[0].name, "read_file");
        assert_eq!(turn.calls[0].input["path"], "a.rs");

        let openai = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "read_file", "arguments": "{\"path\": \"a.rs\"}"}
                    }]
                }
            }]
        });
        let turn = parse_turn(ProviderKind::Openai, &openai).unwrap();
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.calls[0].name, "read_file");
        assert_eq!(turn.calls[0].input["path"], "a.rs");
    }

    #[test]
    fn transient_curl_codes_retry_but_auth_errors_dont() {
        // 35 (TLS) and 56 (recv reset) are the flaky-connection cases.
        assert!(curl_is_transient(Some(35)));
        assert!(curl_is_transient(Some(56)));
        assert!(curl_is_transient(Some(28)));
        // A malformed URL (3) or auth-level failure is not retried.
        assert!(!curl_is_transient(Some(3)));
        assert!(!curl_is_transient(Some(22)));
        assert!(!curl_is_transient(None));
    }

    #[test]
    fn stream_state_assembles_anthropic_tool_call_with_live_events() {
        let events = [
            serde_json::json!({"type": "content_block_start", "index": 0,
                "content_block": {"type": "tool_use", "id": "t1", "name": "write_file"}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"path\":\"a.txt\",\"cont"}}),
            serde_json::json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "ents\":\"hello\"}"}}),
            serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        ];
        let mut state = StreamState::default();
        let mut progress_seen = 0;
        for ev in &events {
            state.handle(ProviderKind::Anthropic, ev, &mut |e| {
                if matches!(e, TurnEvent::ToolProgress { .. }) {
                    progress_seen += 1;
                }
            });
        }
        assert!(progress_seen >= 1, "should emit live tool progress");
        let turn = state.into_turn();
        assert_eq!(turn.stop_reason, "tool_use");
        assert_eq!(turn.calls.len(), 1);
        assert_eq!(turn.calls[0].name, "write_file");
        assert_eq!(turn.calls[0].input["path"], "a.txt");
        assert_eq!(turn.calls[0].input["contents"], "hello");
    }
}
