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
use std::path::{Component, Path, PathBuf};

/// A template file a workflow drops into the project before it runs — the thing
/// that turns a prompt-only recipe into a shareable **skill pack** (e.g. a CI
/// config, a Dockerfile, a starter module).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResource {
    /// Project-relative path (absolute paths and `..` are refused).
    pub path: String,
    pub contents: String,
}

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
    /// Template files materialized into the project before the run (skill packs).
    #[serde(default)]
    pub resources: Vec<WorkflowResource>,
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
        resources: Vec::new(),
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

/// The curated catalog: extra ready-made recipes a user can install into their
/// own set with one click. Unlike the built-ins (always present), these are the
/// "marketplace" gallery — installing one copies it into the user's workflows so
/// it appears in the list and can be edited.
pub fn catalog_workflows() -> Vec<Workflow> {
    let w = |id: &str, name: &str, description: &str, prompt: &str, params: &[&str]| Workflow {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        prompt: prompt.to_string(),
        params: params.iter().map(|s| s.to_string()).collect(),
        resources: Vec::new(),
    };
    vec![
        w(
            "document-project",
            "Document the project",
            "Write/refresh a clear README and inline docs for the public API.",
            "Improve this project's documentation. Inspect the codebase to understand what it \
             does, then write or refresh a clear README (what it is, how to install, run, and \
             test, and a short architecture overview) and add concise doc comments to the main \
             public functions/types. Keep it accurate to the actual code — verify commands you \
             mention. Summarize what you documented.",
            &[],
        ),
        w(
            "performance-pass",
            "Performance pass",
            "Find and fix obvious performance problems, verified.",
            "Do a performance review of this project. Look for obvious problems: needless \
             allocations/clones in hot paths, N+1 queries, work repeated in loops, missing \
             caching, sync I/O on hot paths, and inefficient algorithms/data structures. Fix the \
             safe, clear wins with edit_file and explain each; FLAG risky changes for review. Keep \
             the build/tests green (run_command/verify). Summarize the improvements and their \
             expected impact.",
            &[],
        ),
        w(
            "accessibility-audit",
            "Accessibility audit",
            "Audit a web UI for accessibility and fix the clear issues.",
            "Audit this project's user interface for accessibility (WCAG). Check for missing alt \
             text, unlabeled form controls, poor color contrast, missing ARIA roles/landmarks, \
             keyboard-navigation gaps, and non-semantic markup. Fix the clear issues with \
             edit_file and flag anything that needs a design decision. Verify the build still \
             passes and summarize what you fixed.",
            &[],
        ),
        w(
            "dockerize",
            "Dockerize",
            "Add a production-ready Dockerfile (and compose) for the project.",
            "Containerize this project. Detect its stack and add a correct, production-ready \
             multi-stage Dockerfile (small final image, non-root user, sensible caching), a \
             .dockerignore, and — if it needs services (db, cache) — a docker-compose.yml. Do not \
             run Docker; just produce correct files and document how to build and run them in the \
             README. Summarize what you added.",
            &[],
        ),
        w(
            "api-from-spec",
            "Build API from a spec",
            "Scaffold endpoints from a description or OpenAPI spec.",
            "Build API endpoints for this project from the following specification:\n\n{spec}\n\n\
             Follow the project's existing framework and conventions (inspect first). Implement \
             the routes, request/response handling, validation, and error handling, add focused \
             tests, and run the build/tests to confirm they pass. Summarize the endpoints you \
             created and how to call them.",
            &["spec"],
        ),
        w(
            "cleanup-dead-code",
            "Remove dead code",
            "Find and safely remove unused code, imports, and files.",
            "Find and remove dead code in this project — unused functions, imports, variables, \
             and files that nothing references. Use search and the symbol/graph structure to \
             confirm each is truly unused before deleting it. Be conservative: when unsure, leave \
             it and note it. After removing, run the build/tests (run_command/verify) to confirm \
             nothing broke. Summarize what you removed.",
            &[],
        ),
        // A skill pack: ships a starter file the agent then adapts.
        Workflow {
            id: "setup-ci".to_string(),
            name: "Set up CI (GitHub Actions)".to_string(),
            description: "Drop a GitHub Actions workflow and adapt it to this project's real \
                          build/test commands."
                .to_string(),
            prompt:
                "Set up continuous integration for this project with GitHub Actions. A starter \
                     workflow has been created at .github/workflows/ci.yml — inspect the project, \
                     detect its stack, and adapt that file to the REAL build and test commands \
                     (fix the language setup, caching, and steps to match how this project \
                     actually builds and tests). Verify the commands you put in the file work by \
                     running them. Summarize what you configured."
                    .to_string(),
            params: Vec::new(),
            resources: vec![WorkflowResource {
                path: ".github/workflows/ci.yml".to_string(),
                contents: CI_TEMPLATE.to_string(),
            }],
        },
    ]
}

