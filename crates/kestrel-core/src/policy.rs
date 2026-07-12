//! The policy engine: allow/deny rules that gate the agent's tools.
//!
//! Autonomy needs guardrails. A [`Policy`] can disable whole tools (e.g. block
//! `run_command` or `install_tool`) and block commands matching dangerous
//! substrings. It's checked before every tool call in the agent loop; a denied
//! call returns an error the model sees and adapts to — no interactive prompt,
//! so it stays safe even during a long unattended run. Sensible destructive
//! patterns are blocked by default.

use crate::providers::ToolCall;
use serde::{Deserialize, Serialize};

/// Allow/deny rules for the agent's tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    /// Tool names the agent may not use at all.
    pub denied_tools: Vec<String>,
    /// Substrings that block a `run_command`/`git` command (case-insensitive).
    pub denied_patterns: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            denied_tools: Vec::new(),
            denied_patterns: default_denied_patterns(),
        }
    }
}

/// Destructive command fragments blocked out of the box.
pub fn default_denied_patterns() -> Vec<String> {
    [
        "rm -rf /",
        "rm -rf ~",
        "rm -rf *",
        ":(){",
        "mkfs",
        "format c:",
        "del /f /s /q",
        "rmdir /s /q c:",
        "shutdown",
        "reg delete",
        "> /dev/sd",
        "dd if=",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Policy {
    /// Check a tool call against the policy, returning why it is denied (if so).
    pub fn check(&self, call: &ToolCall) -> Result<(), String> {
        if self
            .denied_tools
            .iter()
            .any(|t| t.eq_ignore_ascii_case(&call.name))
        {
            return Err(format!("the '{}' tool is disabled by policy", call.name));
        }
        // Command-bearing tools are matched against the blocked patterns.
        let text = call
            .input
            .get("command")
            .or_else(|| call.input.get("args"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !text.is_empty() {
            let lower = text.to_lowercase();
            for pattern in &self.denied_patterns {
                let p = pattern.trim().to_lowercase();
                if !p.is_empty() && lower.contains(&p) {
                    return Err(format!(
                        "command blocked by policy rule \"{}\"",
                        pattern.trim()
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "1".to_string(),
            name: name.to_string(),
            input,
        }
    }

    #[test]
    fn default_blocks_destructive_commands_allows_safe() {
        let policy = Policy::default();
        assert!(policy
            .check(&call(
                "run_command",
                serde_json::json!({"command": "sudo rm -rf / --no-preserve-root"})
            ))
            .is_err());
        assert!(policy
            .check(&call(
                "run_command",
                serde_json::json!({"command": "cargo test"})
            ))
            .is_ok());
        assert!(policy
            .check(&call(
                "read_file",
                serde_json::json!({"path": "src/main.rs"})
            ))
            .is_ok());
    }

    #[test]
    fn denied_tools_are_rejected() {
        let policy = Policy {
            denied_tools: vec!["run_command".to_string()],
            denied_patterns: vec![],
        };
        assert!(policy
            .check(&call("run_command", serde_json::json!({"command": "ls"})))
            .is_err());
        assert!(policy
            .check(&call("read_file", serde_json::json!({"path": "a"})))
            .is_ok());
    }
}
