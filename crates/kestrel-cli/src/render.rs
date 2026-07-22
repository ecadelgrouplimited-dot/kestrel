//! Turning agent events into terminal output.
//!
//! The same idea as the desktop transcript: an event is not just text, it has a
//! *kind*. Reasoning is transient and belongs on the status line; a tool call is
//! a one-line chip; captured output collapses to a ✅/❌ verdict; the plan is a
//! progress line. Printing all of it identically is what makes agent CLIs
//! unreadable.

use crate::term::{duration, spinner, truncate, Term};
use kestrel_core::AgentEvent;
use std::time::Instant;

/// Renders a run's events, and tracks what it needs for the closing summary.
pub struct Renderer {
    pub term: Term,
    started: Instant,
    ticks: usize,
    /// Files the run wrote, so the summary can count them.
    pub files: std::collections::BTreeSet<String>,
    plan_line: String,
    /// The most recent live thought, redrawn with the spinner.
    thought: String,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            term: Term::new(),
            started: Instant::now(),
            ticks: 0,
            files: std::collections::BTreeSet::new(),
            plan_line: String::new(),
            thought: String::new(),
        }
    }

    pub fn elapsed(&self) -> f32 {
        self.started.elapsed().as_secs_f32()
    }

    /// Handle one event from the agent loop.
    pub fn event(&mut self, event: AgentEvent) {
        match event {
            // Reasoning and narration are transient — they live on the status
            // line so a long think shows movement without flooding the scroll.
            AgentEvent::Thinking(text) => {
                self.thought = text;
                self.redraw_status();
            }
            AgentEvent::Assistant(text) => self.assistant(&text),
            AgentEvent::Tool(text) => {
                let s = self.term.style;
                let coloured = if text.starts_with('⛔') || text.starts_with('🚫') {
                    s.red(&text)
                } else if text.starts_with('⚠') {
                    s.yellow(&text)
                } else {
                    s.dim(&text)
                };
                self.term.line(&format!("  {coloured}"));
                self.thought.clear();
                self.redraw_status();
            }
            AgentEvent::Writing { path, .. } => {
                self.thought = format!("writing {path}");
                self.redraw_status();
            }
            AgentEvent::Wrote { path, .. } => {
                let s = self.term.style;
                self.files.insert(path.clone());
                self.term
                    .line(&format!("  {} {}", s.accent("✍"), s.bold(&path)));
                self.thought.clear();
                self.redraw_status();
            }
            AgentEvent::Plan(plan) => {
                let (done, total) = plan.progress();
                let s = self.term.style;
                let active = plan
                    .steps
                    .iter()
                    .find(|st| st.status == kestrel_core::StepStatus::Active)
                    .map(|st| st.title.clone())
                    .unwrap_or_default();
                self.plan_line = format!("{done}/{total}");
                // Print plan movement permanently — it's the run's spine.
                let bar = progress_bar(done, total);
                let line = if active.is_empty() {
                    format!("  {} {bar} {done}/{total}", s.accent("🗺"))
                } else {
                    format!(
                        "  {} {bar} {done}/{total}  {}",
                        s.accent("🗺"),
                        s.cyan(&active)
                    )
                };
                self.term.line(&line);
                self.redraw_status();
            }
            AgentEvent::Usage(_) => {}
        }
    }

    /// The model's prose, or captured tool output folded to a verdict.
    fn assistant(&mut self, text: &str) {
        let s = self.term.style;
        if kestrel_core::is_tool_output(text) {
            if let Some(summary) = kestrel_core::summarize_output(text) {
                let badge = match summary.status {
                    kestrel_core::Status::Ok => s.green(&format!("✔ {}", summary.headline)),
                    kestrel_core::Status::Failed => s.red(&format!("✘ {}", summary.headline)),
                    kestrel_core::Status::Info => s.dim(&summary.headline),
                };
                self.term.line(&format!("  {badge}"));
                // A failure is worth showing in full; a success isn't.
                if summary.status == kestrel_core::Status::Failed {
                    for line in text
                        .lines()
                        .rev()
                        .take(12)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                    {
                        self.term.line(&format!("    {}", s.dim(line)));
                    }
                }
                self.thought.clear();
                self.redraw_status();
                return;
            }
        }
        // Plain prose from the model.
        self.term.line("");
        for line in text.lines() {
            self.term.line(&format!("  {line}"));
        }
        self.thought.clear();
        self.redraw_status();
    }

    /// Redraw the transient line: spinner, elapsed, and the live thought.
    pub fn redraw_status(&mut self) {
        self.ticks += 1;
        let s = self.term.style;
        let head = format!(
            "{} {}",
            s.accent(&spinner(self.ticks).to_string()),
            s.dim(&duration(self.elapsed()))
        );
        let body = if self.thought.is_empty() {
            s.dim("working…")
        } else {
            s.dim(&truncate(
                &self.thought,
                crate::term::terminal_width().saturating_sub(18),
            ))
        };
        self.term.status(&format!("{head}  {body}"));
    }

    /// The closing card for a finished run.
    pub fn summary(&mut self, ok: bool, incomplete: bool, cost: f64) {
        self.term.finish_status();
        let s = self.term.style;
        let head = if incomplete {
            s.yellow("⏸ paused — /continue to resume")
        } else if ok {
            s.green("✔ run complete")
        } else {
            s.red("✘ run failed")
        };
        let mut parts = vec![format!("{} files", self.files.len())];
        if !self.plan_line.is_empty() {
            parts.push(format!("plan {}", self.plan_line));
        }
        parts.push(duration(self.elapsed()));
        if cost > 0.0 {
            parts.push(format!("${cost:.4}"));
        }
        self.term.line("");
        self.term
            .line(&format!("  {head}   {}", s.dim(&parts.join(" · "))));
        self.term.line("");
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// A tiny unicode progress bar, e.g. `███░░░░`.
pub fn progress_bar(done: usize, total: usize) -> String {
    const WIDTH: usize = 10;
    if total == 0 {
        return "░".repeat(WIDTH);
    }
    let filled = (done * WIDTH).div_ceil(total).min(WIDTH);
    format!("{}{}", "█".repeat(filled), "░".repeat(WIDTH - filled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bar_fills_proportionally() {
        assert_eq!(progress_bar(0, 8), "░░░░░░░░░░");
        assert_eq!(progress_bar(8, 8), "██████████");
        assert_eq!(progress_bar(4, 8), "█████░░░░░");
        // A single completed step of many still shows movement.
        assert!(progress_bar(1, 9).starts_with('█'));
        // No division by zero before a plan exists.
        assert_eq!(progress_bar(0, 0).chars().count(), 10);
    }
}
