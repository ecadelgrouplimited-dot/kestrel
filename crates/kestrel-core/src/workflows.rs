//! Autonomous verified workflows: named, reusable agent recipes.
//!
//! A [`Workflow`] is a titled goal — a prompt that drives the tool-using agent,
//! with optional `{param}` slots the user fills. The built-ins turn the roadmap's
//! specialized agents (release readiness, security remediation, migration,
//! incident triage, …) into one uniform mechanism, and user workflows persist to
//! `<config>/kestrel/workflows.toml` — a shareable file that seeds a workflow
//! marketplace. Running a workflow is just running the agent loop with its
//! filled prompt, so every workflow inherits verification, checkpoints, policy,
//! and budgets for free.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// A named, reusable agent recipe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The agent instruction, possibly containing `{param}` placeholders.
    pub prompt: String,
    /// Named parameters the user fills before running.
    #[serde(default)]
    pub params: Vec<String>,
}

impl Workflow {
    /// Substitute `{param}` placeholders with the supplied values.
    pub fn fill(&self, values: &BTreeMap<String, String>) -> String {
        let mut prompt = self.prompt.clone();
        for (key, value) in values {
            prompt = prompt.replace(&format!("{{{key}}}"), value);
        }
        prompt
    }
}

/// The built-in workflows — the roadmap's specialized agents as recipes.
pub fn builtin_workflows() -> Vec<Workflow> {
    let w = |id: &str, name: &str, description: &str, prompt: &str, params: &[&str]| Workflow {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        prompt: prompt.to_string(),
        params: params.iter().map(|s| s.to_string()).collect(),
    };
    vec![
        w(
            "release-readiness",
            "Release readiness",
            "Assess whether the project is ready to ship and fix trivial blockers.",
            "Assess this project's release readiness and produce a concise report. Run the \
             build and tests (verify() or run_command), search for TODO/FIXME/HACK and obvious \
             debug/leftover code, run `git status` to check for uncommitted changes, scan for \
             hardcoded secrets, and confirm the README's run instructions match reality. Fix any \
             trivial blockers you find and re-verify. Finish with a clear summary: what is ready, \
             what is risky, and what MUST be done before release.",
            &[],
        ),
        w(
            "security-remediation",
            "Security remediation",
            "Review for security issues and remediate what's safe to auto-fix.",
            "Do a security review of this project and remediate what is safe to fix \
             automatically. Look for: hardcoded secrets/keys, injection risks (SQL, command, \
             path), missing authentication/authorization checks, unsafe deserialization, and \
             obviously risky dependencies. For each finding, explain the risk; fix the safe ones \
             with edit_file/write_file and clearly FLAG anything too risky to auto-fix for human \
             review. Verify the build/tests still pass after your changes, then summarize the \
             findings and what you changed.",
            &[],
        ),
        w(
            "migrate",
            "Migration agent",
            "Migrate the project from one framework/version/library to another.",
            "Migrate this project from {from} to {to}. First inspect the codebase and outline a \
             migration plan. Then apply the changes incrementally with edit_file/write_file, and \
             after each meaningful step run the build/tests (run_command/verify) to keep it green \
             — fix breakages as you go. Prefer small, verified steps over one big rewrite. Finish \
             with a summary of what changed and any manual follow-ups the team must handle.",
            &["from", "to"],
        ),
        w(
            "incident",
            "Incident assistant",
            "Find the root cause of an error/incident and propose (and apply) a fix.",
            "An incident occurred. Here is the error or log output:\n\n{error}\n\nFind the root \
             cause in THIS codebase — use search and read_file to trace it. Explain the cause \
             plainly, then propose and apply a fix with edit_file/write_file, and verify the \
             build/tests pass. If the fix is risky or needs a decision, apply the safest \
             mitigation and flag the rest for a human.",
            &["error"],
        ),
        w(
            "add-tests",
            "Raise test coverage",
            "Add focused tests for important untested code.",
            "Improve this project's automated test coverage. Identify important logic that lacks \
             tests (use search and the symbol/graph structure), write focused, meaningful tests \
             for it using the project's existing test framework and conventions, and run them \
             (run_command/verify) to confirm they pass. Summarize what you added and why.",
            &[],
        ),
        w(
            "update-deps",
            "Update dependencies",
            "Safely update dependencies and fix any breakage.",
            "Update this project's dependencies safely. Check for outdated packages, update them \
             (prefer minor/patch unless a major is clearly safe), then run the build and tests \
             (run_command/verify) and fix any breakage you introduced. Summarize what was \
             updated and flag any updates that need manual attention.",
            &[],
        ),
    ]
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WorkflowFile {
    #[serde(default)]
    workflows: Vec<Workflow>,
}

/// The path to the user's saved workflows.
pub fn workflows_path() -> PathBuf {
    crate::settings::config_dir()
        .join("kestrel")
        .join("workflows.toml")
}

/// Load the user's saved workflows (empty if none/invalid).
pub fn load_user_workflows() -> Vec<Workflow> {
    load_user_workflows_from(&workflows_path())
}

/// Load user workflows from a specific path (used by tests).
pub fn load_user_workflows_from(path: &std::path::Path) -> Vec<Workflow> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str::<WorkflowFile>(&t).ok())
        .map(|f| f.workflows)
        .unwrap_or_default()
}

/// Persist the user's workflows.
pub fn save_user_workflows(workflows: &[Workflow]) -> std::io::Result<()> {
    save_user_workflows_to(&workflows_path(), workflows)
}

/// Persist user workflows to a specific path (used by tests).
pub fn save_user_workflows_to(
    path: &std::path::Path,
    workflows: &[Workflow],
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = WorkflowFile {
        workflows: workflows.to_vec(),
    };
    let text = toml::to_string_pretty(&file)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// All workflows: the built-ins, then the user's (a user workflow with the same
/// id overrides the built-in).
pub fn all_workflows() -> Vec<Workflow> {
    let user = load_user_workflows();
    let mut out: Vec<Workflow> = builtin_workflows()
        .into_iter()
        .filter(|b| !user.iter().any(|u| u.id == b.id))
        .collect();
    out.extend(user);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_substitutes_params() {
        let wf = &builtin_workflows()
            .into_iter()
            .find(|w| w.id == "migrate")
            .unwrap();
        let mut values = BTreeMap::new();
        values.insert("from".to_string(), "Express".to_string());
        values.insert("to".to_string(), "Fastify".to_string());
        let filled = wf.fill(&values);
        assert!(filled.contains("from Express to Fastify"));
        assert!(!filled.contains("{from}"));
    }

    #[test]
    fn user_workflows_round_trip_and_override() {
        let dir = std::env::temp_dir().join(format!("kestrel-wf-{}", std::process::id()));
        let path = dir.join("workflows.toml");
        let custom = vec![Workflow {
            id: "release-readiness".to_string(),
            name: "My release check".to_string(),
            description: "custom".to_string(),
            prompt: "do it".to_string(),
            params: vec![],
        }];
        save_user_workflows_to(&path, &custom).unwrap();
        let loaded = load_user_workflows_from(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "My release check");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
