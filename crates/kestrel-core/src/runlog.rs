//! Turning raw tool output into something a person can read at a glance.
//!
//! A forty-line pytest dump buries its own headline. This module extracts the
//! one line that matters — "7 tests passed", "build failed: 3 errors" — and
//! whether it went well, so the UI can show a green or red badge and keep the
//! detail folded away until someone wants it.

/// How a step turned out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Failed,
    /// Ran, but there's nothing to celebrate or worry about.
    Info,
}

/// A one-line verdict for a block of tool output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub status: Status,
    pub headline: String,
}

impl Summary {
    fn ok(headline: impl Into<String>) -> Self {
        Self {
            status: Status::Ok,
            headline: headline.into(),
        }
    }
    fn failed(headline: impl Into<String>) -> Self {
        Self {
            status: Status::Failed,
            headline: headline.into(),
        }
    }
    fn info(headline: impl Into<String>) -> Self {
        Self {
            status: Status::Info,
            headline: headline.into(),
        }
    }
}

/// Whether a block of text looks like captured tool output rather than the
/// model's own prose — worth folding into a card.
pub fn is_tool_output(text: &str) -> bool {
    text.starts_with("exit code:")
        || text.contains("\n--- stdout ---")
        || text.contains("\n--- stderr ---")
        || text.starts_with("--- stdout ---")
        || text.starts_with("✅ PASS")
        || text.starts_with("❌ FAIL")
        || text.contains("test result:")
}

/// Extract the headline verdict from a block of tool output.
pub fn summarize_output(text: &str) -> Option<Summary> {
    // Our own acceptance checks already speak plainly.
    if let Some(line) = text.lines().find(|l| l.starts_with("✅ PASS")) {
        return Some(Summary::ok(trim_headline(line.trim_start_matches("✅ "))));
    }
    if let Some(line) = text.lines().find(|l| l.starts_with("❌ FAIL")) {
        return Some(Summary::failed(trim_headline(
            line.trim_start_matches("❌ "),
        )));
    }

    // Rust: "test result: ok. 7 passed; 0 failed" / "FAILED"
    if let Some(rest) = text.split("test result: ").nth(1) {
        let counts = rest.lines().next().unwrap_or("");
        let passed = number_before(counts, "passed").unwrap_or(0);
        let failed = number_before(counts, "failed").unwrap_or(0);
        return Some(if counts.starts_with("ok") && failed == 0 {
            Summary::ok(format!("{passed} tests passed"))
        } else {
            Summary::failed(format!("{failed} of {} tests failed", passed + failed))
        });
    }

    // pytest: "7 passed in 0.86s" / "2 failed, 5 passed in 1.2s"
    if let Some(line) = text
        .lines()
        .rev()
        .find(|l| l.contains(" passed") || l.contains(" failed") || l.contains(" error"))
    {
        let cleaned = line.trim().trim_matches('=').trim();
        if let Some(failed) = number_before(cleaned, "failed") {
            if failed > 0 {
                return Some(Summary::failed(trim_headline(cleaned)));
            }
        }
        if let Some(passed) = number_before(cleaned, "passed") {
            if passed > 0 {
                return Some(Summary::ok(trim_headline(cleaned)));
            }
        }
    }

    // Compiler/type errors anywhere in the output.
    let errors = text
        .lines()
        .filter(|l| l.trim_start().starts_with("error[") || l.trim_start().starts_with("error:"))
        .count();
    if errors > 0 {
        return Some(Summary::failed(format!(
            "{errors} error{}",
            if errors == 1 { "" } else { "s" }
        )));
    }

    // Fall back to the process exit code.
    if let Some(rest) = text.split("exit code: ").nth(1) {
        let code: i32 = rest
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .parse()
            .unwrap_or(-1);
        return Some(if code == 0 {
            Summary::ok("completed")
        } else {
            Summary::failed(format!("exited with code {code}"))
        });
    }
    if text.trim().is_empty() {
        return None;
    }
    Some(Summary::info("output"))
}

/// The integer immediately before `word`, e.g. `7` in "7 passed".
fn number_before(text: &str, word: &str) -> Option<u32> {
    let idx = text.find(word)?;
    text[..idx]
        .split_whitespace()
        .next_back()?
        .trim_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .ok()
}

/// Keep a headline short enough for a badge.
fn trim_headline(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= 90 {
        return line.to_string();
    }
    let cut: String = line.chars().take(87).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_test_results() {
        let ok = "running 7 tests\ntest result: ok. 7 passed; 0 failed; 0 ignored";
        assert_eq!(summarize_output(ok).unwrap(), Summary::ok("7 tests passed"));

        let bad = "test result: FAILED. 5 passed; 2 failed; 0 ignored";
        let s = summarize_output(bad).unwrap();
        assert_eq!(s.status, Status::Failed);
        assert!(s.headline.contains("2 of 7"));
    }

    #[test]
    fn pytest_results() {
        let ok = "collecting ... collected 7 items\n\n===== 7 passed in 0.86s =====";
        let s = summarize_output(ok).unwrap();
        assert_eq!(s.status, Status::Ok);
        assert!(s.headline.contains("7 passed"));

        let bad = "===== 2 failed, 5 passed in 1.20s =====";
        assert_eq!(summarize_output(bad).unwrap().status, Status::Failed);
    }

    #[test]
    fn compiler_errors_and_exit_codes() {
        let errs = "error[E0308]: mismatched types\nerror: aborting";
        let s = summarize_output(errs).unwrap();
        assert_eq!(s.status, Status::Failed);
        assert_eq!(s.headline, "2 errors");

        assert_eq!(
            summarize_output("exit code: 0\n--- stdout ---\nhi").unwrap(),
            Summary::ok("completed")
        );
        assert_eq!(
            summarize_output("exit code: 1\n--- stderr ---\nboom")
                .unwrap()
                .status,
            Status::Failed
        );
    }

    #[test]
    fn our_own_acceptance_checks_pass_through() {
        let pass = "✅ PASS — http://localhost:8000 (rendered): all 3 expected item(s) present.";
        let s = summarize_output(pass).unwrap();
        assert_eq!(s.status, Status::Ok);
        assert!(s.headline.starts_with("PASS"));

        let fail = "❌ FAIL — report.docx (120 words): missing \"Budget\".";
        assert_eq!(summarize_output(fail).unwrap().status, Status::Failed);
    }

    #[test]
    fn recognises_tool_output_versus_prose() {
        assert!(is_tool_output("exit code: 0\n--- stdout ---\nok"));
        assert!(is_tool_output("✅ PASS — everything present"));
        assert!(!is_tool_output(
            "I'll start by researching the current Polars API."
        ));
        assert_eq!(summarize_output("   "), None);
    }

    #[test]
    fn headlines_stay_short() {
        let long = format!("✅ PASS — {}", "x".repeat(200));
        let s = summarize_output(&long).unwrap();
        assert!(s.headline.chars().count() <= 90);
    }
}