/// Starter CI workflow shipped by the `setup-ci` skill; the agent adapts it.
const CI_TEMPLATE: &str = "# Starter CI — adapt the steps to this project's real build/test.\n\
name: CI\n\
on:\n\
  push:\n\
    branches: [ main ]\n\
  pull_request:\n\
\n\
jobs:\n\
  build-and-test:\n\
    runs-on: ubuntu-latest\n\
    steps:\n\
      - uses: actions/checkout@v4\n\
      # TODO: set up the language toolchain for this project.\n\
      # TODO: install dependencies.\n\
      - name: Build\n\
        run: echo \"replace with the project's build command\"\n\
      - name: Test\n\
        run: echo \"replace with the project's test command\"\n";

/// Materialize a workflow's resource files into the project (skill packs),
/// refusing any absolute or `..`-escaping path. Returns the paths written.
pub fn materialize_resources(
    root: &Path,
    resources: &[WorkflowResource],
) -> std::io::Result<Vec<String>> {
    let mut written = Vec::new();
    for res in resources {
        let rel = Path::new(&res.path);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, Component::ParentDir)) {
            continue;
        }
        let full = root.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, &res.contents)?;
        written.push(res.path.clone());
    }
    Ok(written)
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

/// Add a workflow to the user's set (replacing any with the same id) and save.
pub fn install_workflow(workflow: &Workflow) -> std::io::Result<()> {
    let mut user = load_user_workflows();
    upsert(&mut user, workflow.clone());
    save_user_workflows(&user)
}

/// Remove a user workflow by id and save. Removing a built-in's id just drops
/// the user override, restoring the built-in.
pub fn remove_user_workflow(id: &str) -> std::io::Result<()> {
    let mut user = load_user_workflows();
    user.retain(|w| w.id != id);
    save_user_workflows(&user)
}

/// Whether an id belongs to a built-in workflow (which can't be truly deleted,
/// only overridden).
pub fn is_builtin_workflow(id: &str) -> bool {
    builtin_workflows().iter().any(|w| w.id == id)
}

/// Import workflows from a shared `.toml` file, merging them into the user's set
/// (same id replaces). Returns the number imported.
pub fn import_workflows_from(path: &std::path::Path) -> std::io::Result<usize> {
    let incoming = load_user_workflows_from(path);
    if incoming.is_empty() {
        return Ok(0);
    }
    let mut user = load_user_workflows();
    let count = incoming.len();
    for wf in incoming {
        upsert(&mut user, wf);
    }
    save_user_workflows(&user)?;
    Ok(count)
}

/// Export the given workflows to a shareable `.toml` file others can import.
pub fn export_workflows_to(path: &std::path::Path, workflows: &[Workflow]) -> std::io::Result<()> {
    save_user_workflows_to(path, workflows)
}

/// Insert `wf` into `list`, replacing any existing entry with the same id.
fn upsert(list: &mut Vec<Workflow>, wf: Workflow) {
    if let Some(existing) = list.iter_mut().find(|w| w.id == wf.id) {
        *existing = wf;
    } else {
        list.push(wf);
    }
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
            resources: vec![],
        }];
        save_user_workflows_to(&path, &custom).unwrap();
        let loaded = load_user_workflows_from(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "My release check");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialize_writes_resources_and_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!("kestrel-skill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let resources = vec![
            WorkflowResource {
                path: ".github/workflows/ci.yml".to_string(),
                contents: "name: CI".to_string(),
            },
            WorkflowResource {
                path: "../escape.txt".to_string(),
                contents: "nope".to_string(),
            },
        ];
        let written = materialize_resources(&dir, &resources).unwrap();
        assert_eq!(written, vec![".github/workflows/ci.yml".to_string()]);
        assert!(dir.join(".github/workflows/ci.yml").exists());
        assert!(!dir.parent().unwrap().join("escape.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setup_ci_skill_carries_a_resource() {
        let ci = catalog_workflows()
            .into_iter()
            .find(|w| w.id == "setup-ci")
            .unwrap();
        assert_eq!(ci.resources.len(), 1);
        assert_eq!(ci.resources[0].path, ".github/workflows/ci.yml");
    }

    #[test]
    fn catalog_is_distinct_from_builtins() {
        let catalog = catalog_workflows();
        assert!(!catalog.is_empty());
        for c in &catalog {
            assert!(
                !is_builtin_workflow(&c.id),
                "catalog id {} collides with a built-in",
                c.id
            );
            assert!(!c.name.is_empty() && !c.prompt.is_empty());
        }
    }

    #[test]
    fn export_then_import_file_round_trips() {
        let dir = std::env::temp_dir().join(format!("kestrel-mkt-{}", std::process::id()));
        let path = dir.join("shared.toml");
        let shared = vec![catalog_workflows()[0].clone()];
        export_workflows_to(&path, &shared).unwrap();
        // Importing reads the same shape back out.
        let reloaded = load_user_workflows_from(&path);
        assert_eq!(reloaded.len(), 1);
        assert_eq!(reloaded[0].id, shared[0].id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
