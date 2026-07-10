//! The verification runner: detect and execute a project's check/test/build
//! ladder and report structured results.
//!
//! This is the other half of the verified-diff wedge. Symbol extraction and
//! the context graph let Kestrel *propose* a change; this module lets Kestrel
//! *prove* it — by running the project's real formatter, linter, tests, and
//! build in a defined order and capturing exactly what happened (command,
//! exit code, duration, output tails). A change is not "done" until it passes
//! this ladder; that is what turns a reviewed diff into a verified one.
//!
//! The ladder short-circuits on the first failure, mirroring how a human
//! verifies: there is no point running the test suite if the code does not
//! format or compile.

use crate::inspect::ProjectMarker;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

/// How many trailing output lines to keep from each command's stdout/stderr.
const OUTPUT_TAIL_LINES: usize = 30;

/// One planned verification command (a shell command line) with a human label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyStep {
    pub label: String,
    pub command: String,
}

impl VerifyStep {
    fn new(label: &str, command: &str) -> Self {
        Self {
            label: label.to_string(),
            command: command.to_string(),
        }
    }
}

/// The outcome of running one verification step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub label: String,
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

/// The result of running a verification ladder.
#[derive(Debug, Clone)]
pub struct VerificationReport {
    pub steps: Vec<StepResult>,
    /// True only if every planned step that ran succeeded.
    pub passed: bool,
    /// Steps that were planned but skipped because an earlier step failed.
    pub skipped: Vec<VerifyStep>,
}

/// Derive an ordered verification ladder from a project's detected markers.
/// Prefers non-mutating check commands (e.g. `cargo fmt --check`). Pure and
/// deterministic, so the plan is testable without touching the filesystem.
pub fn plan_verification(markers: &[ProjectMarker]) -> Vec<VerifyStep> {
    let kinds: std::collections::BTreeSet<&str> = markers.iter().map(|m| m.kind.as_str()).collect();
    let mut steps = Vec::new();

    if kinds.contains("rust_cargo") {
        steps.push(VerifyStep::new("format", "cargo fmt --all -- --check"));
        steps.push(VerifyStep::new("test", "cargo test"));
    }

    if kinds.contains("node_package") {
        let runner = if kinds.contains("pnpm_lock") {
            "pnpm"
        } else if kinds.contains("yarn_lock") {
            "yarn"
        } else {
            "npm"
        };
        steps.push(VerifyStep::new("test", &format!("{runner} test")));
    }

    if kinds.contains("python_project") || kinds.contains("python_requirements") {
        steps.push(VerifyStep::new("test", "python -m pytest"));
    }

    if kinds.contains("go_module") {
        steps.push(VerifyStep::new("build", "go build ./..."));
        steps.push(VerifyStep::new("test", "go test ./..."));
    }

    if kinds.contains("dotnet_solution") || kinds.contains("dotnet_project") {
        steps.push(VerifyStep::new("build", "dotnet build"));
        steps.push(VerifyStep::new("test", "dotnet test"));
    }

    steps
}

/// Run a verification ladder in `root`, short-circuiting on the first failure.
/// Each command is executed through the platform shell so that `PATH`/`PATHEXT`
/// resolution (e.g. `npm.cmd` on Windows) and shell builtins work.
pub fn run_verification(root: &Path, steps: &[VerifyStep]) -> VerificationReport {
    let mut results = Vec::new();
    let mut passed = true;
    let mut skipped = Vec::new();

    for step in steps {
        if !passed {
            skipped.push(step.clone());
            continue;
        }

        let start = Instant::now();
        let output = shell_command(&step.command).current_dir(root).output();
        let duration_ms = start.elapsed().as_millis();

        let result = match output {
            Ok(out) => StepResult {
                label: step.label.clone(),
                command: step.command.clone(),
                success: out.status.success(),
                exit_code: out.status.code(),
                duration_ms,
                stdout_tail: tail(&String::from_utf8_lossy(&out.stdout)),
                stderr_tail: tail(&String::from_utf8_lossy(&out.stderr)),
            },
            Err(err) => StepResult {
                label: step.label.clone(),
                command: step.command.clone(),
                success: false,
                exit_code: None,
                duration_ms,
                stdout_tail: String::new(),
                stderr_tail: format!("failed to launch command: {err}"),
            },
        };

        if !result.success {
            passed = false;
        }
        results.push(result);
    }

    VerificationReport {
        steps: results,
        passed,
        skipped,
    }
}

/// Build a shell command for the current platform.
fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", command]);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", command]);
        cmd
    }
}

/// Keep the last `OUTPUT_TAIL_LINES` lines of `text`, trimmed.
fn tail(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = trimmed.lines().collect();
    let start = lines.len().saturating_sub(OUTPUT_TAIL_LINES);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn marker(kind: &str) -> ProjectMarker {
        ProjectMarker {
            kind: kind.to_string(),
            path: PathBuf::from(kind),
        }
    }

    #[test]
    fn rust_plan_checks_format_then_tests() {
        let steps = plan_verification(&[marker("rust_cargo")]);
        let labels: Vec<_> = steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["format", "test"]);
        assert!(steps[0].command.contains("--check"));
    }

    #[test]
    fn node_plan_uses_detected_runner() {
        let steps = plan_verification(&[marker("node_package"), marker("pnpm_lock")]);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].command, "pnpm test");
    }

    #[test]
    fn empty_project_has_no_plan() {
        assert!(plan_verification(&[]).is_empty());
    }

    #[test]
    fn runner_captures_success_and_failure_exit_codes() {
        let root = std::env::temp_dir();
        let pass = run_verification(&root, &[VerifyStep::new("ok", "exit 0")]);
        assert!(pass.passed);
        assert_eq!(pass.steps[0].exit_code, Some(0));

        let fail = run_verification(&root, &[VerifyStep::new("bad", "exit 3")]);
        assert!(!fail.passed);
        assert!(!fail.steps[0].success);
        assert_eq!(fail.steps[0].exit_code, Some(3));
    }

    #[test]
    fn ladder_short_circuits_after_first_failure() {
        let root = std::env::temp_dir();
        let report = run_verification(
            &root,
            &[
                VerifyStep::new("first", "exit 1"),
                VerifyStep::new("second", "exit 0"),
            ],
        );
        assert!(!report.passed);
        assert_eq!(report.steps.len(), 1); // second never ran
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].label, "second");
    }
}
