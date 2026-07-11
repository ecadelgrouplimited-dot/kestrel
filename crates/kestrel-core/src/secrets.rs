//! A lightweight, dependency-free secret scanner.
//!
//! Before the agent's changes "land" (get committed), Kestrel flags lines that
//! look like leaked credentials — provider API keys, cloud keys, private-key
//! blocks, or a `secret =`-style assignment holding a high-entropy literal.
//! It is deliberately heuristic (no regex dependency): a *warning* surface, not
//! a gate, tuned to catch the obvious mistakes while ignoring placeholders.

/// One suspected secret, by file and line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretFinding {
    pub path: String,
    pub line: usize,
    pub kind: String,
}

/// Scan the given project-relative files for likely secrets, capped for size.
pub fn scan_secrets(root: &std::path::Path, rel_paths: &[String]) -> Vec<SecretFinding> {
    let mut findings = Vec::new();
    for rel in rel_paths {
        let full = root.join(rel);
        if std::fs::metadata(&full)
            .map(|m| m.len() > 1_000_000)
            .unwrap_or(true)
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&full) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if let Some(kind) = detect_secret(line) {
                findings.push(SecretFinding {
                    path: rel.clone(),
                    line: i + 1,
                    kind: kind.to_string(),
                });
                if findings.len() >= 200 {
                    return findings;
                }
            }
        }
    }
    findings
}

/// Classify a single line, returning the kind of secret if one is detected.
pub fn detect_secret(line: &str) -> Option<&'static str> {
    if line.contains("PRIVATE KEY-----") {
        return Some("private key");
    }
    if prefixed_token(line, "sk-ant-", 20) {
        return Some("Anthropic API key");
    }
    if prefixed_token(line, "ghp_", 20) || prefixed_token(line, "gho_", 20) {
        return Some("GitHub token");
    }
    if prefixed_token(line, "xoxb-", 10) || prefixed_token(line, "xoxp-", 10) {
        return Some("Slack token");
    }
    if prefixed_token(line, "AIza", 30) {
        return Some("Google API key");
    }
    if aws_access_key(line) {
        return Some("AWS access key");
    }
    if prefixed_token(line, "sk-", 24) {
        return Some("API key");
    }
    let lower = line.to_lowercase();
    let keyworded = ["api_key", "apikey", "secret", "token", "password", "passwd"]
        .iter()
        .any(|k| lower.contains(k));
    if keyworded && contains_secret_literal(line) {
        return Some("possible hardcoded secret");
    }
    None
}

/// True if `prefix` is present and followed by at least `min` token characters.
fn prefixed_token(line: &str, prefix: &str, min: usize) -> bool {
    let Some(idx) = line.find(prefix) else {
        return false;
    };
    let rest = &line[idx + prefix.len()..];
    rest.chars().take_while(|c| is_token_char(*c)).count() >= min
}

/// True if the line contains `AKIA` followed by 16 uppercase alphanumerics.
fn aws_access_key(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(pos) = line[i..].find("AKIA") {
        let start = i + pos + 4;
        if start + 16 <= bytes.len()
            && bytes[start..start + 16]
                .iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        {
            return true;
        }
        i = start;
    }
    false
}

fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// True if the line assigns a high-entropy string literal (a likely secret),
/// ignoring obvious placeholders.
fn contains_secret_literal(line: &str) -> bool {
    for quote in ['"', '\''] {
        if let Some(start) = line.find(quote) {
            if let Some(rel_end) = line[start + 1..].find(quote) {
                let value = &line[start + 1..start + 1 + rel_end];
                if is_secretish(value) {
                    return true;
                }
            }
        }
    }
    false
}

fn is_secretish(value: &str) -> bool {
    let value = value.trim();
    if value.len() < 20 || value.contains(' ') {
        return false;
    }
    let lower = value.to_lowercase();
    for placeholder in [
        "example",
        "your-",
        "your_",
        "placeholder",
        "xxxx",
        "...",
        "changeme",
    ] {
        if lower.contains(placeholder) {
            return false;
        }
    }
    let has_alpha = value.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = value.chars().any(|c| c.is_ascii_digit());
    has_alpha && has_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_provider_and_generic_secrets() {
        assert_eq!(
            detect_secret("const k = \"sk-ant-api03-abcDEF123456ghiJKL789mnop\";"),
            Some("Anthropic API key")
        );
        assert_eq!(
            detect_secret("AWS_KEY=AKIAIOSFODNN7EXAMPLE1"),
            Some("AWS access key")
        );
        assert_eq!(
            detect_secret("api_key: \"a1b2c3d4e5f6g7h8i9j0k1l2\""),
            Some("possible hardcoded secret")
        );
    }

    #[test]
    fn ignores_placeholders_and_plain_prose() {
        assert_eq!(
            detect_secret("api_key = \"your-api-key-here-please\""),
            None
        );
        assert_eq!(
            detect_secret("let name = \"just a normal string value\";"),
            None
        );
        assert_eq!(detect_secret("// set your token in the settings"), None);
    }

    #[test]
    fn scans_files_on_disk() {
        let dir = std::env::temp_dir().join(format!("kestrel-secrets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.ts"),
            "export const token = \"ghp_ABCdef0123456789ABCdef0123456789ABCD\";\n",
        )
        .unwrap();
        let findings = scan_secrets(&dir, &["config.ts".to_string()]);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 1);
        assert_eq!(findings[0].kind, "GitHub token");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
