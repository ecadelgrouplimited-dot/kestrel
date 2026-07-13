//! Source formatting across languages.
//!
//! Kestrel's editor "Format" action isn't Rust-only: it dispatches to the right
//! system formatter for the file's language — `rustfmt`, `gofmt`, `black`,
//! `prettier` — piping the buffer through the tool's stdin/stdout. Each is
//! optional; if the tool isn't installed we say so instead of failing silently.

use std::io::Write;
use std::process::{Command, Stdio};

/// A formatter chosen for a file: the program to run and its arguments.
pub struct Formatter {
    pub program: &'static str,
    pub args: Vec<String>,
    /// Human label for status messages (e.g. "prettier").
    pub label: &'static str,
}

/// The formatter for a file by its name/extension, if one is configured.
pub fn formatter_for(filename: &str) -> Option<Formatter> {
    let ext = std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let f = |program: &'static str, args: &[&str], label: &'static str| Formatter {
        program,
        args: args.iter().map(|s| s.to_string()).collect(),
        label,
    };
    Some(match ext.as_str() {
        "rs" => f("rustfmt", &["--edition", "2021"], "rustfmt"),
        "go" => f("gofmt", &[], "gofmt"),
        "py" | "pyi" => f("black", &["-q", "-"], "black"),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "json" | "jsonc" | "css" | "scss"
        | "less" | "html" | "vue" | "svelte" | "md" | "markdown" | "yaml" | "yml" | "graphql" => {
            Formatter {
                program: "prettier",
                args: vec!["--stdin-filepath".to_string(), filename.to_string()],
                label: "prettier",
            }
        }
        _ => return None,
    })
}

/// Whether any formatter is configured for this file type.
pub fn can_format(filename: &str) -> bool {
    formatter_for(filename).is_some()
}

/// Format `source` for `filename` with its configured formatter, returning the
/// formatted text or a human-readable error (including "not installed").
pub fn format_source(filename: &str, source: &str) -> Result<String, String> {
    let Some(fmt) = formatter_for(filename) else {
        return Err("No formatter is configured for this file type.".to_string());
    };
    let mut child = spawn(fmt.program, &fmt.args)
        .map_err(|_| format!("{} isn't installed or on PATH.", fmt.label))?;
    // Write the buffer to stdin, then close it (drop) so the tool proceeds.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "could not open the formatter's stdin".to_string())?;
        stdin
            .write_all(source.as_bytes())
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(format!("{} failed: {}", fmt.label, err.trim()))
    }
}

/// Spawn `program args…` with piped stdio. On Windows, npm-installed tools like
/// `prettier` are `.cmd` shims that `Command::new` won't find by bare name, so
/// retry with a `.cmd` suffix.
fn spawn(program: &str, args: &[String]) -> std::io::Result<std::process::Child> {
    let build = |prog: &str| {
        Command::new(prog)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };
    match build(program) {
        Ok(child) => Ok(child),
        Err(err) if cfg!(windows) && err.kind() == std::io::ErrorKind::NotFound => {
            build(&format!("{program}.cmd"))
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatches_by_extension() {
        assert_eq!(formatter_for("main.rs").unwrap().program, "rustfmt");
        assert_eq!(formatter_for("app.py").unwrap().program, "black");
        assert_eq!(formatter_for("server.go").unwrap().program, "gofmt");
        assert_eq!(formatter_for("index.tsx").unwrap().program, "prettier");
        assert_eq!(formatter_for("styles.css").unwrap().program, "prettier");
        assert!(formatter_for("data.bin").is_none());
        assert!(can_format("README.md"));
        assert!(!can_format("archive.zip"));
    }

    #[test]
    fn prettier_gets_the_filename_for_parser_selection() {
        let fmt = formatter_for("component.vue").unwrap();
        assert!(fmt.args.iter().any(|a| a == "component.vue"));
    }
}
