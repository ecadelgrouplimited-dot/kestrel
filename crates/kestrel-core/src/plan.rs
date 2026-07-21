//! The task plan: an autonomous run's spine.
//!
//! A capable agent doesn't just react turn-to-turn — it decomposes a goal into a
//! concrete, checkable list of steps, works them in order, and marks progress.
//! The model maintains the plan through the `update_plan` tool; Kestrel persists
//! it to `.kestrel/plan.json` (so a paused run resumes against the same
//! checklist) and surfaces it live in the UI. The plan also grounds the loop:
//! the agent shouldn't declare victory while steps remain, and a stalled run is
//! nudged back to it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The state of a single plan step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    /// Not started.
    Todo,
    /// Being worked right now.
    Active,
    /// Finished.
    Done,
}

impl StepStatus {
    /// A checkbox-style glyph for the step.
    pub fn glyph(&self) -> &'static str {
        match self {
            StepStatus::Todo => "☐",
            StepStatus::Active => "▶",
            StepStatus::Done => "☑",
        }
    }
}

/// One step in the plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub title: String,
    pub status: StepStatus,
}

/// The agent's working plan for the current goal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plan {
    /// A short restatement of the overall goal.
    #[serde(default)]
    pub goal: String,
    pub steps: Vec<PlanStep>,
}

impl Plan {
    /// (done, total) step counts.
    pub fn progress(&self) -> (usize, usize) {
        let done = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Done)
            .count();
        (done, self.steps.len())
    }

    /// Whether every step is done (and there is at least one).
    pub fn all_done(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(|s| s.status == StepStatus::Done)
    }

    /// The steps still outstanding (todo or active), by title.
    pub fn outstanding(&self) -> Vec<&str> {
        self.steps
            .iter()
            .filter(|s| s.status != StepStatus::Done)
            .map(|s| s.title.as_str())
            .collect()
    }

    /// Render the plan as a compact checklist for the model to read back.
    pub fn render(&self) -> String {
        let (done, total) = self.progress();
        let mut out = format!("Plan ({done}/{total} done)");
        if !self.goal.trim().is_empty() {
            out.push_str(&format!(" — goal: {}", self.goal.trim()));
        }
        out.push('\n');
        for step in &self.steps {
            out.push_str(&format!("  {} {}\n", step.status.glyph(), step.title));
        }
        out
    }
}

/// Where a project's plan is stored.
pub fn plan_path(root: &Path) -> PathBuf {
    root.join(".kestrel").join("plan.json")
}

/// Load a project's saved plan (empty if none/invalid).
pub fn load_plan(root: &Path) -> Plan {
    std::fs::read_to_string(plan_path(root))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Persist a project's plan.
pub fn save_plan(root: &Path, plan: &Plan) -> std::io::Result<()> {
    let path = plan_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(plan)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// Clear a project's plan (used when a fresh goal starts).
pub fn clear_plan(root: &Path) {
    let _ = std::fs::remove_file(plan_path(root));
}

/// Parse the `update_plan` tool input into a [`Plan`]. Tolerant of a bare steps
/// array or a `{ goal, steps }` object, and of missing/odd status strings
/// (defaulting to `todo`).
pub fn plan_from_tool_input(input: &serde_json::Value) -> Plan {
    let goal = input
        .get("goal")
        .and_then(|g| g.as_str())
        .unwrap_or("")
        .to_string();
    let steps_val = input.get("steps").unwrap_or(input);
    let mut steps = Vec::new();
    if let Some(arr) = steps_val.as_array() {
        for item in arr {
            // A step may be a plain string or { title, status }.
            let (title, status) = if let Some(s) = item.as_str() {
                (s.to_string(), StepStatus::Todo)
            } else {
                let title = item
                    .get("title")
                    .and_then(|t| t.as_str())
                    .or_else(|| item.get("step").and_then(|t| t.as_str()))
                    .unwrap_or("")
                    .to_string();
                let status = match item
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("todo")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "done" | "complete" | "completed" => StepStatus::Done,
                    "active" | "in_progress" | "in-progress" | "doing" => StepStatus::Active,
                    _ => StepStatus::Todo,
                };
                (title, status)
            };
            if !title.trim().is_empty() {
                steps.push(PlanStep { title, status });
            }
        }
    }
    Plan { goal, steps }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_and_done() {
        let plan = Plan {
            goal: "ship it".into(),
            steps: vec![
                PlanStep {
                    title: "a".into(),
                    status: StepStatus::Done,
                },
                PlanStep {
                    title: "b".into(),
                    status: StepStatus::Active,
                },
            ],
        };
        assert_eq!(plan.progress(), (1, 2));
        assert!(!plan.all_done());
        assert_eq!(plan.outstanding(), vec!["b"]);
        assert!(plan.render().contains("1/2 done"));
    }

    #[test]
    fn parses_various_tool_shapes() {
        // Object form with statuses.
        let p = plan_from_tool_input(&serde_json::json!({
            "goal": "build site",
            "steps": [
                {"title": "scaffold", "status": "done"},
                {"title": "styles", "status": "active"},
                {"title": "deploy", "status": "todo"},
            ]
        }));
        assert_eq!(p.goal, "build site");
        assert_eq!(p.progress(), (1, 3));
        assert_eq!(p.steps[1].status, StepStatus::Active);

        // Bare array of strings.
        let p = plan_from_tool_input(&serde_json::json!(["one", "two"]));
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0].status, StepStatus::Todo);

        // Alternate status spellings + empty titles dropped.
        let p = plan_from_tool_input(&serde_json::json!({
            "steps": [
                {"title": "x", "status": "in_progress"},
                {"title": "", "status": "todo"},
                {"step": "y", "status": "completed"},
            ]
        }));
        assert_eq!(p.steps.len(), 2);
        assert_eq!(p.steps[0].status, StepStatus::Active);
        assert_eq!(p.steps[1].status, StepStatus::Done);
    }

    #[test]
    fn round_trips_to_disk() {
        let dir = std::env::temp_dir().join(format!("kestrel-plan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let plan = Plan {
            goal: "g".into(),
            steps: vec![PlanStep {
                title: "s".into(),
                status: StepStatus::Todo,
            }],
        };
        save_plan(&dir, &plan).unwrap();
        let loaded = load_plan(&dir);
        assert_eq!(loaded.steps.len(), 1);
        assert_eq!(loaded.goal, "g");
        clear_plan(&dir);
        assert!(load_plan(&dir).steps.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
