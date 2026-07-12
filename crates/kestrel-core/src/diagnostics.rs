//! Inline diagnostics: run the project's compiler/checker and parse its errors
//! into structured, jump-to-able findings — the pragmatic, dependency-free
//! alternative to a full LSP client. Detects the toolchain (cargo / tsc / ruff),
//! runs it, and parses the output. The parsers are unit-tested against real
//! output shapes; running the actual compilers is the UI's job (on a thread).

use std::path::Path;
use std::process::Command;

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn icon(self) -> &'static str {
        match self {
            Severity::Error => "🔴",
            Severity::Warning => "🟡",
        }
    }
}

/// One diagnostic at a source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Project-relative file path (as the tool reported it).
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    pub message: String,
}

/// Run the appropriate checker for the project and return its diagnostics.
pub fn run_diagnostics(root: &Path) -> Vec<Diagnostic> {
    if root.join("Cargo.toml").exists() {
        parse_cargo_short(&capture(root, "cargo check --message-format=short --quiet"))
    } else if root.join("tsconfig.json").exists() {
        parse_tsc(&capture(root, "npx tsc --noEmit --pretty false"))
    } else if root.join("pyproject.toml").exists()
        || root.join("requirements.txt").exists()
        || root.join("setup.py").exists()
    {
        parse_ruff(&capture(root, "ruff check --output-format concise ."))
    } else {
        Vec::new()
    }
}

/// The label describing which checker `run_diagnostics` would use.
pub fn checker_name(root: &Path) -> Option<&'static str> {
    if root.join("Cargo.toml").exists() {
        Some("cargo check")
    } else if root.join("tsconfig.json").exists() {
        Some("tsc --noEmit")
    } else if root.join("pyproject.toml").exists()
        || root.join("requirements.txt").exists()
        || root.join("setup.py").exists()
    {
        Some("ruff check")
    } else {
        None
    }
}

/// Run a shell command in `root`, returning combined stdout+stderr.
fn capture(root: &Path, command: &str) -> String {
    let output = if cfg!(windows) {
        Command::new("cmd")
            .args(["/C", command])
            .current_dir(root)
            .output()
    } else {
        Command::new("sh")
            .args(["-c", command])
            .current_dir(root)
            .output()
    };
    match output {
        Ok(out) => format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        Err(_) => String::new(),
    }
}

/// Parse `cargo check --message-format=short` output
/// (`src/main.rs:10:5: error[E0308]: mismatched types`).
pub fn parse_cargo_short(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let (severity, sev_idx) = if let Some(i) = line.find(": error") {
            (Severity::Error, i)
        } else if let Some(i) = line.find(": warning") {
            (Severity::Warning, i)
        } else {
            continue;
        };
        let location: Vec<&str> = line[..sev_idx].rsplitn(3, ':').collect();
        if location.len() < 3 {
            continue;
        }
        let (Ok(col), Ok(line_no)) = (location[0].trim().parse(), location[1].trim().parse())
        else {
            continue;
        };
        let rest = &line[sev_idx + 2..];
        let message = rest
            .split_once(": ")
            .map(|(_, m)| m)
            .unwrap_or(rest)
            .trim()
            .to_string();
        out.push(Diagnostic {
            file: location[2].trim().to_string(),
            line: line_no,
            col,
            severity,
            message,
        });
        if out.len() >= 500 {
            break;
        }
    }
    out
}

/// Parse `tsc --noEmit --pretty false` output
/// (`src/x.ts(10,5): error TS2322: message`).
pub fn parse_tsc(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(open) = line.find('(') else { continue };
        let Some(close_rel) = line[open..].find(')') else {
            continue;
        };
        let close = open + close_rel;
        let file = line[..open].trim().to_string();
        let mut loc = line[open + 1..close].split(',');
        let (Some(line_s), col_s) = (loc.next(), loc.next()) else {
            continue;
        };
        let Ok(line_no) = line_s.trim().parse() else {
            continue;
        };
        let col = col_s.and_then(|c| c.trim().parse().ok()).unwrap_or(1);
        let after = line[close + 1..].trim_start_matches(':').trim();
        let severity = if after.starts_with("error") {
            Severity::Error
        } else if after.starts_with("warning") {
            Severity::Warning
        } else {
            continue;
        };
        let message = after
            .split_once(": ")
            .map(|(_, m)| m)
            .unwrap_or(after)
            .trim()
            .to_string();
        out.push(Diagnostic {
            file,
            line: line_no,
            col,
            severity,
            message,
        });
        if out.len() >= 500 {
            break;
        }
    }
    out
}

/// Parse `ruff check --output-format concise` output
/// (`src/app.py:3:1: F401 'os' imported but unused`). Lints are warnings.
pub fn parse_ruff(text: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.splitn(4, ':');
        let (Some(file), Some(line_s), Some(col_s), Some(rest)) =
            (it.next(), it.next(), it.next(), it.next())
        else {
            continue;
        };
        let (Ok(line_no), Ok(col)) = (line_s.trim().parse(), col_s.trim().parse()) else {
            continue;
        };
        out.push(Diagnostic {
            file: file.trim().to_string(),
            line: line_no,
            col,
            severity: Severity::Warning,
            message: rest.trim().to_string(),
        });
        if out.len() >= 500 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cargo_short() {
        let text = "src/main.rs:10:5: error[E0308]: mismatched types\n\
                    src/lib.rs:3:1: warning: unused import: `std::fmt`\n\
                    Compiling kestrel v0.1.0";
        let d = parse_cargo_short(text);
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].file, "src/main.rs");
        assert_eq!(d[0].line, 10);
        assert_eq!(d[0].col, 5);
        assert_eq!(d[0].severity, Severity::Error);
        assert!(d[0].message.contains("mismatched types"));
        assert_eq!(d[1].severity, Severity::Warning);
    }

    #[test]
    fn parses_tsc() {
        let text =
            "src/App.tsx(12,3): error TS2322: Type 'string' is not assignable to type 'number'.";
        let d = parse_tsc(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "src/App.tsx");
        assert_eq!(d[0].line, 12);
        assert_eq!(d[0].col, 3);
        assert_eq!(d[0].severity, Severity::Error);
        assert!(d[0].message.contains("not assignable"));
    }

    #[test]
    fn parses_ruff() {
        let text = "app/main.py:3:1: F401 [*] `os` imported but unused";
        let d = parse_ruff(text);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].file, "app/main.py");
        assert_eq!(d[0].line, 3);
        assert_eq!(d[0].severity, Severity::Warning);
        assert!(d[0].message.contains("imported but unused"));
    }
}
